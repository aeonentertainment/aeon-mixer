//! The engine's own share of the mic path: a known sound sent through each ingest kind, the real
//! jitter buffer, decoder, slot and master, on the offline driver. No device and no socket, so it
//! runs anywhere and gives the same answer every time.

use engine::block::{BLOCK, RATE};
use engine::driver::OfflineDriver;

use crate::ingest::Ingest;
use crate::matched_filter::{MatchedFilter, first_arrival};
use crate::programme::Programme;
use crate::rig::Rig;

/// How long the stream's first packet waits for the next block to drain it, in frames. The anchor
/// fixes that wait for the rest of the stream, so it is part of the delay, and where a stream
/// begins is chance: one frame's wait and a whole block's are the two ends of what it can cost.
const FIRST_PACKET_WAITS: [usize; 5] = [1, 32, 64, 96, BLOCK];

/// One path's number, in frames, as a range across where in a block the stream began.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub ingest: Ingest,
    /// Whether the slot's corrector is engaged. Off in every row today: a row with it on needs the
    /// voiced phrase and short sweep decision 0009 describes, since the corrector acts on this chirp.
    pub corrected: bool,
    /// Whether the slot's high-pass, gate and compressor are on. None buffers, so this row says
    /// the path holds a sample no longer; the high-pass's phase delay on a low fundamental is
    /// under this chirp, which starts at 500 Hz, and decision 0012 gives it.
    pub cleanup: bool,
    pub playout_frames: u32,
    pub lookahead: usize,
    /// From a sample entering the sender to its leaving the master, least and most.
    pub lag: (f64, f64),
}

impl Row {
    /// What the engine itself holds a sample for, least and most: the lag less what the sender
    /// spends gathering and encoding.
    pub fn held(&self) -> (f64, f64) {
        let sender = (self.ingest.gather_frames() + self.lookahead) as f64;
        (self.lag.0 - sender, self.lag.1 - sender)
    }
}

/// Runs every path at the given playout delay, with the slot's cleanup off and then on.
pub fn measure_all(playout_frames: u32) -> Result<Vec<Row>, String> {
    [false, true]
        .into_iter()
        .flat_map(|cleanup| {
            Ingest::every().into_iter().map(move |ingest| measure(ingest, playout_frames, cleanup))
        })
        .collect()
}

/// Sends the programme through `ingest` from each first-packet wait and reads where it came out.
pub fn measure(ingest: Ingest, playout_frames: u32, cleanup: bool) -> Result<Row, String> {
    let programme = Programme::offline();
    let signal = programme.signal();
    let filter = MatchedFilter::new(programme.chirp.samples());
    let mut lag = (f64::INFINITY, f64::NEG_INFINITY);
    let mut lookahead = 0;
    for wait in FIRST_PACKET_WAITS {
        let phase = (2 * BLOCK - (ingest.gather_frames() + wait) % BLOCK) as u64 % BLOCK as u64;
        let mut rig = Rig::new(ingest, playout_frames, &signal, phase)
            .map_err(|error| format!("{ingest}: {error}"))?;
        rig.controls.params.update(|params| params.slots[0].effects.cleanup.enabled = cleanup);
        lookahead = rig.lookahead;
        let envelope = filter.envelope(&pump(rig, signal.len() + programme.interval()));
        for start in programme.starts() {
            let from = start + phase as usize;
            let arrival = first_arrival(&envelope, from..from + programme.interval())
                .ok_or_else(|| format!("{ingest}: chirp at {start} never came out"))?;
            let seen = arrival.frame - from as f64;
            lag = (lag.0.min(seen), lag.1.max(seen));
        }
    }
    Ok(Row { ingest, corrected: false, cleanup, playout_frames, lookahead, lag })
}

/// The left channel over `frames`, a block at a time, each block under the allocation guard in
/// tests.
pub fn pump(rig: Rig, frames: usize) -> Vec<f32> {
    let Rig { render, controls: _controls, .. } = rig;
    let mut driver = OfflineDriver::new(render);
    let mut block = [0.0f32; 2 * BLOCK];
    let mut left = Vec::with_capacity(frames);
    while left.len() < frames {
        #[cfg(test)]
        assert_no_alloc::assert_no_alloc(|| driver.pump_into(&mut block));
        #[cfg(not(test))]
        driver.pump_into(&mut block);
        left.extend(block.iter().step_by(2));
    }
    left
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
}
