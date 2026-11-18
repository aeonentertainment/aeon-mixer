//! A move to another output mid-song, on the offline driver: what `Movable::moved` says of the
//! backing, and the lyric clock a host keeps from it. The host counts wall time from `start`, less
//! the time between its pause and resume acks, less each move's hold, and never asks whether the
//! song was paused at the move. Its clock must name the backing frame the room hears.

use assert_no_alloc::assert_no_alloc;

use crate::audio_thread::{AudioThread, Controls};
use crate::backing::{Backing, Planar, Stems};
use crate::block::{BLOCK, RATE, Stereo};
use crate::command::{Command, Event};
use crate::driver::Movable;
use crate::graph::Graph;

const SECOND: usize = RATE as usize;
/// The gap a move to the MacBook Pro speakers left in decision 0016, 41 ms.
const GAP: usize = 41 * SECOND / 1000;
/// The backing frame where the stem steps up from silence, which the room hears as the step.
const STEP: usize = 3 * SECOND;
/// A host stamps an ack as it arrives, here at the end of the block that carried it, so its
/// clock may trail by a block at the start and at each resume. A hold counted wrong is a gap off.
const TOLERANCE: usize = 2 * BLOCK;

const START: u32 = 1;
const PAUSE: u32 = 2;
const RESUME: u32 = 3;

/// The host's lyric clock, in frames of wall time.
#[derive(Default)]
struct Host {
    started: Option<usize>,
    paused_at: Option<usize>,
    paused: usize,
    held: usize,
}

impl Host {
    fn position(&self, wall: usize) -> usize {
        let paused = self.paused + self.paused_at.map_or(0, |at| wall - at);
        wall - self.started.expect("started") - paused - self.held
    }
}

struct Song {
    audio: AudioThread,
    controls: Controls,
    /// The left channel of every block rendered.
    heard: Vec<f32>,
    /// Frames of wall time: those rendered, and the gaps no stream ran in.
    wall: usize,
    /// The wall time at each frame heard.
    walls: Vec<usize>,
    host: Host,
}

impl Song {
    /// A backing silent until `STEP` and level from there, swapped in and started.
    fn new() -> Self {
        let (audio, mut controls) = AudioThread::new();
        let mut stem = vec![0.0; 2 * STEP];
        stem[STEP..].fill(0.25);
        let mut graph = Graph::silent();
        let stems = Stems::new(Planar { left: stem.clone(), right: stem }, None);
        graph.set_backing(Backing::new(stems, 0));
        controls.commands.push(Command::Swap { id: 0, graph: Box::new(graph) }).ok().unwrap();
        controls.commands.push(Command::Start { id: START }).ok().unwrap();
        let mut song = Self {
            audio,
            controls,
            heard: Vec::new(),
            wall: 0,
            walls: Vec::new(),
            host: Host::default(),
        };
        song.play(BLOCK);
        song
    }

    fn send(&mut self, command: Command) {
        assert!(self.controls.commands.push(command).is_ok(), "the command ring is full");
    }

    /// At least `frames` more, in whole blocks under the allocation guard, with the host hearing
    /// each block's acks as it ends.
    fn play(&mut self, frames: usize) {
        for _ in 0..frames.div_ceil(BLOCK) {
            let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
            assert_no_alloc(|| self.audio.render(Stereo { left: &mut left, right: &mut right }));
            self.heard.extend_from_slice(&left);
            self.walls.extend((0..BLOCK).map(|k| self.wall + k));
            self.wall += BLOCK;
            while let Some(event) = self.controls.events.pop() {
                let Event::Ack { id, .. } = event else { continue };
                let host = &mut self.host;
                match id {
                    START => host.started = Some(self.wall),
                    PAUSE => host.paused_at = Some(self.wall),
                    RESUME => host.paused += self.wall - host.paused_at.take().expect("paused"),
                    _ => {}
                }
            }
        }
    }

    /// Stops the stream for `GAP` and starts the next, as the follower does. The follower holds
    /// the playhead for the whole gap when `moved` says the stop caught it moving on, and the
    /// host's clock moves by that hold. Returns what `moved` said.
    fn move_output(&mut self) -> bool {
        let holds = Movable::moved(&mut self.audio);
        self.wall += GAP;
        if holds {
            self.host.held += GAP;
        }
        holds
    }

