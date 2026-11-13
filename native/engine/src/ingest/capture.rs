//! A `capture` slot's source: a house mic on this machine, delivered by its device's capture
//! thread into an SPSC ring and pulled a block at a time, with no jitter buffer (docs/design.md
//! 8.4, decision 0018).
//!
//! The input and the output may run on different clocks, so the ring's fill drifts. The voice
//! holds the lowest fill it sees before a pull, over a window, at a target: a block to pull and a
//! margin. When the trough is off target it moves the delay one sample per block, each spliced in
//! by linear interpolation across the block so nothing steps. A run of pulls the ring cannot fill
//! raises the target by what they lacked, up to one input period more. A trough a whole window
//! more than a period over the target is a stall's backlog, and is dropped at once.
//!
//! The fill before a pull is not how long the mic's sample waited: it swings with where the pull
//! falls between two deliveries and in its output's wake. The wait itself holds still, the trough
//! and the headroom less where in the input's period the pulls fall, which the voice reports as
//! its latency and its playout delay. It counts every frame it moves its delay by, so the echo
//! canceller moves its reference with the echo.

use std::sync::Arc;

use kernels::SourceDelay;

use crate::block::{BLOCK, RATE};
use crate::ingest::capture_producer::CaptureProducer;
use crate::ingest::stats::{SharedStats, SlotStats};
use crate::slot::source::Source;

/// Frames the ring holds: 85 ms, room for a stall's backlog on any backend.
pub const RING_FRAMES: usize = 4096;

/// The longest input period a capture slot takes: 21 ms. The output's callback may be longer, as
/// far as the ring holds it beside the period (`fits`). Every backend measured grants 480 or less.
pub const MAX_PERIOD_FRAMES: usize = 1024;

/// Whether the ring holds the healthy fill under an input of `period` and an output that pulls
/// `burst` at a wake: the target at its ceiling and the headroom, so twice the period, the burst
/// and the margin. The ring is fixed, so the audio thread never sizes it.
fn fits(period: usize, burst: usize) -> bool {
    2 * period + burst + MARGIN <= RING_FRAMES
}

/// The period to ask of an input under an output whose callback takes `output_frames` at a wake:
/// the output's own, so an interface that is both runs one cycle, unless the ring cannot hold it
/// beside that callback, and then the longest whole-block period it can, 896 frames under 2048.
/// Never over `MAX_PERIOD_FRAMES`, and never under a block.
pub fn input_period_for(output_frames: u32) -> u32 {
    let output = (output_frames as usize).max(1);
    let burst = output.div_ceil(BLOCK) * BLOCK;
    let longest = RING_FRAMES.saturating_sub(burst + MARGIN) / 2 / BLOCK * BLOCK;
    let asked = if fits(output, burst) { output } else { longest };
    asked.clamp(BLOCK.min(output), MAX_PERIOD_FRAMES) as u32
}

/// What a trough keeps over the block it pulls at opening: a third of a millisecond, for the
/// wakes of two threads.
const MARGIN: usize = 16;

/// Blocks over which the trough is read: 0.34 s, longer than any pattern two periods make.
const WINDOW_BLOCKS: u32 = 128;

/// A ring for one input. The device's capture thread pushes into the producer; `for_input` then
/// tells the voice what the device granted, before the voice goes to the audio thread.
pub fn capture_voice() -> (CaptureProducer, CaptureVoice) {
    let (producer, consumer) = rtrb::RingBuffer::new(RING_FRAMES);
    let voice = CaptureVoice {
        ring: consumer,
        period: BLOCK,
        burst: BLOCK,
        hardware: 0,
        target: BLOCK + MARGIN,
        phase: Phase::Opening,
        delay: 0,
        played: false,
        short: None,
        moved: 0,
        recovering: false,
        stats: SlotStats::default(),
        shared: Arc::new(SharedStats::default()),
    };
    (CaptureProducer { ring: producer }, voice)
}

pub struct CaptureVoice {
    ring: rtrb::Consumer<f32>,
    period: usize,
    /// The most the output pulls back to back at one wake, in whole blocks: a 480-frame callback
    /// takes four, and the input delivers nothing between them.
    burst: usize,
    /// What the OS reports between the hardware and a delivery.
    hardware: u32,
    /// The fill the trough is held at, before a pull.
    target: usize,
    phase: Phase,
    /// The fill before the last pull.
    delay: usize,
    /// Whether the last pull played the mic, past priming and settling and not silenced.
    played: bool,
    /// The pulls running short since the last full one: how many, what they lacked in all, and what
    /// the first of them lacked.
    short: Option<Short>,
    /// Frames the slot has moved its delay by while playing: spliced in or out, a gap's silence,
    /// a backlog dropped. The echo canceller follows them, less the target's own moves, which
    /// the latency already says.
    moved: i64,
    /// From an input that stopped until the trough is back on the target, which the moves of
    /// that recovery do not count toward.
    recovering: bool,
    stats: SlotStats,
    shared: Arc<SharedStats>,
}

