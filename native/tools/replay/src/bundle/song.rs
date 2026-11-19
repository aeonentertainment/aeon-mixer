//! A bundle's song played again: the mixer its manifest describes, under the profile given, each
//! line applied at its frame and each packet pushed before the first block that starts at or
//! after its arrival (docs/design.md 8.11). `Song::prepare` makes all of it before the first block
//! for whatever clock plays it; `play` plays it on the offline driver, every block under the
//! allocation guard. The same bundle plays to the same samples every time.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use assert_no_alloc::assert_no_alloc;
use engine::audio_thread::AudioThread;
use engine::backing::{Backing, RenderJob, StemSources};
use engine::block::BLOCK;
use engine::command::{Event as EngineEvent, EventConsumer, RetiredConsumer};
use engine::driver::OfflineDriver;
use engine::graph::Graph;
use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::{
    Codec, JitterBuffer, PacketProducer, PacketSource, SharedStats, SlotStats, packet_ring,
};
use engine::params::StemGains;
use engine::slot::Slot;
use engine::slot::corrector_delay::CorrectorDelay;
use engine::slot::echo_report::{EchoFindings, EchoReport};
use engine::slot::effects::Reading;
use engine::slot::source::Source;

use super::Bundle;
use super::calls::{self, ms_to_frames};
use super::desk::Desk;
use super::player::Player;
use super::profile::{PlayoutDelay, Profile};
use super::silence::Silence;

pub struct Played {
    /// Interleaved stereo, from the manifest's frame.
    pub mix: Vec<f32>,
    pub slots: Vec<PlayedSlot>,
    /// The master meter's readings, as `masterLevel` carries them.
    pub master_db: Vec<f32>,
    /// What the replay could not play as the song did, in the order it met each.
    pub notes: Vec<String>,
}

pub struct PlayedSlot {
    pub kind: String,
    /// Whether the bundle recorded the slot's packets; one it did not replays as silence.
    pub recorded: bool,
    /// The slot's counters as `stats()` gives them; zero for a slot that replays as silence.
    pub stats: SlotStats,
    /// The slot's meter readings, as `level` carries them.
    pub meter_db: Vec<f32>,
    /// Packets the slot's ring had no room for.
    pub ring_full: usize,
    /// The corrector's readings, every 10 ms, as `pitch` carries them.
    pub readings: Vec<Reading>,
    /// How far the corrector held the voice after each block, as `stats()` reads it.
    pub corrector_delay_frames: Vec<u32>,
    /// The echo canceller's findings after each block that had them, as `stats()` reads them.
    pub echo: Vec<EchoFindings>,
    /// Times the howl guard ducked.
    pub howl_catches: u32,
}

/// A bundle's song made ready for any clock: the engine's audio thread, the player a render
/// calls before each block, and the control thread's ends. Block 0 is the manifest's frame.
pub struct Song {
    pub audio: AudioThread,
    pub player: Player,
    pub events: EventConsumer,
    pub retired: RetiredConsumer,
    /// The audio thread's count of frames rendered.
    pub frames: Arc<AtomicU64>,
    pub slots: Vec<SongSlot>,
    /// Blocks the song lasts: to its `end` line, or its last line or packet without one.
    pub blocks: usize,
    /// What the replay cannot play as the song did, in the order it met each.
    pub notes: Vec<String>,
}

/// What the control thread reads of a slot once its graph is on the audio thread.
pub struct SongSlot {
    pub kind: String,
    pub recorded: bool,
    /// The slot's counters, for a slot with packets to play.
    pub stats: Option<Arc<SharedStats>>,
    pub corrector_delay: Arc<CorrectorDelay>,
    pub echo: Arc<EchoReport>,
}

/// One slot's ingest, the control thread's end of it.
pub(super) struct Ingest {
    pub(super) packets: Option<(PacketProducer, Arc<SharedStats>)>,
}

impl Song {
    /// `bundle` under `profile`, with the stems in `stems`, or the mic path alone without.
    pub fn prepare(
        bundle: &Bundle,
        profile: &Profile,
        stems: Option<StemSources>,
    ) -> Result<Self, String> {
        let manifest = &bundle.manifest;
        let mut rings = Vec::new();
        let mut slots = Vec::new();
        let mut engine_slots = Vec::new();
        for (position, slot) in manifest.slots.iter().enumerate() {
            if slot.index != position {
                return Err(format!("manifest.json lists slot {} at {position}", slot.index));
            }
            let (ingest, source) = open(&slot.kind, profile.ingest.playout_delay_ms)?;
            let engine_slot = Slot::new(source);
            let (ring, stats) = ingest.packets.unzip();
            rings.push(ring);
            slots.push(SongSlot {
                kind: slot.kind.clone(),
                recorded: slot.recorded,
                stats,
                corrector_delay: engine_slot.corrector_delay(),
                echo: engine_slot.echo_report(),
            });
            engine_slots.push(engine_slot);
        }
        let mut graph = Box::new(Graph::new(engine_slots));
        let mut notes = Vec::new();
        let mut rendered = None;
        if let (Some(backing), Some(files)) = (&manifest.backing, stems) {
            let stems = RenderJob::start(files, backing.key, backing.speed)
                .finish()
                .map_err(|e| format!("the stems will not render: {e}"))?;
            rendered = Some(stems.frames);
            graph.set_backing(Backing::new(stems, backing.key));
        } else if manifest.backing.is_some() {
            notes.push("no stems given, so the mic path plays alone".to_owned());
        }

        let (audio, mut controls) = AudioThread::new();
        controls.params.update(|params| {
            for slot in &mut params.slots[..manifest.slots.len()] {
                *slot = calls::slot_start(&profile.slot);
            }
            if let Some(backing) = &manifest.backing {
                let defaults = &profile.backing.stem_gains;
                params.stems = StemGains {
                    instrumental: backing.stem_gains.instrumental.unwrap_or(defaults.instrumental),
                    guide: backing.stem_gains.guide_vocals.unwrap_or(defaults.guide_vocals),
                };
            }
        });

        let last = bundle.events.last().map_or(0, |&(at, _)| at);
        let until = if bundle.ended {
            last
        } else {
            notes.push("the bundle has no end line: it was cut short".to_owned());
            last.max(bundle.arrivals.last().map_or(0, |(_, arrival)| arrival.at))
        };
        let blocks = usize::try_from(until).unwrap_or(0).div_ceil(BLOCK);
        // A line or packet past the last block's start is never taken up
        let last_start = blocks as i64 * BLOCK as i64 - BLOCK as i64;
        let mut desk =
            Desk::new(profile, bundle, *controls.params.current(), graph, rendered, &mut notes);
        for (at, event) in bundle.events.iter().take_while(|&&(at, _)| at <= last_start) {
            desk.apply(*at, event)?;
        }
        let steps = desk.into_steps();
        let arrivals =
            bundle.arrivals.iter().take_while(|(_, arrival)| arrival.at <= last_start).copied();
        let player =
            Player::new(controls.params, controls.commands, steps, arrivals.collect(), rings);
        Ok(Self {
            audio,
            player,
            events: controls.events,
            retired: controls.retired,
            frames: controls.frames,
            slots,
            blocks,
            notes,
        })
    }
}

