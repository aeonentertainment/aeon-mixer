//! A bundle's song played again on the offline driver: the mixer its manifest describes, under
//! the profile given, each line applied at its frame and each packet pushed before the first
//! block that starts at or after its arrival (docs/design.md 8.11). Every block renders under
//! the allocation guard. The same bundle plays to the same samples every time.

use std::sync::Arc;

use assert_no_alloc::assert_no_alloc;
use engine::audio_thread::{AudioThread, Controls};
use engine::backing::{Backing, RenderJob, StemFiles};
use engine::block::BLOCK;
use engine::command::{Command, Event as EngineEvent};
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
use super::line::{Event, Guide};
use super::manifest::StemKind;
use super::profile::{PlayoutDelay, Profile};
use super::silence::Silence;

/// `STOP_FADE_MS` in `src/backing.ts`: how long `stopAtMs` fades the stems out.
const STOP_FADE_MS: f64 = 3000.0;

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

/// What the control thread reads of a slot once its graph is on the audio thread.
struct Reports {
    corrector_delay: Arc<CorrectorDelay>,
    echo: Arc<EchoReport>,
    /// The guard's count after the last block. It only rises today; summing its rises keeps the
    /// song's total whole if a reset ever clears it (docs/design.md 8.9).
    howl_catches: u32,
}

/// One slot's ingest, the control thread's end of it.
struct Ingest {
    packets: Option<(PacketProducer, Arc<SharedStats>)>,
    ring_full: usize,
}

/// Plays `bundle` under `profile`, with the stems in `stems`, or the mic path alone without.
pub fn play(
    bundle: &Bundle,
    profile: &Profile,
    stems: Option<StemFiles>,
) -> Result<Played, String> {
    let manifest = &bundle.manifest;
    let mut ingests = Vec::new();
    let mut slots = Vec::new();
    for (position, slot) in manifest.slots.iter().enumerate() {
        if slot.index != position {
            return Err(format!("manifest.json lists slot {} at {position}", slot.index));
        }
        let (ingest, source) = open(&slot.kind, profile.ingest.playout_delay_ms)?;
        ingests.push(ingest);
        slots.push(Slot::new(source));
    }
    let mut reports: Vec<Reports> = slots
        .iter()
        .map(|slot| Reports {
            corrector_delay: slot.corrector_delay(),
            echo: slot.echo_report(),
            howl_catches: 0,
        })
        .collect();
    let mut graph = Box::new(Graph::new(slots));
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
    let mut driver = OfflineDriver::new(audio.into_render());
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
    let mut desk = Desk {
        controls: &mut controls,
        profile,
        bundle,
        graph: Some(graph),
        guides: vec![None; manifest.slots.len()],
        next_id: 0,
        rendered,
        notes: &mut notes,
    };
    let mut played: Vec<PlayedSlot> = manifest
        .slots
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
    let mut mix = Vec::with_capacity(blocks * 2 * BLOCK);
    let mut master_db = Vec::new();
    let mut block = [0.0f32; 2 * BLOCK];
    let mut events = bundle.events.iter().peekable();
    let mut arrivals = bundle.arrivals.iter().peekable();
    for k in 0..blocks {
        let start = (k * BLOCK) as i64;
        while let Some((_, event)) = events.next_if(|&&(at, _)| at <= start) {
            desk.apply(event)?;
        }
        while let Some((slot, arrival)) = arrivals.next_if(|(_, arrival)| arrival.at <= start) {
            let ingest = &mut ingests[*slot];
            if let Some((producer, _)) = &mut ingest.packets
                && !producer.push(&arrival.packet)
            {
                ingest.ring_full += 1;
            }
        }
        assert_no_alloc(|| driver.pump_into(&mut block));
        mix.extend_from_slice(&block);
        while let Some(event) = desk.controls.events.pop() {
            match event {
                EngineEvent::Level { slot, rms_db, .. } => {
                    if let Some(slot) = played.get_mut(usize::from(slot)) {
                        slot.meter_db.push(rms_db);
                    }
                }
                EngineEvent::MasterLevel { rms_db, .. } => master_db.push(rms_db),
                EngineEvent::Pitch {
                    slot,
                    position_ms,
                    hz,
                    confidence,
                    target_midi,
                    error_semitones,
                } => {
                    if let Some(slot) = played.get_mut(usize::from(slot)) {
                        slot.readings.push(Reading {
                            position_ms,
                            hz,
                            confidence,
                            target_midi,
                            error_semitones,
                        });
                    }
                }
                _ => {}
            }
        }
        for (slot, report) in played.iter_mut().zip(&mut reports) {
            slot.corrector_delay_frames.push(report.corrector_delay.frames());
            slot.echo.extend(report.echo.findings());
            let catches = report.echo.howl_catches();
            slot.howl_catches += catches.saturating_sub(report.howl_catches);
            report.howl_catches = catches;
        }
        while desk.controls.retired.pop().is_some() {}
    }
    for (slot, ingest) in played.iter_mut().zip(&ingests) {
        slot.ring_full = ingest.ring_full;
        if let Some((_, stats)) = &ingest.packets {
            slot.stats = stats.read();
        }
    }
    Ok(Played { mix, slots: played, master_db, notes })
}

