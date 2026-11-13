//! One mic input: its source, the effect chain, the meter, then gain and pan into the stereo
//! bus. The order is docs/design.md section 4; the ramps are one block.

pub mod corrector_delay;
pub mod cue;
pub mod echo_report;
#[cfg(test)]
mod echo_tests;
pub mod effects;
pub mod guide;
pub mod meter;
pub mod shared;
pub mod source;

use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;

use crate::block::{BLOCK, RATE, Stereo};
use crate::command::{Event, EventProducer};
use crate::params::SlotParams;
use corrector_delay::CorrectorDelay;
use echo_report::EchoReport;
use effects::{Compressor, Corrector, EchoCanceller, Gate, HighPass, HowlGuard, Reverb, Voice};
use guide::Guide;
use kernels::EchoHistory;
use meter::Meter;
use shared::Shared;
use source::Source;

pub struct Slot {
    source: Box<dyn Source>,
    high_pass: HighPass,
    gate: Gate,
    guide: Option<Box<Guide>>,
    echo_canceller: EchoCanceller,
    howl_guard: HowlGuard,
    echo_report: Arc<EchoReport>,
    /// What this slot put into the master, folded to mono, for the canceller to adapt without.
    own: EchoHistory,
    corrector: Corrector,
    corrector_delay: Arc<CorrectorDelay>,
    reverb: Reverb,
    voice: Voice,
    meter: Meter,
    compressor: Compressor,
    /// Left and right gains the last block ended on; `None` before the first block, which starts
    /// on its target instead of ramping up from silence.
    gains: Option<[f32; 2]>,
    input: [f32; BLOCK],
}

impl Slot {
    pub fn new(source: Box<dyn Source>) -> Self {
        Self {
            source,
            high_pass: HighPass::new(),
            gate: Gate::new(),
            guide: None,
            echo_canceller: EchoCanceller::new(),
            howl_guard: HowlGuard::new(),
            echo_report: Arc::default(),
            own: EchoHistory::new(),
            corrector: Corrector::new(),
            corrector_delay: Arc::default(),
            reverb: Reverb::new(),
            voice: Voice::new(),
            meter: Meter::new(),
            compressor: Compressor::new(),
            gains: None,
            input: [0.0; BLOCK],
        }
    }

    /// How far the corrector makes the voice trail what `source` handed over, now; 0 with
    /// correction off.
    pub fn corrector_latency_frames(&self) -> usize {
        self.corrector.latency_frames()
    }

    /// The same delay for the control thread, once the graph has moved to the audio thread.
    pub fn corrector_delay(&self) -> Arc<CorrectorDelay> {
        Arc::clone(&self.corrector_delay)
    }

    /// What the echo stages have found, for the control thread.
    pub fn echo_report(&self) -> Arc<EchoReport> {
        Arc::clone(&self.echo_report)
    }