enum Phase {
    /// The first pull drops what arrived while the mixer was armed.
    Opening,
    /// Silence until the ring holds its headroom over the target.
    Priming,
    /// Still silence, for a window, draining a block a pull to read the trough; then the excess
    /// goes at once, so the first sample played waits as long as every one after it.
    Settling {
        trough: usize,
        blocks: u32,
    },
    Measuring {
        trough: usize,
        blocks: u32,
    },
    /// Frames still to add to the fill (positive) or take from it (negative), one per block.
    Correcting {
        frames: i64,
    },
    /// An output moved to a callback the ring cannot hold: silence until the next move, which no
    /// priming could end. `counted` once the stats carry it.
    Unplayable {
        counted: bool,
    },
}

#[derive(Clone, Copy)]
struct Short {
    pulls: usize,
    lacked: usize,
    first: usize,
}

const MEASURING: Phase = Phase::Measuring { trough: usize::MAX, blocks: 0 };

impl CaptureVoice {
    /// An input that delivers `period_frames` at a time, with `hardware_frames` between its
    /// hardware and a delivery as the OS reports it, so the guide clock and the echo canceller can
    /// count the whole of the wait, under an output whose callback takes `output_frames` at a wake.
    /// Refuses a period over `MAX_PERIOD_FRAMES`, or a callback the ring cannot hold beside it.
    pub fn for_input(
        self,
        period_frames: u32,
        hardware_frames: u32,
        output_frames: u32,
    ) -> Result<Self, String> {
        let most = MAX_PERIOD_FRAMES;
        let (period, output) = (period_frames as usize, output_frames as usize);
        if !(1..=most).contains(&period) {
            return Err(format!(
                "the input delivers {period} frames at a time; a capture slot takes 1 to {most}"
            ));
        }
        let burst = output.div_ceil(BLOCK) * BLOCK;
        if output == 0 || !fits(period, burst) {
            let longest = (RING_FRAMES - 2 * period - MARGIN) / BLOCK * BLOCK;
            return Err(format!(
                "the output takes {output} frames at a time; under a {period}-frame input a \
                 capture slot plays under 1 to {longest}"
            ));
        }
        Ok(Self { period, burst, hardware: hardware_frames, ..self })
    }

    /// The counters as the audio thread last published them, for the control thread.
    pub fn shared_stats(&self) -> Arc<SharedStats> {
        Arc::clone(&self.shared)
    }

    /// The fill the voice holds its trough at, in frames.
    pub fn target_frames(&self) -> usize {
        self.target
    }

    /// The fill before the last pull, in frames.
    pub fn ring_frames(&self) -> usize {
        self.delay
    }

    /// Whether the last pull played the mic. Priming and settling hold the fill over the target
    /// and the headroom on purpose, and play silence, so a measurement of the fill judges only the
    /// pulls that played.
    pub fn played(&self) -> bool {
        self.played
    }

    /// How long a sample waits from the mic's delivery to its pull, the input's period included.
    /// A pull just after a delivery finds more in the ring than one just before it, and its samples
    /// waited as much less, so the wait is the trough and its headroom less where in the input's
    /// period the pulls fall, which the fill cannot see. Under an output that takes a block a wake
    /// the pulls fall on a lattice of `gcd(period, BLOCK)`, and the wait is within that step of
    /// the headroom. Under one that takes several, the step is the whole period. This is the step's
    /// middle: within half a step of the truth, and still while the trough is on target.
    pub fn wait_frames(&self) -> usize {
        self.target + self.headroom_frames() - self.step() / 2
    }

    /// How far into the input's period the wait can fall short of the trough and its headroom.
    fn step(&self) -> usize {
        if self.burst == BLOCK { gcd(self.period, BLOCK) } else { self.period }
    }

    /// What the fill holds over the trough just after a delivery, at the first pull of a wake: the
    /// input's period and the blocks the wake will pull after this one. A healthy fill before a
    /// pull stays at or under the target and this.
    pub fn headroom_frames(&self) -> usize {
        self.period + self.burst - BLOCK
    }

