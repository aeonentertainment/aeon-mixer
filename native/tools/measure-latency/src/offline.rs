//! The engine's own share of the mic path: a known sound sent through each ingest kind, the real
//! jitter buffer, decoder, slot and master, on the offline driver. No device and no socket, so it
//! runs anywhere and gives the same answer every time.

use engine::block::{BLOCK, RATE};
use engine::driver::OfflineDriver;
use engine::params::{CleanupParams, CorrectorParams, EffectParams};

use crate::ingest::Ingest;
use crate::matched_filter::{MatchedFilter, first_arrival};
use crate::programme::Programme;
use crate::rig::{Delay, Rig};

/// How long the stream's first packet waits for the next block to drain it, in frames. The anchor
/// fixes that wait for the rest of the stream, so it is part of the delay, and where a stream
/// begins is chance: one frame's wait and a whole block's are the two ends of what it can cost.
const FIRST_PACKET_WAITS: [usize; 5] = [1, 32, 64, 96, BLOCK];

/// The voices a corrected row sings, low, middle and high: A2, A3 and A4, each 40 cents sharp so
/// the corrector keeps shifting it and its delay keeps moving.
pub fn voices() -> [f64; 3] {
    [110.0, 220.0, 440.0].map(|hz| hz * 2f64.powf(40.0 / 1200.0))
}

/// Full correction to the nearest semitone at once, with no guide: the most the corrector moves.
const CORRECTING: CorrectorParams =
    CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };

/// One path's number, in frames, as a range across where in a block the stream began.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub ingest: Ingest,
    /// The slot's corrector, when it is engaged.
    pub corrector: Option<Correction>,
    /// Whether the slot's high-pass, gate and compressor are on. None buffers, so this row says
    /// the path holds a sample no longer; the high-pass's phase delay on a low fundamental is
    /// under this chirp, which starts at 500 Hz, and decision 0012 gives it.
    pub cleanup: bool,
    pub delay: Delay,
    /// What the slot reports as its delay when the measurement ends; none for a `push` slot.
    pub slot_delay_ms: Option<f32>,
    pub lookahead: usize,
    /// From a sample entering the sender to its leaving the master, least and most.
    pub lag: (f64, f64),
}

/// The voice a corrected row sang, and the delay the slot's corrector reported as each chirp went
/// through, least and most, in frames. The chirps go through at different heights of its sawtooth,
/// so this is the span they found, not the most it can reach: a budget counts
/// `correctorMaxDelayMs`.
#[derive(Clone, Copy, Debug)]
pub struct Correction {
    pub hz: f64,
    pub reported: (f64, f64),
}

impl Row {
    /// What the engine itself holds a sample for, least and most: the lag less what the sender
    /// spends gathering and encoding.
    pub fn held(&self) -> (f64, f64) {
        let sender = (self.ingest.gather_frames() + self.lookahead) as f64;
        (self.lag.0 - sender, self.lag.1 - sender)
    }
}

/// Runs every path at the given playout delay, with the slot's corrector off and then on for each
/// voice, then with its cleanup on, and every jitter-buffered path once more with the slot left to
/// choose its own delay and cleanup off.
pub fn measure_all(playout_frames: u32) -> Result<Vec<Row>, String> {
    let corrected = Ingest::every().into_iter().flat_map(|ingest| {
        let off = measure(ingest, playout_frames, false);
        let on = voices().map(|hz| measure_corrected(ingest, playout_frames, hz));
        [off].into_iter().chain(on)
    });
    let cleaned = Ingest::every().into_iter().map(|ingest| measure(ingest, playout_frames, true));
    let chooses = Ingest::every().into_iter().filter(Ingest::has_playout_delay).map(measure_chosen);
    corrected.chain(cleaned).chain(chooses).collect()
}

/// Sends the programme through `ingest` from each first-packet wait and reads where it came out.
pub fn measure(ingest: Ingest, playout_frames: u32, cleanup: bool) -> Result<Row, String> {
    measure_with(ingest, Delay::Pinned(playout_frames), Programme::offline(), cleanup)
}

