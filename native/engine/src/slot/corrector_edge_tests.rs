//! A slot's corrector at its edges: every parameter at its limits, a ramp interrupted by another,
//! correction engaged on a voice already sounding, a reset mid-note, and packet loss with a pause
//! through the jitter buffer. Each renders through a real slot under the allocation guard.

use kernels::fixtures::{BUZZ, harmonic_voice, steepest_step};

use super::tests::{render, through_slot};
use super::*;
use crate::ingest::fixtures::L16Sender;
use crate::ingest::packet::RING_CAPACITY;
use crate::ingest::{Codec, JitterBuffer, PacketSource, packet_ring};
use crate::params::CorrectorParams;
use crate::slot::effects::Corrector;
use crate::slot::source::fixtures::Clip;

const ON: CorrectorParams = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 40.0 };

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |most, sample| most.max(sample.abs()))
}

/// A corrected voice is finite, no louder than the singer by more than a correction's crossfade
/// can add, and delayed inside the ceiling a host budgets with.
fn sound(output: &[f32], input: &[f32], delays: &[u32]) {
    assert!(output.iter().all(|sample| sample.is_finite()), "a sample is not finite");
    assert!(peak(output) <= 1.3 * peak(input), "{} against {}", peak(output), peak(input));
    let most = delays.iter().copied().max().unwrap_or(0);
    assert!(most <= Corrector::MAX_LATENCY_FRAMES, "the delay reached {most}");
}

// Every knob at an end of its range, and past it, as a host could send: strength 0, a hair over 0,
// 1 and 2, retune 0, 40 ms, 10 s and negative, and off
#[test]
fn every_corrector_parameter_at_its_limits() {
    let frames = 2 * RATE as usize;
    let input = harmonic_voice(frames, &BUZZ, |frame| match frame {
        0..48_000 => 112.6,
        48_000..62_400 => 0.0,
        _ => 225.2,
    });
    for (enabled, strength, retune_ms) in [
        (true, 0.0, 0.0),
        (true, 1e-6, 40.0),
        (true, 1.0, 0.0),
        (true, 1.0, 10_000.0),
        (true, 2.0, -5.0),
        (false, 1.0, 0.0),
    ] {
        let corrector = CorrectorParams { enabled, strength, retune_ms };
        let mut delays = Vec::new();
        let output = through_slot(input.clone(), |_| corrector, |delay| delays.push(delay));
        sound(&output, &input, &delays);
    }
}

// A host that flips correction on and off every 8 ms, faster than its 20 ms crossfade, gets a
// sound voice; once it settles on off, the slot is the input bit for bit and reads no delay
#[test]
fn a_correction_ramp_interrupted_by_another_settles_on_the_last() {
    let frames = 2 * RATE as usize;
    let input = harmonic_voice(frames, &BUZZ, |_| 112.6);
    let flipping = frames / BLOCK * 3 / 4;
    let mut delays = Vec::new();
    let output = through_slot(
        input.clone(),
        |block| match block {
            _ if block >= flipping => CorrectorParams { enabled: false, ..ON },
            _ if (block / 3) % 2 == 0 => ON,
            _ => CorrectorParams { strength: 0.0, ..ON },
        },
        |delay| delays.push(delay),
    );
    sound(&output, &input, &delays);
    assert_eq!(*delays.last().unwrap(), 0, "off reads no delay");
    for frame in (flipping + 20) * BLOCK..input.len() {
        assert_eq!(output[frame].to_bits(), input[frame].to_bits(), "off changed frame {frame}");
    }
}

// A singer who turns correction on mid-note hears it fade in over its crossfade: no step in the
// first 50 ms is steeper than the voice's own. The input ends inside a block, and the block that
// holds its last sample, padded with silence, is rendered too
#[test]
fn correction_engaged_on_a_voice_already_sounding_fades_in() {
    let input = harmonic_voice(12_345 + RATE as usize / 2, &BUZZ, |_| 225.2).split_off(12_345);
    let mut delays = Vec::new();
    let output = through_slot(input.clone(), |_| ON, |delay| delays.push(delay));
    sound(&output, &input, &delays);
    let onset = &output[..RATE as usize / 20];
    assert!(
        steepest_step(onset) <= 1.5 * steepest_step(&input),
        "a step of {}",
        steepest_step(onset)
    );
    assert!(delays.iter().any(|delay| *delay > 0), "correction engaged");
}

// A host's `resetSlot` mid-note, on the audio thread: the corrector starts again from the dry
// voice, the delay falls, and correction engages again
#[test]
fn a_reset_mid_note_starts_the_corrector_again() {
    let frames = 2 * RATE as usize;
    let input = harmonic_voice(frames, &BUZZ, |_| 112.6);
    let at = frames / BLOCK / 2;
    let mut slot = Slot::new(Box::new(Clip::new(input.clone())));
    let mut delays = Vec::new();
    let reset = |block: usize, slot: &mut Slot| {
        if block == at {
            slot.reset();
        }
    };
    let output = render(&mut slot, frames / BLOCK, |_| ON, reset, |delay| delays.push(delay));
    sound(&output, &input, &delays);
    assert!(delays[at] < delays[at - 1], "the reset left {} of {}", delays[at], delays[at - 1]);
    assert!(*delays.last().unwrap() > 0, "correction engaged again");
}

// One L16 packet in thirty lost, then 300 ms with none, as a phone's DTX sends nothing in a rest:
// the voice stays sound, and the silent pause lets the delay come down to a quarter period of the
// voice above the shifter's floor
#[test]
fn packet_loss_and_a_pause_through_the_jitter_buffer() {
    const PACKET: usize = 240;
    let frames = 2 * RATE as usize;
    let pause = 48_000..62_400;
    let input =
        harmonic_voice(frames, &BUZZ, |frame| if pause.contains(&frame) { 0.0 } else { 112.6 });
    let mut sender = L16Sender::new(PACKET, 1);
    // Built ahead, so the audio thread only pushes them; each is due once its first frame has
    // played
    let mut due: Vec<(usize, _)> = (0..frames / PACKET)
        .map(|k| (k * PACKET, sender.packet(&input[k * PACKET..(k + 1) * PACKET])))
        .filter(|(sent, _)| !(sent / PACKET + 1).is_multiple_of(30) && !pause.contains(sent))
        .collect();
    due.reverse();
    let (mut producer, consumer) = packet_ring(RING_CAPACITY);
    let jitter = JitterBuffer::new(RATE * 15 / 1000, Codec::L16);
    let mut slot = Slot::new(Box::new(PacketSource::new(consumer, jitter)));
    let mut delays = Vec::new();
    let deliver = |block: usize, _: &mut Slot| {
        while due.last().is_some_and(|(sent, _)| *sent <= block * BLOCK) {
            let (_, packet) = due.pop().unwrap();
            assert!(producer.push(&packet), "the ring has room");
        }
    };
    let output = render(&mut slot, frames / BLOCK, |_| ON, deliver, |delay| delays.push(delay));
    sound(&output, &input, &delays);
    let lowest = delays[pause.start / BLOCK..pause.end / BLOCK].iter().copied().min().unwrap();
    let resting = 5.0 + 0.25 * f64::from(RATE) / 112.6;
    assert!(f64::from(lowest) <= resting + 2.0, "the pause left {lowest}, resting {resting:.0}");
}