    /// The most the target rises to: one input period over where it opens.
    fn ceiling_target(&self) -> usize {
        BLOCK + self.period + MARGIN
    }

    fn discard(&mut self, frames: usize) {
        if let Ok(chunk) = self.ring.read_chunk(frames) {
            chunk.commit_all();
        }
    }

    /// Moves `frames` into `out` at a rate of `frames / out.len()`, reading one past them when it
    /// stretches, and commits them. Linear interpolation over a whole block turns a one-sample
    /// move into a fractional delay that sweeps across it: no step, and at worst -2 dB at 10 kHz
    /// for that block.
    fn splice(&mut self, frames: usize, out: &mut [f32]) {
        let n = out.len();
        let peek = (frames + 1).min(self.ring.slots());
        let Ok(chunk) = self.ring.read_chunk(peek) else {
            out.fill(0.0);
            return;
        };
        let (first, second) = chunk.as_slices();
        if frames == n {
            let (head, tail) = out.split_at_mut(first.len().min(n));
            head.copy_from_slice(&first[..head.len()]);
            tail.copy_from_slice(&second[..tail.len()]);
        } else {
            let at = |i: usize| if i < first.len() { first[i] } else { second[i - first.len()] };
            for (i, sample) in out.iter_mut().enumerate() {
                let (index, rest) = ((i * frames) / n, (i * frames) % n);
                *sample = if rest == 0 {
                    at(index)
                } else {
                    let a = at(index);
                    a + (at(index + 1) - a) * rest as f32 / n as f32
                };
            }
        }
        chunk.commit(frames);
    }

    fn publish(&mut self, stats: SlotStats) {
        if stats != self.stats {
            self.stats = stats;
            self.shared.publish(&stats);
        }
    }
}

impl Source for CaptureVoice {
    fn pull(&mut self, out: &mut [f32]) {
        self.played = false;
        let n = out.len();
        let mut stats = self.stats;
        stats.playout_delay_ms = (self.wait_frames() as f64 * 1000.0 / f64::from(RATE)) as f32;
        if let Phase::Unplayable { counted } = self.phase {
            self.discard(self.ring.slots());
            self.delay = 0;
            out.fill(0.0);
            if !counted {
                stats.resyncs += 1;
                stats.playout_delay_ms = 0.0;
                self.phase = Phase::Unplayable { counted: true };
                self.publish(stats);
            }
            return;
        }
        if let Phase::Opening = self.phase {
            self.discard(self.ring.slots());
            self.phase = Phase::Priming;
        }
        let fill = self.ring.slots();
        // A trough is only sure to sit on the target when the fill starts its headroom over it,
        // since a delivery may have just landed. The same holds after a backlog is dropped.
        if let Phase::Priming = self.phase {
            if fill < self.target + self.headroom_frames() {
                self.delay = fill;
                out.fill(0.0);
                return;
            }
            self.phase = Phase::Settling { trough: usize::MAX, blocks: 0 };
        }
        // Played while the trough is still being found, the opening's excess would have to go a
        // sample a block, sliding the delay by up to a period under an echo canceller that has
        // just found the path. Silent, it goes at once.
        if let Phase::Settling { trough, blocks } = self.phase {
            self.delay = fill;
            out.fill(0.0);
            if fill < n {
                self.phase = Phase::Priming;
                return;
            }
            let trough = trough.min(fill);
            self.discard(n);
            self.phase = if blocks + 1 < WINDOW_BLOCKS {
                Phase::Settling { trough, blocks: blocks + 1 }
            } else {
                self.discard(trough.saturating_sub(self.target));
                MEASURING
            };
            return;
        }
        self.delay = fill;

        let step = match self.phase {
            Phase::Correcting { frames } if frames > 0 && fill >= n => -1,
            Phase::Correcting { frames } if frames < 0 && fill > n => 1,
            _ => 0,
        };
        let frames = (n as i64 + step) as usize;
        if fill < frames {
            // Short: play what there is, then silence, and keep count until a pull is full again
            self.splice(fill, &mut out[..fill]);
            self.played = true;
            out[fill..].fill(0.0);
            stats.concealed += 1;
            let lacked = n - fill;
            self.short = Some(match self.short {
                Some(run) => Short { pulls: run.pulls + 1, lacked: run.lacked + lacked, ..run },
                None => Short { pulls: 1, lacked, first: lacked },
            });
            self.phase = MEASURING;
            self.publish(stats);
            return;
        }
        // Deliveries came back within the most pulls one wake can take: the phase of two clocks or
        // a late wake ran the ring short, and the target covers all of it from now on. Longer, the
        // input stopped, which no target covers, and only the first pull's lack is the phase's.
        // The limit is the ring's, not this output's, so it holds after the output moves.
        if let Some(run) = self.short.take() {
            let phase = run.pulls <= (self.burst / BLOCK).max(MAX_PERIOD_FRAMES / BLOCK);
            let raise = if phase { run.lacked } else { run.first };
            let was = self.target;
            self.target = (self.target + raise + MARGIN).min(self.ceiling_target());
            if phase {
                // The silence played the samples that came late that much later
                self.moved += raise as i64;
            } else {
                // What a stopped input hands over after, a backlog or nothing, says nothing of
                // how far the delay moved, so its recovery goes uncounted and the latency the
                // target sets is where the delay ends up
                self.moved += (self.target - was) as i64;
                self.recovering = true;
            }
        }
        self.splice(frames, out);
        self.played = true;
        if !self.recovering {
            self.moved -= step;
        }

        self.phase = match self.phase {
            Phase::Correcting { frames } => match frames + step {
                0 => {
                    self.recovering = false;
                    MEASURING
                }
                left => Phase::Correcting { frames: left },
            },
            Phase::Measuring { trough, blocks } => {
                let trough = trough.min(fill);
                if blocks + 1 < WINDOW_BLOCKS {
                    Phase::Measuring { trough, blocks: blocks + 1 }
                } else if trough == self.target {
                    self.recovering = false;
                    MEASURING
                } else if trough > self.target + self.period {
                    // A backlog: the whole window stayed more than a period over the target, which
                    // drift never does between windows. A fill high for one pull is no backlog, since
                    // an output catching up after a late wake fills that way and drains it at once.
                    self.discard(trough - self.target);
                    if !self.recovering {
                        self.moved -= (trough - self.target) as i64;
                    }
                    stats.resyncs += 1;
                    MEASURING
                } else {
                    stats.resyncs += 1;
                    Phase::Correcting { frames: self.target as i64 - trough as i64 }
                }
            }
            Phase::Opening | Phase::Priming | Phase::Settling { .. } | Phase::Unplayable { .. } => {
                MEASURING
            }
        };
        self.publish(stats);
    }