/// The same with the corrector engaged, on phrases sung at `hz` ahead of each chirp.
pub fn measure_corrected(ingest: Ingest, playout_frames: u32, hz: f64) -> Result<Row, String> {
    measure_with(ingest, Delay::Pinned(playout_frames), Programme::corrected(hz), false)
}

/// The same with the slot choosing its delay, read once it has. There is no network here, so this
/// is what a slot comes to on a link with no jitter, the least it can hold.
pub fn measure_chosen(ingest: Ingest) -> Result<Row, String> {
    measure_with(ingest, Delay::Chosen, Programme::offline_settled(), false)
}

fn measure_with(
    ingest: Ingest,
    delay: Delay,
    programme: Programme,
    cleanup: bool,
) -> Result<Row, String> {
    let mut lag = (f64::INFINITY, f64::NEG_INFINITY);
    let mut reported = (f64::INFINITY, f64::NEG_INFINITY);
    let mut last = None;
    for wait in FIRST_PACKET_WAITS {
        let run = run(ingest, delay, programme, cleanup, wait)?;
        for crossing in &run.crossings {
            lag = (lag.0.min(crossing.lag), lag.1.max(crossing.lag));
            reported = (reported.0.min(crossing.corrector), reported.1.max(crossing.corrector));
        }
        last = Some(run);
    }
    let Run { lookahead, slot_delay_ms, .. } = last.expect("there are waits");
    let corrector = programme.phrases.map(|phrases| Correction { hz: phrases.hz, reported });
    Ok(Row { ingest, corrector, cleanup, delay, slot_delay_ms, lookahead, lag })
}

/// One stream through `ingest`, its first packet waiting `wait` frames for a block.
struct Run {
    crossings: Vec<Crossing>,
    lookahead: usize,
    slot_delay_ms: Option<f32>,
}

/// One chirp through the path, in frames: from entering the sender to leaving the master, and the
/// corrector's delay as it went through.
struct Crossing {
    lag: f64,
    corrector: f64,
}

/// Sends the programme through `ingest` once, from one first-packet wait. The corrector is engaged
/// when the programme sings ahead of its chirps: a voice is what it corrects, and the chirp alone
/// would set it shifting.
fn run(
    ingest: Ingest,
    delay: Delay,
    programme: Programme,
    cleanup: bool,
    wait: usize,
) -> Result<Run, String> {
    let signal = programme.signal();
    let effects = EffectParams {
        corrector: if programme.phrases.is_some() {
            CORRECTING
        } else {
            CorrectorParams::default()
        },
        cleanup: CleanupParams { enabled: cleanup },
        ..EffectParams::default()
    };
    let phase = (2 * BLOCK - (ingest.gather_frames() + wait) % BLOCK) as u64 % BLOCK as u64;
    let mut rig =
        Rig::new(ingest, delay, &signal, phase).map_err(|error| format!("{ingest}: {error}"))?;
    rig.controls.params.update(|params| params.slots[0].effects = effects);
    let lookahead = rig.lookahead;
    let stats = rig.stats.clone();
    let (left, corrector) = pump(rig, signal.len() + programme.interval());
    let envelope = MatchedFilter::new(programme.chirp.samples()).envelope(&left);
    let crossings = programme
        .starts()
        .map(|start| {
            let from = start + phase as usize;
            // No room offline, so the strongest peak is the chirp: Opus leaves noise near a short
            // one within the 20 dB a room's onset rule would take
            let arrival = first_arrival(&envelope, from..from + programme.interval(), 0)
                .ok_or_else(|| format!("{ingest}: chirp at {start} never came out"))?;
            // As the chirp enters the sender, the phrase has gone through the corrector and the
            // chirp has yet to reach it, so this is the delay it meets
            let corrector = f64::from(corrector[from / BLOCK]);
            Ok(Crossing { lag: arrival.frame - from as f64, corrector })
        })
        .collect::<Result<_, String>>()?;
    let slot_delay_ms = stats.map(|stats| stats.read().playout_delay_ms);
    Ok(Run { crossings, lookahead, slot_delay_ms })
}