    /// Plays on until the room hears the step, and returns where the host's clock put it.
    fn host_at_step(&mut self) -> usize {
        while !self.heard.iter().any(|sample| *sample > 0.0) {
            self.play(SECOND / 10);
        }
        let heard = self.heard.iter().position(|sample| *sample > 0.0).expect("the step");
        self.host.position(self.walls[heard])
    }
}

fn assert_on_the_step(song: &mut Song) {
    let host = song.host_at_step();
    assert!(host.abs_diff(STEP) <= TOLERANCE, "the host put the step at {host}, it is at {STEP}");
}

#[test]
fn a_move_while_playing_holds_the_backing_for_the_gap() {
    let mut song = Song::new();
    song.play(SECOND);
    assert!(song.move_output(), "playing, the stop holds the playhead");
    assert_on_the_step(&mut song);
}

/// The issue's race: a resume sent before the move lands after it, so the gap fell in the pause.
#[test]
fn a_move_during_a_pause_holds_nothing_though_a_resume_waits() {
    let mut song = Song::new();
    song.play(SECOND);
    song.send(Command::Pause { id: PAUSE });
    song.play(SECOND / 5);
    assert!(song.host.paused_at.is_some(), "the pause has landed");
    song.send(Command::Resume { id: RESUME });
    assert!(!song.move_output(), "paused, nothing moves on to hold");
    assert_on_the_step(&mut song);
}

/// A pause fades out for 20 ms before it holds, and the playhead runs on through the fade, so a
/// move inside it holds the playhead though the host has already sent its pause.
#[test]
fn a_move_during_a_pause_fade_holds_the_backing_for_the_gap() {
    let mut song = Song::new();
    song.play(SECOND);
    song.send(Command::Pause { id: PAUSE });
    song.play(BLOCK);
    assert!(song.host.paused_at.is_none(), "the pause is still fading");
    assert!(song.move_output(), "fading, the stop holds the playhead");
    song.play(SECOND / 5);
    song.send(Command::Resume { id: RESUME });
    assert_on_the_step(&mut song);
}

#[test]
fn a_move_just_after_a_resume_holds_the_backing_for_the_gap() {
    let mut song = Song::new();
    song.play(SECOND);
    song.send(Command::Pause { id: PAUSE });
    song.play(SECOND / 5);
    song.send(Command::Resume { id: RESUME });
    song.play(BLOCK);
    assert!(song.move_output(), "resumed, the stop holds the playhead");
    assert_on_the_step(&mut song);
}

#[test]
fn a_move_with_no_song_or_before_start_holds_nothing() {
    let (mut audio, mut controls) = AudioThread::new();
    assert!(!Movable::moved(&mut audio), "between songs");
    let mut graph = Graph::silent();
    graph.set_backing(Backing::new(Stems::new(Planar::silence(SECOND), None), 0));
    controls.commands.push(Command::Swap { id: 0, graph: Box::new(graph) }).ok().unwrap();
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    assert_no_alloc(|| audio.render(Stereo { left: &mut left, right: &mut right }));
    assert!(!Movable::moved(&mut audio), "armed, not started");
}

/// A seek fades out before it jumps, with the playhead running on, so a move in the fade holds
/// it. A song that has ended, or been swapped out for the next one, has nothing left to hold,
/// though its graph still fades out of the room.
#[test]
fn a_seek_fade_holds_the_backing_and_an_ended_or_swapped_out_song_does_not() {
    let mut song = Song::new();
    song.play(SECOND);
    song.send(Command::Seek { id: 9, to_frame: 5 * SECOND as u64 });
    song.play(BLOCK);
    assert!(song.move_output(), "fading to the seek, the stop holds the playhead");
    song.play(2 * SECOND);
    assert!(!song.move_output(), "ended, nothing moves on to hold");

    let mut song = Song::new();
    song.play(SECOND);
    song.send(Command::Swap { id: 10, graph: Box::new(Graph::silent()) });
    song.play(BLOCK);
    assert!(!song.move_output(), "swapped out mid-fade, the song is no longer the room's");
}