/// Plays `bundle` under `profile` on the offline driver, with the stems in `stems`, or the mic
/// path alone without.
pub fn play(
    bundle: &Bundle,
    profile: &Profile,
    stems: Option<StemSources>,
) -> Result<Played, String> {
    let Song { audio, mut player, mut events, mut retired, slots, blocks, notes, .. } =
        Song::prepare(bundle, profile, stems)?;
    let mut driver = OfflineDriver::new(audio.into_render());
    let mut played: Vec<PlayedSlot> = slots
        .iter()
        .map(|slot| PlayedSlot {
            kind: slot.kind.clone(),
            recorded: slot.recorded,
            stats: SlotStats::default(),
            meter_db: Vec::new(),
            ring_full: 0,
            readings: Vec::new(),
            corrector_delay_frames: Vec::with_capacity(blocks),
            echo: Vec::new(),
            howl_catches: 0,
        })
        .collect();
    // The guard's count after the last block. It only rises today; summing its rises keeps the
    // song's total whole if a reset ever clears it (docs/design.md 8.9).
    let mut howl_catches = vec![0; slots.len()];
    let mut mix = Vec::with_capacity(blocks * 2 * BLOCK);
    let mut master_db = Vec::new();
    let mut block = [0.0f32; 2 * BLOCK];
    for _ in 0..blocks {
        let handed = assert_no_alloc(|| {
            let handed = player.before_block();
            driver.pump_into(&mut block);
            handed
        });
        if !handed {
            return Err("the command ring is full".to_owned());
        }
        mix.extend_from_slice(&block);
        while let Some(event) = events.pop() {
            match event {
                EngineEvent::Level { slot, rms_db, .. } => {
                    if let Some(slot) = played.get_mut(usize::from(slot)) {
                        slot.meter_db.push(rms_db);
                    }
                }
                EngineEvent::MasterLevel { rms_db, .. } => master_db.push(rms_db),
                EngineEvent::Pitch { slot, reading } => {
                    if let Some(slot) = played.get_mut(usize::from(slot)) {
                        slot.readings.push(reading);
                    }
                }
                _ => {}
            }
        }
        for ((slot, song_slot), seen) in played.iter_mut().zip(&slots).zip(&mut howl_catches) {
            slot.corrector_delay_frames.push(song_slot.corrector_delay.frames());
            slot.echo.extend(song_slot.echo.findings());
            let catches = song_slot.echo.howl_catches();
            slot.howl_catches += catches.saturating_sub(*seen);
            *seen = catches;
        }
        while retired.pop().is_some() {}
    }
    for (index, (slot, song_slot)) in played.iter_mut().zip(&slots).enumerate() {
        slot.ring_full = player.ring_full(index);
        if let Some(stats) = &song_slot.stats {
            slot.stats = stats.read();
        }
    }
    Ok(Played { mix, slots: played, master_db, notes })
}

/// A slot's source as `node/src/ingest.rs` opens it, with no socket: the replay is its network
/// thread, and so is a live song writing a bundle.
pub(super) fn open(kind: &str, delay: PlayoutDelay) -> Result<(Ingest, Box<dyn Source>), String> {
    let codec = match kind {
        "rtp" => Codec::opus().map_err(|e| e.to_string())?,
        "l16" => Codec::L16,
        "push" | "capture" => {
            return Ok((Ingest { packets: None }, Box::new(Silence)));
        }
        other => return Err(format!("manifest.json names a slot kind {other:?}")),
    };
    let (producer, consumer) = packet_ring(RING_CAPACITY);
    let jitter = match delay {
        PlayoutDelay::Pinned(ms) => JitterBuffer::new(ms_to_frames(ms) as u32, codec),
        PlayoutDelay::Auto(_) => JitterBuffer::chosen(codec),
    };
    let stats = jitter.shared_stats();
    let source = Box::new(PacketSource::new(consumer, jitter));
    Ok((Ingest { packets: Some((producer, stats)) }, source))
}