    /// Pulls one block, takes the room's echo of the master out of it, guards it against a howl,
    /// cleans it, corrects it toward the guide at the cue, meters it, compresses it, voices it, adds
    /// reverb, and adds it to `out` at the ramped gain, silent while another slot is soloed and this
    /// one is not.
    pub fn render(
        &mut self,
        index: u8,
        params: &SlotParams,
        shared: Shared<'_>,
        out: &mut Stereo<'_>,
        events: &mut EventProducer,
    ) {
        self.source.pull(&mut self.input);
        self.echo_canceller.process(
            &params.effects.echo_cancel,
            shared.history,
            &self.own,
            self.source.latency_frames(),
            &mut self.input,
        );
        self.howl_guard.process(&params.effects.howl_guard, &mut self.input);
        self.echo_report.publish(self.echo_canceller.findings());
        self.echo_report.publish_howl_catches(self.howl_guard.catches());
        self.high_pass.process(&params.effects.cleanup, &mut self.input);
        self.gate.process(&params.effects.cleanup, &mut self.input);
        let guide_offset =
            (f64::from(params.guide_offset_ms) * f64::from(RATE) / 1000.0).round() as i64;
        let frame = shared.cue.frame - i64::from(self.source.latency_frames()) + guide_offset;
        self.corrector.process(
            &params.effects.corrector,
            self.guide.as_deref(),
            frame,
            shared.cue.semitones,
            &mut self.input,
            |reading| {
                events.push(Event::Pitch {
                    slot: index,
                    position_ms: reading.position_ms,
                    hz: reading.hz,
                    confidence: reading.confidence,
                    target_midi: reading.target_midi,
                    error_semitones: reading.error_semitones,
                });
            },
        );
        self.corrector_delay.publish(self.corrector.latency_frames());
        // The meter reads the corrected voice ahead of the compressor, the voice and the reverb:
        // it is the singing
        for sample in self.input {
            if let Some(reading) = self.meter.push_frame(sample * sample, sample.abs()) {
                events.push(Event::Level {
                    slot: index,
                    rms_db: reading.rms_db,
                    peak_db: reading.peak_db,
                });
            }
        }
        self.compressor.process(&params.effects.cleanup, &mut self.input);
        self.voice.process(&params.effects.voice, &mut self.input);
        self.reverb.process(&params.effects.reverb, &mut self.input);

        let target = target_gains(params, shared.any_solo);
        let [mut left, mut right] = self.gains.unwrap_or(target);
        let step = [(target[0] - left) / BLOCK as f32, (target[1] - right) / BLOCK as f32];
        let (mut own_left, mut own_right) = ([0.0; BLOCK], [0.0; BLOCK]);
        for (((sample, l), r), (own_l, own_r)) in self
            .input
            .iter()
            .zip(out.left.iter_mut())
            .zip(out.right.iter_mut())
            .zip(own_left.iter_mut().zip(own_right.iter_mut()))
        {
            left += step[0];
            right += step[1];
            (*own_l, *own_r) = (sample * left, sample * right);
            *l += *own_l;
            *r += *own_r;
        }
        self.own.push(&own_left, &own_right);
        self.gains = Some(target);
    }

    /// Installs a guide and returns the one it displaces, for the retired ring.
    pub fn set_guide(&mut self, guide: Option<Box<Guide>>) -> Option<Box<Guide>> {
        std::mem::replace(&mut self.guide, guide)
    }

    pub fn guide(&self) -> Option<&Guide> {
        self.guide.as_deref()
    }

    /// The output's callback now takes `callback_frames` at a wake.
    pub fn output_moved(&mut self, callback_frames: u32) {
        self.source.output_moved(callback_frames);
    }

    /// A new singer: the source forgets its stream, the corrector and the voice what they heard,
    /// and the gate and compressor the level of the phone before, and the echo stages the path,
    /// since a new sender is a new one. A source that keeps its sender, a capture slot's mic, keeps
    /// its stream and the path, and only the corrector starts again.
    pub fn reset(&mut self) {
        self.source.reset();
        self.corrector.reset();
        if self.source.keeps_its_sender() {
            return;
        }
        self.forget_the_path();
    }

    /// A move to another output: the source starts on the new clock, and every stage forgets the
    /// path through the old output and what it heard.
    pub fn restart(&mut self) {
        self.source.restart();
        self.corrector.reset();
        self.forget_the_path();
    }

    fn forget_the_path(&mut self) {
        self.echo_canceller.reset();
        self.howl_guard.reset();
        self.voice.reset();
        self.gate = Gate::new();
        self.compressor = Compressor::new();
    }
}