/// The left channel over `frames`, a block at a time, each block under the allocation guard in
/// tests, and the corrector's delay in frames as `stats()` reads it after each block.
pub fn pump(rig: Rig, frames: usize) -> (Vec<f32>, Vec<u32>) {
    let Rig { render, controls: _controls, corrector, .. } = rig;
    let mut driver = OfflineDriver::new(render);
    let mut block = [0.0f32; 2 * BLOCK];
    let mut left = Vec::with_capacity(frames);
    let mut held = Vec::with_capacity(frames.div_ceil(BLOCK));
    while left.len() < frames {
        #[cfg(test)]
        assert_no_alloc::assert_no_alloc(|| driver.pump_into(&mut block));
        #[cfg(not(test))]
        driver.pump_into(&mut block);
        left.extend(block.iter().step_by(2));
        held.push(corrector.frames());
    }
    (left, held)
}

/// A frame count in milliseconds.
pub fn ms(frames: f64) -> f64 {
    frames * 1000.0 / f64::from(RATE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Encoder;

    const PLAYOUT: u32 = RATE * 15 / 1000;

    /// What a jitter-buffered path may hold a sample for: the playout delay and the wait for the
    /// next block, up to one block. Nothing else on the mic path may delay a sample (design
    /// section 3). A new buffer on this path fails here, to within half a frame on raw PCM. Opus
    /// gets 3 frames, since a lossy waveform's peak moves a little under the decoder.
    #[test]
    fn a_jitter_buffered_path_holds_a_sample_for_the_playout_delay_and_a_block_at_most() {
        let playout = f64::from(PLAYOUT);
        let block = BLOCK as f64;
        for ingest in Ingest::every().into_iter().filter(Ingest::has_playout_delay) {
            let tolerance = if matches!(ingest, Ingest::L16 { .. }) { 0.5 } else { 3.0 };
            let row = measure(ingest, PLAYOUT, false).unwrap();
            let (least, most) = row.held();
            assert!(
                least >= playout - tolerance && most <= playout + block + tolerance,
                "{ingest}: held {least}..{most} frames"
            );
            // The range is the whole of what a block's wait can cost, not part of it
            assert!(
                most - least > block - 2.0 - 2.0 * tolerance,
                "{ingest}: {least}..{most} frames"
            );
        }
    }

    /// The slot's cleanup buffers nothing, so on every path a sample leaves when it did with
    /// cleanup off, to within a frame of the chirp's peak moving under the high-pass's phase.
    #[test]
    fn cleanup_holds_a_sample_no_longer_on_any_path() {
        for ingest in Ingest::every() {
            let off = measure(ingest, PLAYOUT, false).unwrap();
            let on = measure(ingest, PLAYOUT, true).unwrap();
            let moved = [on.lag.0 - off.lag.0, on.lag.1 - off.lag.1];
            assert!(moved.iter().all(|m| m.abs() < 1.0), "{ingest}: moved {moved:?} frames");
        }
    }

    /// A slot left to choose on a link with no jitter comes to the floor, whatever the frame and
    /// wherever in a block the stream began: the fall measures from the earliest packet, so the
    /// block's wait a held stream keeps for good is gone from the range too.
    #[test]
    fn a_slot_that_chooses_holds_the_floor() {
        let floor = engine::ingest::delay_chooser::FLOOR as f64;
        for ingest in [
            Ingest::L16 { frame: 240 },
            Ingest::Rtp { frame: 960, encoder: Encoder::RestrictedLowDelay },
            Ingest::Rtp { frame: 120, encoder: Encoder::RestrictedLowDelay },
        ] {
            let row = measure_chosen(ingest).unwrap();
            let reported = f64::from(row.slot_delay_ms.unwrap()) * f64::from(RATE) / 1000.0;
            assert!((reported - floor).abs() < 0.5, "{ingest}: reported {reported}");
            let (least, most) = row.held();
            assert!(
                least >= floor - 3.0 && most <= floor + BLOCK as f64 + 3.0,
                "{ingest}: held {least}..{most} frames"
            );
        }
    }

    /// A playout delay is what the row says it is, so the number moves frame for frame with it.
    #[test]
    fn the_lag_moves_with_the_playout_delay() {
        let ingest = Ingest::L16 { frame: 240 };
        let usual = measure(ingest, PLAYOUT, false).unwrap();
        let shorter = measure(ingest, PLAYOUT / 3, false).unwrap();
        let moved = usual.lag.0 - shorter.lag.0;
        assert!((moved - f64::from(PLAYOUT - PLAYOUT / 3)).abs() < 0.5, "moved {moved} frames");
    }

    /// A host's 10 ms chunks on a 128-frame grid: a chunk that lands after the last one ran out
    /// slips the stream later, and it settles at the chunk and at most two blocks. Section 8.4's
    /// 30 ms ceiling is not what this slot runs at; it only catches a host that falls behind.
    #[test]
    fn a_push_slot_settles_within_two_blocks_of_its_chunk() {
        let chunk = RATE as usize / 100;
        let row = measure(Ingest::Push { chunk }, PLAYOUT, false).unwrap();
        let (least, most) = row.held();
        assert!(least >= -0.5 && most <= (2 * BLOCK) as f64 + 0.5, "held {least}..{most} frames");
    }

    /// Opus's restricted low delay spends 2.5 ms ahead of the packet where voice mode spends 6.5,
    /// and the engine holds the sample just as long either way: #335 gets to quote the difference.
    #[test]
    fn low_delay_costs_the_sender_four_milliseconds_less_and_the_engine_nothing() {
        let frame = 240;
        let voip = measure(Ingest::Rtp { frame, encoder: Encoder::Voip }, PLAYOUT, false).unwrap();
        let low =
            measure(Ingest::Rtp { frame, encoder: Encoder::RestrictedLowDelay }, PLAYOUT, false)
                .unwrap();
        assert_eq!(voip.lookahead - low.lookahead, 192);
        let difference = voip.lag.0 - low.lag.0;
        assert!((difference - 192.0).abs() < 4.0, "the lags differ by {difference} frames");
    }

    /// The corrector delays the voice by what the slot reports and nothing else on the path moves:
    /// stream by stream, each chirp after a phrase leaves when the same chirp left with no phrase
    /// and the corrector off, plus the delay the slot reported as it went through. To within a
    /// frame on raw PCM, the shifter's fractional read point, and 3 on Opus. The phrases leave the
    /// delay over a millisecond apart, so the run follows a sawtooth, not one height of it.
    #[test]
    fn a_corrected_chirp_leaves_late_by_what_the_corrector_reports() {
        let [low, middle, _] = voices();
        for (ingest, hz, tolerance) in [
            (Ingest::L16 { frame: 240 }, low, 1.0),
            (Ingest::Rtp { frame: 120, encoder: Encoder::RestrictedLowDelay }, middle, 3.0),
        ] {
            let row = measure_corrected(ingest, PLAYOUT, hz).unwrap();
            let reported = row.corrector.expect("a corrected row says so").reported;
            assert!(reported.1 - reported.0 > 48.0, "{ingest}: one height, {reported:?}");
            let pinned = Delay::Pinned(PLAYOUT);
            for wait in FIRST_PACKET_WAITS {
                let off = run(ingest, pinned, Programme::offline(), false, wait).unwrap();
                let on = run(ingest, pinned, Programme::corrected(hz), false, wait).unwrap();
                let path = off.crossings[0].lag;
                for crossing in on.crossings {
                    let beyond = crossing.lag - path - crossing.corrector;
                    assert!(
                        beyond.abs() <= tolerance,
                        "{ingest}, wait {wait}: {beyond} frames past what the corrector reported"
                    );
                }
            }
        }
    }
}