    /// A move: what the ring holds was paced by the clock that stopped, and the move's gap waited
    /// in it. The slot primes and settles again, silent, as at its opening.
    fn restart(&mut self) {
        self.short = None;
        self.phase = match self.phase {
            Phase::Unplayable { counted } => Phase::Unplayable { counted },
            _ => Phase::Opening,
        };
    }

    /// `ResetSlot` hands the mic to another singer, but it is the same device: the ring keeps its
    /// fill and plays on, and the slot's echo stages keep the path.
    fn keeps_its_sender(&self) -> bool {
        true
    }

    /// The new output's callback sets the headroom; a change means priming and settling again
    /// under it. A callback the ring cannot hold beside the input's period leaves the slot silent
    /// until the next move, since priming under it would only run dry, again and again.
    fn output_moved(&mut self, callback_frames: u32) {
        let burst = (callback_frames as usize).max(1).div_ceil(BLOCK) * BLOCK;
        if burst == self.burst {
            return;
        }
        self.burst = burst;
        self.short = None;
        self.phase = if fits(self.period, burst) {
            Phase::Opening
        } else {
            Phase::Unplayable { counted: false }
        };
    }

    /// How long ago the mic heard what `pull` hands over: the hardware's share as the OS reports
    /// it, and the wait from delivery to pull. It moves only with the target, so the echo
    /// canceller, which follows a latency that holds for two blocks, follows each real move.
    fn latency_frames(&self) -> u32 {
        self.hardware + self.wait_frames() as u32
    }