/// Mute wins over solo; an unsoloed slot is silent while any other is soloed. Then the
/// constant-power pan law spreads what is left across the bus.
fn target_gains(params: &SlotParams, any_solo: bool) -> [f32; 2] {
    let audible = !params.mute && (params.solo || !any_solo);
    let gain = if audible { params.gain } else { 0.0 };
    let angle = (params.pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    [gain * angle.cos(), gain * angle.sin()]
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use assert_no_alloc::assert_no_alloc;
    use kernels::YinDetector;

    use super::*;
    use crate::backing::{Backing, Planar, Stems, Transport};
    use crate::command::event_ring;
    use crate::driver::OfflineDriver;
    use crate::graph::Graph;
    use crate::ingest::fixtures::L16Sender;
    use crate::ingest::packet::RING_CAPACITY;
    use crate::ingest::{Codec, JitterBuffer, PacketSource, packet_ring};
    use crate::params::{CleanupParams, CorrectorParams, EffectParams, Params};
    use crate::slot::cue::Cue;
    use crate::slot::guide::Note;

    const CORRECTING: CorrectorParams =
        CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };

    /// Twelve harmonics at `hz`, the buzz of a sung vowel.
    fn voice(hz: f64, frame: usize) -> f32 {
        let t = frame as f64 / f64::from(RATE);
        let sample: f64 = (1..=12).map(|h| (TAU * hz * h as f64 * t).sin() / h as f64).sum();
        (0.25 * sample) as f32
    }

    /// Hands over `samples`, then silence.
    struct Clip {
        samples: Vec<f32>,
        at: usize,
    }

    impl Source for Clip {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
                self.at += 1;
            }
        }
    }

    /// `samples` through a new slot with `corrector`, hard left so the left channel is its mono
    /// output, under the allocation guard. After each block `after_block` hears what stats reads.
    fn through_slot(
        samples: Vec<f32>,
        corrector: CorrectorParams,
        mut after_block: impl FnMut(u32),
    ) -> Vec<f32> {
        let frames = samples.len();
        let mut slot = Slot::new(Box::new(Clip { samples, at: 0 }));
        let delay = slot.corrector_delay();
        let params = SlotParams {
            pan: -1.0,
            effects: EffectParams { corrector, ..EffectParams::default() },
            ..SlotParams::default()
        };
        let (mut events, _drain) = event_ring();
        let history = EchoHistory::new();
        let mut output = Vec::with_capacity(frames);
        for _ in 0..frames.div_ceil(BLOCK) {
            let mut left = [0.0; BLOCK];
            let mut right = [0.0; BLOCK];
            assert_no_alloc(|| {
                let mut out = Stereo { left: &mut left, right: &mut right };
                let shared = Shared { any_solo: false, cue: Cue::default(), history: &history };
                slot.render(0, &params, shared, &mut out, &mut events);
            });
            output.extend_from_slice(&left);
            after_block(delay.frames());
        }
        output
    }

    // Off is bit for bit what the source handed over, with no delay, whichever way a host says
    // off: a singer who did not ask for correction pays nothing for it.
    #[test]
    fn correction_off_hands_the_voice_over_bit_for_bit() {
        let off = [
            CorrectorParams::default(),
            CorrectorParams { enabled: false, strength: 1.0, retune_ms: 0.0 },
            CorrectorParams { enabled: true, strength: 0.0, retune_ms: 40.0 },
        ];
        let input: Vec<f32> = (0..RATE as usize).map(|frame| voice(226.0, frame)).collect();
        for corrector in off {
            let output = through_slot(input.clone(), corrector, |delay| assert_eq!(delay, 0));
            for (frame, (out, sample)) in output.iter().zip(&input).enumerate() {
                assert_eq!(out.to_bits(), sample.to_bits(), "{corrector:?} changed frame {frame}");
            }
        }
    }

    // Whatever a voice leaves the delay at, the audio is that late: a click in the rest after a
    // phrase arrives where stats said it would, to within the one frame the shifter's fractional
    // read point can round to. The phrase lengths leave the sawtooth at different heights.
    #[test]
    fn stats_reads_the_delay_the_audio_has() {
        const REST: usize = RATE as usize / 5;
        let mut heights = Vec::new();
        for phrase in (RATE as usize / 5..RATE as usize / 2).step_by(RATE as usize / 40) {
            // 40 cents above A2, so the shift keeps the read point moving
            let mut input: Vec<f32> = (0..phrase).map(|frame| voice(112.6, frame)).collect();
            input.resize(phrase + REST, 0.0);
            let click = input.len();
            input.push(0.5);
            input.resize(click + BLOCK * 16, 0.0);

            let mut reported = Vec::new();
            let output = through_slot(input, CORRECTING, |delay| reported.push(delay));
            let before_click = reported[click / BLOCK - 1] as i64;
            let arrived = (click..output.len())
                .max_by(|a, b| output[*a].abs().total_cmp(&output[*b].abs()))
                .expect("frames after the click") as i64;
            let late = arrived - click as i64;
            assert!(
                (late - before_click).abs() <= 1,
                "a {phrase} frame phrase: click {late} frames late, stats read {before_click}"
            );
            heights.push(late);
        }
        heights.sort_unstable();
        assert!(heights[heights.len() - 1] - heights[0] > 48, "one height: {heights:?}");
    }

    // On the offline driver and through a real source: L16 packets into a jitter buffer, then the
    // slot. The voice sits 30 cents above A3, nearer A than B flat, so landing on the guide's
    // B flat shows the guide led it. 10 cents is the tolerance the corrector's own tests allow for
    // the detector's error.
    #[test]
    fn a_tone_from_its_source_lands_on_its_guide_note_and_stats_reads_its_delay() {
        const B_FLAT_3: u8 = 58;
        const PLAYOUT: u32 = RATE * 15 / 1000;
        const PACKET: usize = 240;
        let frames = 2 * RATE as usize;
        let a3_sharp = 220.0 * 2f64.powf(30.0 / 1200.0);

        let (mut producer, consumer) = packet_ring(RING_CAPACITY);
        let jitter = JitterBuffer::new(PLAYOUT, Codec::L16);
        let mut slot = Slot::new(Box::new(PacketSource::new(consumer, jitter)));
        let delay = slot.corrector_delay();
        slot.set_guide(Some(Box::new(Guide {
            notes: vec![Note { midi: B_FLAT_3, start_frame: 0, end_frame: 2 * frames as u64 }],
            key_mask: 0,
        })));
        let mut graph = Graph::new(vec![slot]);
        let (mut events, _drain) = event_ring();
        let mut backing = Planar::default();
        backing.pad_to(2 * frames);
        graph.set_backing(Backing::new(Stems::new(backing, None), 0));
        graph.steer(1, Transport::Start, &mut events);
        let mut params = Params::default();
        params.slots[0].pan = -1.0;
        params.slots[0].effects.corrector = CorrectorParams { retune_ms: 40.0, ..CORRECTING };
        let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
            graph.render(&params, out, &mut events);
        }));

        let input: Vec<f32> = (0..frames).map(|frame| voice(a3_sharp, frame)).collect();
        let mut sender = L16Sender::new(PACKET, 1);
        let mut left = Vec::with_capacity(frames);
        let (mut sent, mut most) = (0, 0);
        let mut interleaved = [0.0; 2 * BLOCK];
        while left.len() < frames {
            // A packet reaches the ring once the output clock has passed its first frame
            if sent <= left.len() && sent + PACKET <= frames {
                let packet = sender.packet(&input[sent..sent + PACKET]);
                assert!(producer.push(&packet), "the ring has room");
                sent += PACKET;
            }
            assert_no_alloc(|| driver.pump_into(&mut interleaved));
            left.extend(interleaved.iter().step_by(2));
            most = most.max(delay.frames());
        }

        // The end of the take, long after the jitter buffer and the retune have settled
        let window = 2048;
        let read = YinDetector::new(RATE, window).analyze(&left[left.len() - window..]);
        let cents = 1200.0 * (f64::from(read.hz) / (220.0 * 2f64.powf(100.0 / 1200.0))).log2();
        assert!(cents.abs() < 10.0, "came out {cents:+.1} cents from B flat 3");
        // The shifter's floor, and at most an eighth over one period of the voice
        let bound = 64 + (1.125 * f64::from(RATE) / a3_sharp) as u32;
        assert!(most > 0 && most <= bound, "delay reached {most} frames, bound {bound}");
    }

    /// What a tuning panel reads from one `Event::Pitch`: where, the target, and the error.
    #[derive(Debug)]
    struct Shown {
        position_ms: f64,
        target_midi: Option<i16>,
        error_semitones: f32,
    }

    /// `input` through a one-slot graph following `guide` on the offline driver, hard left, under
    /// the allocation guard. The backing plays silence from frame 0, so the guide's clock is the
    /// output's. Returns the left channel and what every pitch event showed.
    fn on_the_offline_driver(
        input: &[f32],
        guide: Guide,
        corrector: CorrectorParams,
    ) -> (Vec<f32>, Vec<Shown>) {
        let mut slot = Slot::new(Box::new(Clip { samples: input.to_vec(), at: 0 }));
        slot.set_guide(Some(Box::new(guide)));
        let mut graph = Graph::new(vec![slot]);
        let (mut events, mut drain) = event_ring();
        let mut backing = Planar::default();
        backing.pad_to(2 * input.len());
        graph.set_backing(Backing::new(Stems::new(backing, None), 0));
        graph.steer(1, Transport::Start, &mut events);
        let mut params = Params::default();
        params.slots[0].pan = -1.0;
        params.slots[0].effects.corrector = corrector;
        let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
            graph.render(&params, out, &mut events);
        }));

        let mut left = Vec::with_capacity(input.len());
        let mut interleaved = [0.0; 2 * BLOCK];
        while left.len() < input.len() {
            assert_no_alloc(|| driver.pump_into(&mut interleaved));
            left.extend(interleaved.iter().step_by(2));
        }
        let shown = std::iter::from_fn(|| drain.pop())
            .filter_map(|event| match event {
                Event::Pitch { position_ms, target_midi, error_semitones, .. } => {
                    Some(Shown { position_ms, target_midi, error_semitones })
                }
                _ => None,
            })
            .collect();
        (left, shown)
    }

    /// C major's classes, with C in bit 0.
    const C_MAJOR: u16 = 0b1010_1011_0101;
    const B_FLAT_3: u8 = 58;
    /// The guide's B flat sounds from half a second to one second.
    const NOTE_MS: std::ops::Range<f64> = 500.0..1000.0;
    /// A reading's window reaches 21 ms either side of its centre, and the target it carries is
    /// the one set as it closed: this far from an edge, a reading has heard one target only.
    const EDGE_MARGIN_MS: f64 = 50.0;
    /// The corrector's own tests allow 5 cents for the detector's error.
    const ERROR_TOLERANCE: f32 = 0.05;

    fn b_flat_in_c_major() -> Guide {
        let frame = |ms: f64| (ms * f64::from(RATE) / 1000.0) as u64;
        Guide {
            notes: vec![Note {
                midi: B_FLAT_3,
                start_frame: frame(NOTE_MS.start),
                end_frame: frame(NOTE_MS.end),
            }],
            key_mask: C_MAJOR,
        }
    }

    /// 30 cents above A3: in C major's key the nearest class is A, 0.3 below, and against the
    /// guide's B flat the voice is 0.7 flat.
    fn a3_sharp(seconds: f64) -> Vec<f32> {
        let hz = 220.0 * 2f64.powf(30.0 / 1200.0);
        (0..(seconds * f64::from(RATE)) as usize).map(|frame| voice(hz, frame)).collect()
    }

    // The tuning panel's reading: during the note it names the note and how far flat the voice
    // is; around it, no note, and the distance to the key's nearest class
    #[test]
    fn a_reading_shows_the_guide_note_and_the_error_from_it() {
        let (_, shown) = on_the_offline_driver(&a3_sharp(1.5), b_flat_in_c_major(), CORRECTING);
        let settled = shown.iter().filter(|s| s.position_ms > EDGE_MARGIN_MS);
        let away_from_edges = settled.filter(|s| {
            (s.position_ms - NOTE_MS.start).abs() > EDGE_MARGIN_MS
                && (s.position_ms - NOTE_MS.end).abs() > EDGE_MARGIN_MS
        });
        let (mut in_note, mut in_key) = (0, 0);
        for reading in away_from_edges {
            let (target, error) = if NOTE_MS.contains(&reading.position_ms) {
                in_note += 1;
                (Some(i16::from(B_FLAT_3)), 0.7)
            } else {
                in_key += 1;
                (None, -0.3)
            };
            assert_eq!(reading.target_midi, target, "{reading:?}");
            assert!((reading.error_semitones - error).abs() < ERROR_TOLERANCE, "{reading:?}");
        }
        assert!(in_note >= 35 && in_key >= 75, "{in_note} readings in the note, {in_key} in key");
    }

    // Off, nothing pulls: no target, no error, and the voice is what the source handed over
    #[test]
    fn correction_off_shows_no_target_and_leaves_the_voice_alone() {
        let input = a3_sharp(1.5);
        let off = CorrectorParams { enabled: false, ..CORRECTING };
        let (left, shown) = on_the_offline_driver(&input, b_flat_in_c_major(), off);
        assert_eq!(shown.len(), 150, "readings every 10 ms");
        for reading in shown {
            assert_eq!((reading.target_midi, reading.error_semitones), (None, 0.0), "{reading:?}");
        }
        for (frame, (out, sample)) in left.iter().zip(&input).enumerate() {
            assert_eq!(out.to_bits(), sample.to_bits(), "frame {frame} changed");
        }
    }

    /// A click every 100 ms over silence.
    struct Clicks {
        at: usize,
    }

    impl Source for Clicks {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = if self.at % 4_800 == 1_234 { 0.5 } else { 0.0 };
                self.at += 1;
            }
        }
    }

    // The done-when: with the high-pass, gate and compressor on, every click comes out loudest on
    // the frame it went in, so the stages buffer nothing on the mic path
    #[test]
    fn cleaning_the_voice_buffers_nothing() {
        let params = SlotParams {
            pan: -1.0,
            effects: EffectParams {
                cleanup: CleanupParams { enabled: true },
                ..EffectParams::default()
            },
            ..SlotParams::default()
        };
        let mut slot = Slot::new(Box::new(Clicks { at: 0 }));
        let (mut events, _drain) = event_ring();
        let history = EchoHistory::new();
        let mut output = Vec::with_capacity(RATE as usize);
        for _ in 0..RATE as usize / BLOCK {
            let mut left = [0.0; BLOCK];
            let mut right = [0.0; BLOCK];
            assert_no_alloc(|| {
                let mut out = Stereo { left: &mut left, right: &mut right };
                let shared = Shared { any_solo: false, cue: Cue::default(), history: &history };
                slot.render(0, &params, shared, &mut out, &mut events);
            });
            output.extend_from_slice(&left);
        }
        for (period, window) in output.as_chunks::<4_800>().0.iter().enumerate() {
            let loudest =
                (0..window.len()).max_by(|a, b| window[*a].abs().total_cmp(&window[*b].abs()));
            assert_eq!(loudest, Some(1_234), "click {period} moved");
            assert!(window[..1_234].iter().all(|s| s.abs() < 1e-6), "click {period} came early");
        }
    }

    #[test]
    fn the_pan_law_keeps_power_constant() {
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let [l, r] = target_gains(&SlotParams { pan, ..SlotParams::default() }, false);
            assert!((l * l + r * r - 1.0).abs() < 1e-6, "pan {pan}: {l} {r}");
        }
        let hard_left = target_gains(&SlotParams { pan: -1.0, ..SlotParams::default() }, false);
        assert!((hard_left[0] - 1.0).abs() < 1e-6 && hard_left[1].abs() < 1e-6);
        let centre = target_gains(&SlotParams::default(), false);
        assert!((centre[0] - centre[1]).abs() < 1e-6);
    }

    #[test]
    fn mute_beats_solo_and_solo_silences_the_others() {
        let soloed = SlotParams { solo: true, ..SlotParams::default() };
        let plain = SlotParams::default();
        let muted_solo = SlotParams { mute: true, solo: true, ..SlotParams::default() };
        assert!(target_gains(&soloed, true)[0] > 0.0);
        assert_eq!(target_gains(&plain, true), [0.0, 0.0]);
        assert_eq!(target_gains(&muted_solo, true), [0.0, 0.0]);
        assert!(target_gains(&plain, false)[0] > 0.0);
    }
}