/// A slot's source as `node/src/ingest.rs` opens it, with no socket: the replay is its network
/// thread.
fn open(kind: &str, delay: PlayoutDelay) -> Result<(Ingest, Box<dyn Source>), String> {
    let codec = match kind {
        "rtp" => Codec::opus().map_err(|e| e.to_string())?,
        "l16" => Codec::L16,
        "push" | "capture" => {
            return Ok((Ingest { packets: None, ring_full: 0 }, Box::new(Silence)));
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
    Ok((Ingest { packets: Some((producer, stats)), ring_full: 0 }, source))
}

/// The mixer's side of the song: each line turned into the parameter or command the live mixer
/// sent for it, as `src/mixer.ts` sends them.
struct Desk<'a> {
    controls: &'a mut Controls,
    profile: &'a Profile,
    bundle: &'a Bundle,
    /// The graph until `start` swaps it in.
    graph: Option<Box<Graph>>,
    /// Each slot's guide, held while armed and sent at `start`.
    guides: Vec<Option<Guide>>,
    next_id: u32,
    /// The stems' length as this replay rendered them.
    rendered: Option<u64>,
    notes: &'a mut Vec<String>,
}

impl Desk<'_> {
    fn apply(&mut self, event: &Event) -> Result<(), String> {
        let live = self.graph.is_none();
        match event {
            Event::SetSlot { index, settings } => {
                let slot = self.slot(*index)?;
                self.controls.params.update(|p| calls::set_slot(&mut p.slots[slot], settings));
            }
            Event::SetSlotEffects { index, effects } => {
                let slot = self.slot(*index)?;
                let chain = calls::effects(effects, &self.profile.slot.effects)?;
                self.controls.params.update(|p| p.slots[slot].effects = chain);
            }
            Event::SetSlotGuide { index, guide, guide_offset_ms } => {
                let slot = self.slot(*index)?;
                if let Some(offset) = *guide_offset_ms {
                    self.controls.params.update(|p| p.slots[slot].guide_offset_ms = offset);
                }
                self.guides[slot] = guide.clone();
                if live {
                    self.send_guide(slot)?;
                }
            }
            Event::ResetSlot { index } => {
                let slot = self.slot(*index)? as u8;
                if live {
                    self.send(Command::ResetSlot { slot })?;
                }
            }
            Event::Start {} => self.start()?,
            Event::Seek { to_ms } => {
                let to_frame = ms_to_frames(*to_ms);
                self.steer(|id| Command::Seek { id, to_frame })?;
            }
            Event::Pause {} => self.steer(|id| Command::Pause { id })?,
            Event::Resume {} => self.steer(|id| Command::Resume { id })?,
            Event::SetBackingKey { semitones } => {
                let semitones = *semitones;
                self.steer(|id| Command::SetKey { id, semitones })?;
            }
            Event::SetStemGain { kind, gain } => self.controls.params.update(|p| match kind {
                StemKind::Instrumental => p.stems.instrumental = *gain,
                StemKind::GuideVocals => p.stems.guide = *gain,
            }),
            Event::BackingRendered { frames } => {
                if let Some(rendered) = self.rendered
                    && rendered != *frames
                {
                    self.notes.push(format!(
                        "the stems rendered to {rendered} frames, where the song's were {frames}"
                    ));
                }
            }
            Event::OutputChanged {} => {
                self.notes.push("the output moved mid-song, which replays as no move".to_owned());
            }
            Event::Error { message } => {
                self.notes.push(format!("the song reported an error: {message}"));
            }
            // The replay's own engine acks the commands it is sent
            Event::Ack {} | Event::BackingEnded {} | Event::End {} => {}
        }
        Ok(())
    }

    /// `start`: the graph swapped in with its guides, the backing started, and its stop armed.
    fn start(&mut self) -> Result<(), String> {
        let graph = self.graph.take().ok_or("events.jsonl starts the song twice")?;
        self.steer(|id| Command::Swap { id, graph })?;
        for slot in 0..self.guides.len() {
            if self.guides[slot].is_some() {
                self.send_guide(slot)?;
            }
        }
        self.steer(|id| Command::Start { id })?;
        if let Some(stop_at_ms) = self.bundle.manifest.backing.as_ref().and_then(|b| b.stop_at_ms) {
            let at_frame = ms_to_frames(stop_at_ms);
            let fade_frames = ms_to_frames(STOP_FADE_MS) as u32;
            self.steer(|id| Command::StopAt { id, at_frame, fade_frames })?;
        }
        Ok(())
    }

    /// The slot's guide to the live graph, or none to take it away.
    fn send_guide(&mut self, slot: usize) -> Result<(), String> {
        let guide = self.guides[slot].as_ref().map(|guide| Box::new(calls::guide(guide)));
        self.send(Command::SetGuide { slot: slot as u8, guide })
    }

    fn steer(&mut self, command: impl FnOnce(u32) -> Command) -> Result<(), String> {
        self.next_id += 1;
        self.send(command(self.next_id))
    }

    fn send(&mut self, command: Command) -> Result<(), String> {
        self.controls.commands.push(command).map_err(|_| "the command ring is full".to_owned())
    }

    fn slot(&self, index: usize) -> Result<usize, String> {
        if index < self.guides.len() {
            Ok(index)
        } else {
            Err(format!("events.jsonl names slot {index}, which the manifest does not list"))
        }
    }
}