    /// The latency holds still between moves of the target, but the delay does not: the slot
    /// splices frames in and out a frame a block, plays a gap's silence and drops a backlog, and
    /// each moves the echo. The canceller follows the latency and these, so they leave out the
    /// target's own moves, which the latency says. The drift between the two clocks is the
    /// canceller's to follow.
    fn delay(&self) -> SourceDelay {
        SourceDelay {
            latency_frames: self.latency_frames(),
            spliced_frames: self.moved - self.target as i64,
            ..SourceDelay::default()
        }
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::audio_thread::AudioThread;
    use crate::block::RATE;
    use crate::command::Command;
    use crate::driver::OfflineDriver;
    use crate::graph::Graph;
    use crate::slot::Slot;

    const SECOND: u64 = RATE as u64 / BLOCK as u64;

    /// A fake input on its own clock, `ppm` fast or slow against the output, delivering `period`
    /// frames of a sine whenever its clock has them, a few frames early or late as a real wake
    /// is. The output clock is the block count: each `block` pulls one from the voice, under the
    /// allocation guard.
    struct Rig {
        producer: CaptureProducer,
        voice: CaptureVoice,
        ppm: i64,
        period: u64,
        offset: u64,
        jitter: u64,
        seed: u64,
        delivered: u64,
        blocks: u64,
        stalled: bool,
        /// The output's callback: the input delivers only as one begins, and it pulls the
        /// blocks the callback needs back to back.
        callback: u64,
        next_wake: u64,
        /// How long the first sample of the last pull had waited since the mic heard it.
        waited: u64,
        hz: f64,
        scratch: Vec<f32>,
        out: [f32; BLOCK],
    }

    impl Rig {
        fn new(ppm: i64, period: u64) -> Self {
            let (producer, voice) = capture_voice();
            let voice = voice.for_input(period as u32, 0, BLOCK as u32).expect("a period it takes");
            Self {
                producer,
                voice,
                ppm,
                period,
                offset: 37,
                jitter: 0,
                seed: 0x5eed,
                delivered: 0,
                blocks: 0,
                stalled: false,
                callback: BLOCK as u64,
                next_wake: 0,
                waited: 0,
                hz: 440.0,
                scratch: vec![0.0; RING_FRAMES],
                out: [0.0; BLOCK],
            }
        }

        fn jittered(mut self, frames: u64) -> Self {
            self.jitter = frames;
            self
        }

        /// The same input under an output whose callback takes `callback` frames at a wake.
        fn under(mut self, callback: u64) -> Self {
            let (producer, voice) = capture_voice();
            self.producer = producer;
            self.voice = voice
                .for_input(self.period as u32, 0, callback as u32)
                .expect("a callback it takes");
            self.callback = callback;
            self
        }

        /// The frames the input's clock has run by the end of the current block.
        fn due(&self) -> u64 {
            let ticks = (self.blocks + 1) * BLOCK as u64;
            (u128::from(ticks) * (1_000_000 + self.ppm) as u128 / 1_000_000) as u64 + self.offset
        }

        fn wake(&mut self) -> u64 {
            self.seed =
                self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            if self.jitter == 0 { 0 } else { (self.seed >> 33) % (2 * self.jitter + 1) }
        }

        fn block(&mut self) {
            let wakes = self.blocks * BLOCK as u64 >= self.next_wake;
            if wakes {
                self.next_wake += self.callback;
            }
            if wakes && !self.stalled {
                while self.delivered + self.period + self.wake() <= self.due() + self.jitter {
                    self.deliver(self.period as usize);
                }
            }
            let heard = self.due();
            let Self { voice, out, .. } = self;
            assert_no_alloc(|| voice.pull(out));
            self.waited = voice.ring_frames() as u64 + heard - self.delivered;
            self.blocks += 1;
        }

        fn deliver(&mut self, frames: usize) {
            for (k, sample) in self.scratch[..frames].iter_mut().enumerate() {
                let t = (self.delivered + k as u64) as f64 / f64::from(RATE);
                *sample = (0.5 * (TAU * self.hz * t).sin()) as f32;
            }
            let Self { producer, scratch, .. } = self;
            producer.push(&scratch[..frames]);
            self.delivered += frames as u64;
        }

        /// Runs `blocks` and returns the lowest, mean and highest delay, in frames, over them.
        fn run(&mut self, blocks: u64) -> (u32, f64, u32) {
            let (mut low, mut sum, mut high) = (u32::MAX, 0u64, 0);
            for _ in 0..blocks {
                self.block();
                let delay = self.voice.ring_frames() as u32;
                low = low.min(delay);
                sum += u64::from(delay);
                high = high.max(delay);
            }
            (low, sum as f64 / blocks as f64, high)
        }

        /// The input stops for `blocks`. With `backlog` it then hands over all it missed at
        /// once, as ALSA and WASAPI do after a late wake; without, those frames are gone, as
        /// CoreAudio skips a cycle.
        fn stall(&mut self, blocks: u64, backlog: bool) {
            self.stalled = true;
            self.run(blocks);
            self.stalled = false;
            if !backlog {
                let missed = (self.due() - self.delivered) / self.period * self.period;
                self.delivered += missed;
            }
        }

        fn stats(&self) -> SlotStats {
            self.voice.shared_stats().read()
        }
    }

    #[test]
    fn a_mic_on_the_outputs_clock_holds_its_target_exactly() {
        for period in [64, 128, 480] {
            let mut rig = Rig::new(0, period);
            rig.run(2 * SECOND);
            let settled = rig.stats();
            let target = rig.voice.target_frames() as u32;
            assert_eq!(target, (BLOCK + MARGIN) as u32, "{period}: nothing raised it");
            let (low, _, high) = rig.run(60 * SECOND);
            assert_eq!(low, target, "{period}: the trough sits on the target");
            assert!(high < target + period as u32, "{period}: {high}");
            assert_eq!(rig.stats(), settled, "{period}: no move after settling, no gap");
            assert_eq!(settled.concealed, 0, "{period}");
        }
    }

    /// An hour at 200 ppm either way, twice the tolerance of the crystals in ordinary audio
    /// devices, with wakes up to 8 frames early or late, under a block-a-wake output and a
    /// 480-frame callback. In the first half hour the phase of the two clocks and the jitter may run
    /// the ring short, in at most one wake's worth of pulls, and the target rises to cover all of
    /// it. From then on nothing is short, and the fill stays between a block and its headroom over
    /// the target.
    #[test]
    fn a_mic_on_its_own_clock_holds_its_delay_within_a_bound_for_an_hour() {
        for (period, callback) in [(128u64, 128u64), (480, 128), (128, 480), (480, 480)] {
            for ppm in [200, -200] {
                let mut rig = Rig::new(ppm, period).under(callback).jittered(8);
                rig.run(5 * SECOND);
                rig.run(1800 * SECOND);
                let learnt = rig.stats();
                let target = rig.voice.target_frames() as u32;
                let (low, mean, high) = rig.run(1800 * SECOND);
                let stats = rig.stats();
                let case = format!("{period} frames under {callback}, {ppm} ppm");
                println!(
                    "{case}: {low}, mean {mean:.0}, {high} frames over target {target}; {stats:?}"
                );
                let wake = callback.div_ceil(BLOCK as u64) as u32;
                assert!(learnt.concealed <= wake, "{case}: {learnt:?}");
                assert_eq!(stats.concealed, learnt.concealed, "{case}: no gap once learnt");
                assert_eq!(rig.voice.target_frames() as u32, target, "{case}");
                assert!(target <= (BLOCK as u64 + period) as u32 + MARGIN as u32, "{case}");
                assert!(low >= BLOCK as u32, "{case}: {low}");
                let headroom = rig.voice.headroom_frames() as u32;
                assert!(high <= target + headroom, "{case}: {high}");
            }
        }
    }

    /// 50 ms without a delivery, then the input's backlog all at once or not at all. The stall
    /// itself is silence, and a block more when the input resumes just after a pull; within a
    /// second of it the delay is back under its bound, and the target has risen by at most a
    /// block for it.
    #[test]
    fn the_fill_recovers_after_a_stall() {
        for backlog in [true, false] {
            let mut rig = Rig::new(50, 128);
            rig.run(5 * SECOND);
            let before = rig.stats();
            let target = rig.voice.target_frames() as u32;
            rig.stall(19, backlog);
            rig.run(SECOND);
            let (low, _, high) = rig.run(10 * SECOND);
            let after = rig.stats();
            assert!(after.concealed - before.concealed <= 20, "{backlog}: {after:?}");
            assert!(rig.voice.target_frames() as u32 <= target + BLOCK as u32, "{backlog}");
            assert!(
                low >= BLOCK as u32 && high <= target + 2 * BLOCK as u32,
                "{backlog}: {low} {high}"
            );
            if backlog {
                assert!(after.resyncs > before.resyncs, "the backlog was dropped, not played");
            }
        }
    }

    /// A 4 kHz tone through an input 1000 ppm fast, so the voice drops about 48 samples a second,
    /// a period's worth in each move. A dropped or repeated sample would step by up to twice the tone's own largest
    /// change between samples; spliced across a block, no step is more than a hair over it.
    #[test]
    fn a_move_of_one_sample_leaves_no_step() {
        let mut rig = Rig::new(1000, 128);
        rig.hz = 4000.0;
        rig.run(2 * SECOND);
        let before = rig.stats();
        let slope = (0.5 * TAU * rig.hz / f64::from(RATE)) as f32;
        let mut last = rig.out[BLOCK - 1];
        let (mut steepest, mut energy) = (0.0f32, 0.0f64);
        for _ in 0..10 * SECOND {
            rig.block();
            for sample in rig.out {
                steepest = steepest.max((sample - last).abs());
                energy += f64::from(sample * sample);
                last = sample;
            }
        }
        let rms = (energy / (10 * SECOND * BLOCK as u64) as f64).sqrt();
        assert!((rms - 0.5 / 2f64.sqrt()).abs() < 0.002, "the tone came through whole: {rms}");
        let moved = rig.stats().resyncs - before.resyncs;
        assert!(moved >= 3, "{moved} moves");
        assert_eq!(rig.stats().concealed, 0);
        assert!(steepest <= slope * 1.02, "steepest {steepest} for a slope of {slope}");
    }

    /// What arrived while the mixer was armed is dropped, so the slot opens on the live input.
    #[test]
    fn the_first_pull_drops_what_arrived_while_armed() {
        let (mut producer, voice) = capture_voice();
        let mut voice = voice.for_input(128, 300, 128).expect("a period it takes");
        producer.push(&[1.0; 2000]);
        let mut out = [0.0; BLOCK];
        voice.pull(&mut out);
        assert_eq!(out, [0.0; BLOCK]);
        for _ in 0..WINDOW_BLOCKS + 2 {
            producer.push(&[0.5; BLOCK]);
            voice.pull(&mut out);
            assert_eq!(out, [0.0; BLOCK], "silent while it primes and settles");
        }
        producer.push(&[0.5; BLOCK]);
        voice.pull(&mut out);
        assert_eq!(out, [0.5; BLOCK], "the live frames alone");
        assert_eq!(voice.ring_frames(), BLOCK + MARGIN, "settled on the target");
        let wait = BLOCK + MARGIN + BLOCK - BLOCK / 2;
        assert_eq!(voice.latency_frames(), 300 + wait as u32, "the hardware and the wait");
    }

    /// Through the real audio thread and slot on the offline driver: a mic on the output's clock
    /// plays sample for sample, under the allocation guard. Each block arrives whole just before
    /// its pull, so the ring holds that block and the margin, and the output trails the newest
    /// block by the margin.
    #[test]
    fn a_capture_slot_plays_the_mic_exactly_through_the_graph() {
        let (mut producer, voice) = capture_voice();
        let graph = Box::new(Graph::new(vec![Slot::new(Box::new(
            voice.for_input(128, 0, 128).expect("a period it takes"),
        ))]));
        let (audio, mut controls) = AudioThread::new();
        controls.commands.push(Command::Swap { id: 1, graph }).ok().unwrap();
        controls.params.update(|p| p.slots[0].pan = -1.0);
        let mut driver = OfflineDriver::new(audio.into_render());
        let tone = |k: usize| (0.25 * (TAU * 440.0 * k as f64 / f64::from(RATE)).sin()) as f32;
        let mut block = vec![0.0; 2 * BLOCK];
        let mut left = Vec::new();
        let mut mic = [0.0; BLOCK];
        for b in 0..2 * SECOND as usize {
            for (k, sample) in mic.iter_mut().enumerate() {
                *sample = tone(b * BLOCK + k);
            }
            producer.push(&mic);
            assert_no_alloc(|| driver.pump_into(&mut block));
            left.extend(block.iter().step_by(2).copied());
        }
        let lag = MARGIN;
        let from = SECOND as usize * BLOCK;
        let worst = (from..left.len()).map(|i| (left[i] - tone(i - lag)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-6, "{worst}");
    }

    /// The latency a capture slot reports, which the guide clock and the echo canceller take as
    /// the mic's share of the path, against how long its samples really waited: within a step, and
    /// still across a minute, on every period and either clock, and under a 480-frame callback
    /// that pulls three or four blocks a wake.
    #[test]
    fn its_latency_is_the_wait_and_holds_still() {
        let cases = [(64u64, 128u64), (128, 128), (480, 128), (128, 480), (256, 480), (480, 480)];
        for (period, callback) in cases {
            for ppm in [0, 200, -200] {
                let mut rig = Rig::new(ppm, period).under(callback);
                rig.run(120 * SECOND);
                let reported = rig.voice.latency_frames();
                let before = rig.stats();
                let step = rig.voice.step() as i64;
                let mut worst = 0i64;
                for _ in 0..60 * SECOND {
                    rig.block();
                    assert_eq!(rig.voice.latency_frames(), reported, "{period}, {callback}, {ppm}");
                    worst = worst.max((i64::from(reported) - rig.waited as i64).abs());
                }
                let case = format!("{period} frames under {callback}, {ppm} ppm");
                println!("{case}: reported {reported}, off by {worst} at most");
                assert_eq!(rig.stats().concealed, before.concealed, "{case}");
                assert!(worst <= step, "{case}: off by {worst}, a step is {step}");
            }
        }
    }

    /// What the echo canceller follows of a capture slot, its latency and the frames it splices,
    /// moves with how long its samples really waited, but for the drift of the two clocks, which
    /// the canceller follows by itself. Over an hour at 200 ppm either way, with wakes up to 8
    /// frames early or late, through the gap a slower input meets, each move of the target and
    /// every splice, it strays from the true wait less the drift by 4 frames at most, a twelfth of
    /// a millisecond. Twenty minutes in, the input stops for 40 blocks and hands over all it
    /// missed: once the slot is back on its target, two seconds on, it is where it was, to the same
    /// 4 frames. Forty minutes in it stops again and those frames are gone, and the slot comes back
    /// on its target within a step and those 4 frames of where it was, as its latency does: 16
    /// frames for a 480-frame input under a block-a-wake output, a period under a longer callback.
    /// A pull the ring could not fill has no first sample to judge.
    #[test]
    fn its_latency_and_splices_move_with_the_wait_but_for_the_drift() {
        let cases = [(128, 128), (480, 128), (128, 480), (480, 480)];
        for (period, callback) in cases {
            for ppm in [0, 200, -200] {
                let mut rig = Rig::new(ppm, period).under(callback).jittered(8);
                rig.run(SECOND);
                // The true wait less the drift, against what the slot says: lowest and highest
                // before the first stall, between the two, and after the second
                let mut spans = [(f64::MAX, f64::MIN); 3];
                let (mut span, mut judged_from) = (0, 0);
                for block in 0..3600 * SECOND {
                    if block == 1200 * SECOND || block == 2400 * SECOND {
                        rig.stall(40, block == 1200 * SECOND);
                        (span, judged_from) = (span + 1, block + 2 * SECOND);
                    }
                    rig.block();
                    let full = rig.voice.played() && rig.voice.ring_frames() >= BLOCK;
                    if block < judged_from || !full {
                        continue;
                    }
                    let delay = rig.voice.delay();
                    let followed = i64::from(delay.latency_frames) + delay.spliced_frames;
                    let drift = (rig.blocks * BLOCK as u64) as f64 * ppm as f64 * 1e-6;
                    let off = rig.waited as f64 - followed as f64 - drift;
                    let (low, high) = &mut spans[span];
                    (*low, *high) = (low.min(off), high.max(off));
                }
                let case = format!("{period} frames under {callback}, {ppm} ppm");
                println!("{case}: the true wait less the drift, {spans:.1?} over it");
                let middle = |(low, high): (f64, f64)| (low + high) / 2.0;
                assert!(spans.iter().all(|(low, high)| high - low <= 4.0), "{case}: {spans:.1?}");
                assert!((middle(spans[1]) - middle(spans[0])).abs() <= 4.0, "{case}: a backlog");
                let step = rig.voice.step() as f64 + 4.0;
                assert!((middle(spans[2]) - middle(spans[1])).abs() <= step, "{case}: frames gone");
            }
        }
    }

    /// The ring is fixed at 4096 frames, so a slot refuses, when its input opens, an input period
    /// over 1024 frames or an output callback the ring could not hold beside it: a 2048-frame
    /// callback under any input of 1016 frames or less, and under a 128-frame mic up to 3712.
    #[test]
    fn a_period_past_the_ring_is_refused_when_the_input_opens() {
        let open = |period, callback| capture_voice().1.for_input(period, 0, callback).map(|_| ());
        assert_eq!(open(1024, 1024), Ok(()));
        assert_eq!(open(1016, 2048), Ok(()));
        assert_eq!(open(128, 3712), Ok(()));
        let refused = open(2048, 128).expect_err("an input period over 1024");
        assert!(refused.contains("2048 frames at a time"), "{refused}");
        let refused = open(1024, 2048).expect_err("a callback the ring cannot hold beside it");
        assert!(refused.contains("the output takes 2048"), "{refused}");
        let refused = open(128, 3713).expect_err("one block past what a 128-frame mic allows");
        assert!(refused.contains("1 to 3712"), "{refused}");
        assert!(open(0, 128).is_err() && open(128, 0).is_err());
    }
}
