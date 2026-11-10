use std::f64::consts::PI;

use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::test_support::{RATE, cents_between};

const SR: f64 = RATE as f64;
const BLOCK: usize = 128;
/// The done-when's bar for where a note lands.
const PITCH_TOLERANCE_CENTS: f64 = 5.0;
/// Long enough for the detector to have heard the voice, and the delay to have settled on it.
const SETTLED_SECONDS: f64 = 0.25;
/// How much of a voice's harmonic energy, here 20 dB down, says its grid is an octave out.
const OCTAVE_FLOOR: f64 = 0.01;
/// A vowel near /a/: where the formants sit, each with its bandwidth.
const FORMANTS: [(f64, f64); 4] =
    [(700.0, 90.0), (1220.0, 110.0), (2600.0, 170.0), (3500.0, 250.0)];
/// Above this the voice has too few harmonics under its first formant to say where it is.
const WELL_SAMPLED_HZ: f64 = 200.0;

/// A vocal tract as a transfer function, magnitude and phase: resonators in series over a
/// source falling at 6 dB an octave. Its phases are what a real voice's minimum-phase tract gives,
/// which is what a pitch-synchronous splice has to line up.
fn tract(hz: f64) -> (f64, f64) {
    let w = 2.0 * PI * hz;
    let (mut magnitude, mut phase) = (1.0 / (1.0 + (hz / 100.0).powi(2)).sqrt(), 0.0);
    for (centre, bandwidth) in FORMANTS {
        let (sigma, omega) = (PI * bandwidth, 2.0 * PI * centre);
        let pole_power = sigma * sigma + omega * omega;
        let (real, imaginary) = (pole_power - w * w, 2.0 * sigma * w);
        magnitude *= pole_power / real.hypot(imaginary);
        phase -= imaginary.atan2(real);
    }
    (magnitude, phase)
}

/// A voice singing the pitch contour `hz` frame by frame, its tract's formants scaled by `factor`.
fn sung(hz: &[f64], factor: f64) -> Vec<f32> {
    let top = hz.iter().copied().fold(f64::MAX, f64::min);
    let harmonics = (10_000.0 / top) as usize;
    let mut signal = vec![0.0; hz.len()];
    // The tract's response changes slowly next to a sample: reading it every millisecond is plenty
    let mut response = vec![(0.0, 0.0); harmonics];
    let mut phase = 0.0;
    for (i, (out, frequency)) in signal.iter_mut().zip(hz).enumerate() {
        if i % 48 == 0 {
            for (h, slot) in response.iter_mut().enumerate() {
                *slot = tract((h + 1) as f64 * frequency / factor);
            }
        }
        phase += 2.0 * PI * frequency / SR;
        for (h, (magnitude, shift)) in response.iter().enumerate() {
            let harmonic = (h + 1) as f64;
            if harmonic * frequency < 0.45 * SR {
                *out += magnitude * (harmonic * phase + shift).sin();
            }
        }
    }
    let peak = signal.iter().fold(0.0_f64, |top, s| top.max(s.abs()));
    signal.iter().map(|s| (0.25 * s / peak) as f32).collect()
}

fn vowel(hz: f64, seconds: f64) -> Vec<f32> {
    sung(&vec![hz; (seconds * SR) as usize], 1.0)
}

/// White noise that always comes out the same.
fn noise(samples: usize, level: f32) -> Vec<f32> {
    let mut seed = 7_u32;
    (0..samples)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed as f32 / 4_294_967_296.0 * 2.0 - 1.0) * level
        })
        .collect()
}

/// A hiss above 3 kHz, the shape of an /s/: white noise less its low end, by a two-point difference
/// run through twice.
fn hiss(samples: usize, level: f32) -> Vec<f32> {
    let white = noise(samples, level);
    let once: Vec<f32> = white.windows(2).map(|w| w[1] - w[0]).collect();
    let mut twice: Vec<f32> = once.windows(2).map(|w| 0.5 * (w[1] - w[0])).collect();
    twice.resize(samples, 0.0);
    twice
}

struct Run {
    output: Vec<f32>,
    /// The most `latency_samples` read once the voice had been heard, in samples.
    most_latency: usize,
}

/// `input` through `shifter` in the engine's blocks, every call under the allocation guard.
fn shifted_by(shifter: &mut VoiceShifter, input: &[f32]) -> Run {
    let mut output = vec![0.0; input.len()];
    let settled_block = (SETTLED_SECONDS * SR) as usize / BLOCK;
    let mut most_latency = 0;
    for (index, (input, output)) in input.chunks(BLOCK).zip(output.chunks_mut(BLOCK)).enumerate() {
        assert_no_alloc(|| shifter.process(input, output));
        if index >= settled_block {
            most_latency = most_latency.max(shifter.latency_samples());
        }
    }
    Run { output, most_latency }
}

fn shifted(input: &[f32], semitones: f32, formant_factor: f32) -> Run {
    let mut shifter = VoiceShifter::new(RATE);
    shifter.set(semitones, formant_factor);
    shifted_by(&mut shifter, input)
}

/// A window's sum of a sine and cosine at `hz`, so its amplitude.
fn amplitude_at(windowed: &[f64], window_sum: f64, hz: f64) -> f64 {
    let step = 2.0 * PI * hz / SR;
    let (mut real, mut imaginary) = (0.0, 0.0);
    let (rotate_real, rotate_imaginary) = (step.cos(), step.sin());
    let (mut c, mut s) = (1.0, 0.0);
    for x in windowed {
        real += x * c;
        imaginary += x * s;
        (c, s) = (c * rotate_real - s * rotate_imaginary, c * rotate_imaginary + s * rotate_real);
    }
    2.0 * real.hypot(imaginary) / window_sum
}

fn hann_windowed(stretch: &[f32]) -> (Vec<f64>, f64) {
    let count = stretch.len() as f64;
    let window = |i: usize| 0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / count).cos();
    let sum = (0..stretch.len()).map(window).sum();
    (stretch.iter().enumerate().map(|(i, x)| f64::from(*x) * window(i)).collect(), sum)
}

/// The pitch of a stretch of voice, finer than YIN's few cents: the frequency near `guess` at which
/// the first dozen harmonics hold the most energy. Reading all of them at once is what makes it
/// the pitch and not whichever harmonic is loudest.
fn pitch_near(stretch: &[f32], guess: f64) -> f64 {
    let (windowed, sum) = hann_windowed(stretch);
    let energy = |hz: f64| -> f64 {
        (1..=12)
            .map(|h| h as f64 * hz)
            .take_while(|harmonic| *harmonic < 6_000.0)
            .map(|harmonic| amplitude_at(&windowed, sum, harmonic).powi(2))
            .sum()
    };
    let mut best = 0.0;
    for (step, span) in [(4.0, 15), (1.0, 4), (0.2, 5), (0.05, 4)] {
        let cents = |i: i32| best + step * f64::from(i);
        let score = |i: i32| energy(guess * 2f64.powf(cents(i) / 1200.0));
        let top = (-span..=span).max_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap_or(0);
        best = cents(top);
    }
    let hz = guess * 2f64.powf(best / 1200.0);
    assert_is_the_fundamental(&windowed, sum, hz);
    hz
}

/// `pitch_near` sums a grid of harmonics, and a voice an octave away also lands on that grid, so it
/// checks the grid is the right one. A voice an octave up leaves the odd harmonics empty; one an
/// octave down fills the gaps between them.
fn assert_is_the_fundamental(windowed: &[f64], sum: f64, hz: f64) {
    let energy = |multiples: &[f64]| -> f64 {
        multiples
            .iter()
            .map(|multiple| multiple * hz)
            .take_while(|frequency| *frequency < 6_000.0)
            .map(|frequency| amplitude_at(windowed, sum, frequency).powi(2))
            .sum()
    };
    let odd = energy(&[1.0, 3.0, 5.0, 7.0, 9.0, 11.0]);
    let even = energy(&[2.0, 4.0, 6.0, 8.0, 10.0, 12.0]);
    let between = energy(&[0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5, 11.5]);
    let total = odd + even;
    assert!(
        odd > OCTAVE_FLOOR * total,
        "an octave high of {hz:.1} Hz: odd {odd:.2e}, all {total:.2e}"
    );
    assert!(
        between < OCTAVE_FLOOR * total,
        "an octave low of {hz:.1} Hz: between {between:.2e}, all {total:.2e}"
    );
}

/// How far the voice's harmonics sit from the envelope asked for, as the RMS in dB over the
/// harmonics within 30 dB of the loudest and under 5 kHz, and where the two put their highest
/// peak above 300 Hz. Level is not in it: the kernel matches level, the envelope is the question.
struct EnvelopeError {
    rms_db: f64,
    peak_hz: f64,
    asked_peak_hz: f64,
    spacing_hz: f64,
}

fn envelope_error(stretch: &[f32], hz: f64, factor: f64) -> EnvelopeError {
    let (windowed, sum) = hann_windowed(stretch);
    let frequencies: Vec<f64> =
        (1..).map(|h| h as f64 * hz).take_while(|frequency| *frequency < 5_000.0).collect();
    let measured: Vec<f64> =
        frequencies.iter().map(|f| amplitude_at(&windowed, sum, *f).max(1e-9)).collect();
    let asked: Vec<f64> = frequencies.iter().map(|f| tract(f / factor).0).collect();
    let power = |set: &[f64]| set.iter().map(|a| a * a).sum::<f64>();
    let scale = (power(&measured) / power(&asked)).sqrt();
    let loudest = asked.iter().copied().fold(0.0, f64::max);
    let errors: Vec<f64> = measured
        .iter()
        .zip(&asked)
        .filter(|(_, a)| **a >= loudest * 0.0316)
        .map(|(m, a)| 20.0 * (m / (a * scale)).log10())
        .collect();
    let rms_db = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64).sqrt();
    EnvelopeError {
        rms_db,
        peak_hz: highest_peak(&frequencies, &measured),
        asked_peak_hz: highest_peak(&frequencies, &asked),
        spacing_hz: hz,
    }
}

/// The loudest harmonic above 300 Hz, moved by a parabola through its neighbours in dB against log
/// frequency, since the peak of a formant rarely sits on a harmonic.
fn highest_peak(frequencies: &[f64], amplitudes: &[f64]) -> f64 {
    let top = (0..frequencies.len())
        .filter(|i| frequencies[*i] >= 300.0)
        .max_by(|a, b| amplitudes[*a].total_cmp(&amplitudes[*b]))
        .unwrap_or(0);
    if top == 0 || top + 1 >= frequencies.len() {
        return frequencies[top];
    }
    let db = |i: usize| 20.0 * amplitudes[i].log10();
    let (before, at, after) = (db(top - 1), db(top), db(top + 1));
    let curvature = before - 2.0 * at + after;
    if curvature >= 0.0 {
        return frequencies[top];
    }
    let offset = (0.5 * (before - after) / curvature).clamp(-1.0, 1.0);
    let spacing = (frequencies[top + 1] / frequencies[top]).ln();
    frequencies[top] * (offset * spacing).exp()
}

fn tail(signal: &[f32], from_seconds: f64) -> &[f32] {
    &signal[(from_seconds * SR) as usize..]
}

fn rms(signal: &[f32]) -> f64 {
    let sum: f64 = signal.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / signal.len() as f64).sqrt()
}

// The issue's done-when, one way and the other. Peak tolerance: 10 percent on a voice with the
// harmonics to show its formants, about a semitone and a half at a formant, and one harmonic's
// spacing on a voice that has few, since nothing can place a peak finer than its harmonics
// sample it. RMS tolerance: 4 dB on a well-sampled voice, against the 7 to 12 dB a resampled
// voice's formants are out by; the high voices are held only to the peak.
#[test]
fn six_semitones_either_way_land_in_tune_with_the_formants_where_they_were() {
    for hz in [82.4, 110.0, 220.0, 440.0, 880.0] {
        let input = vowel(hz, 1.5);
        for semitones in [6.0_f32, -6.0] {
            let run = shifted(&input, semitones, 1.0);
            let asked = hz * 2f64.powf(f64::from(semitones) / 12.0);
            let stretch = tail(&run.output, 0.7);

            let cents = cents_between(pitch_near(stretch, asked), asked);
            assert!(
                cents.abs() < PITCH_TOLERANCE_CENTS,
                "{hz} Hz {semitones:+} semitones came out {cents:+.2} cents off"
            );

            let error = envelope_error(stretch, asked, 1.0);
            let well_sampled = asked < WELL_SAMPLED_HZ;
            let allowed_hz =
                if well_sampled { 0.1 * error.asked_peak_hz } else { error.spacing_hz };
            let peak_off = (error.peak_hz - error.asked_peak_hz).abs();
            assert!(
                peak_off <= allowed_hz,
                "{hz} Hz {semitones:+}: formant peak at {:.0} Hz, asked {:.0} Hz",
                error.peak_hz,
                error.asked_peak_hz
            );
            if well_sampled {
                assert!(
                    error.rms_db < 4.0,
                    "{hz} Hz {semitones:+}: envelope off by {:.1} dB RMS",
                    error.rms_db
                );
            }

            let bound = VoiceShifter::latency_bound_samples(RATE, semitones, hz);
            assert!(
                run.most_latency <= bound,
                "{hz} Hz {semitones:+}: latency {} over its bound {bound}",
                run.most_latency
            );
        }
    }
}

// The done-when's other half. Pitch is the filter's to lose, since no delay line runs; a 20 percent
// move must show up at the peak within the same 10 percent, with no delay at all.
#[test]
fn formants_move_twenty_percent_with_the_pitch_left_alone() {
    for hz in [82.4, 110.0, 220.0, 440.0, 880.0] {
        let input = vowel(hz, 1.2);
        for factor in [1.2_f32, 0.8] {
            let run = shifted(&input, 0.0, factor);
            assert_eq!(run.most_latency, 0, "{hz} Hz: a formant move added delay");
            let stretch = tail(&run.output, 0.5);

            let cents = cents_between(pitch_near(stretch, hz), hz);
            assert!(
                cents.abs() < PITCH_TOLERANCE_CENTS,
                "{hz} Hz with formants x{factor} came out {cents:+.2} cents off"
            );

            let error = envelope_error(stretch, hz, f64::from(factor));
            let well_sampled = hz < WELL_SAMPLED_HZ;
            let allowed_hz =
                if well_sampled { 0.1 * error.asked_peak_hz } else { error.spacing_hz };
            let peak_off = (error.peak_hz - error.asked_peak_hz).abs();
            assert!(
                peak_off <= allowed_hz,
                "{hz} Hz x{factor}: formant peak at {:.0} Hz, asked {:.0} Hz",
                error.peak_hz,
                error.asked_peak_hz
            );
            if well_sampled {
                assert!(error.rms_db < 4.0, "{hz} Hz x{factor}: off by {:.1} dB", error.rms_db);
            }
        }
    }
}

// The range harmony needs. Formants are not tested here: past six semitones they go on sampling
// further from where the voice was, and the listen decides how far that holds. At this range a
// crossfade can drift most of a high voice's period, so the delay may overshoot its bound by up to
// half a millisecond.
#[test]
fn an_octave_either_way_lands_in_tune() {
    for hz in [110.0, 440.0, 880.0] {
        let input = vowel(hz, 1.2);
        for semitones in [12.0_f32, -12.0] {
            let run = shifted(&input, semitones, 1.0);
            let asked = hz * 2f64.powf(f64::from(semitones) / 12.0);
            let cents = cents_between(pitch_near(tail(&run.output, 0.5), asked), asked);
            assert!(
                cents.abs() < PITCH_TOLERANCE_CENTS,
                "{hz} Hz {semitones:+} semitones came out {cents:+.2} cents off"
            );
            let bound = VoiceShifter::latency_bound_samples(RATE, semitones, hz) + 24;
            assert!(
                run.most_latency <= bound,
                "{hz} Hz {semitones:+}: {} over {bound}",
                run.most_latency
            );
        }
    }
}

// Off is off: the input itself, undelayed, until the first request.
#[test]
fn no_shift_and_no_move_is_the_input_itself() {
    let input = vowel(220.0, 0.5);
    let run = shifted(&input, 0.0, 1.0);
    assert_eq!(run.output, input, "an idle shifter changed the signal");
    assert_eq!(run.most_latency, 0);
}

// Once the settings go back, the path goes back: the 20 ms fade and a filter landing on flat.
#[test]
fn turning_it_off_returns_to_the_input_itself() {
    let input = vowel(220.0, 1.0);
    let mut shifter = VoiceShifter::new(RATE);
    shifter.set(4.0, 1.2);
    shifted_by(&mut shifter, &input);
    shifter.set(0.0, 1.0);
    shifted_by(&mut shifter, &input);
    let run = shifted_by(&mut shifter, &input);
    assert_eq!(run.output, input, "still changing the signal after going back to no shift");
    assert_eq!(shifter.latency_samples(), 0);
}

// Silence in is silence out, in any setting, and a voice leaves nothing behind but a tail under
// -80 dBFS 200 ms after it stops.
#[test]
fn silence_stays_silent() {
    for (semitones, factor) in [(6.0, 1.0), (-6.0, 1.0), (0.0, 1.2), (4.0, 0.8)] {
        let run = shifted(&vec![0.0; 24_000], semitones, factor);
        assert!(run.output.iter().all(|s| *s == 0.0), "silence came out as sound");

        let mut voice_then_silence = vowel(110.0, 0.5);
        voice_then_silence.resize(48_000, 0.0);
        let run = shifted(&voice_then_silence, semitones, factor);
        let quiet = rms(tail(&run.output, 0.7));
        assert!(quiet < 1e-4, "a tail of {quiet} after the voice stopped");
    }
}

// Noise has no pitch, so this is the path a consonant takes. Its level holds within 3 dB (the
// crossfades' dips on one side, a model that never quite settles on the other), and the delay
// holds the bound of the longest period.
#[test]
fn noise_keeps_its_level_and_its_delay_stays_bound() {
    let input = noise(72_000, 0.3);
    for (semitones, factor) in [(6.0_f32, 1.0), (-6.0, 1.0), (0.0, 1.2), (4.0, 0.8)] {
        let run = shifted(&input, semitones, factor);
        assert!(run.output.iter().all(|s| s.is_finite()), "noise produced a non-finite sample");
        let change = 20.0 * (rms(tail(&run.output, 0.5)) / rms(tail(&input, 0.5))).log10();
        assert!(change.abs() < 3.0, "noise changed level by {change:+.1} dB");
        let bound = VoiceShifter::latency_bound_samples(RATE, semitones, 82.0);
        assert!(run.most_latency <= bound, "noise: {} over {bound}", run.most_latency);
    }
}

// An /s/ keeps its place in the spectrum through a transposition: the filter puts back what the
// delay line moved. Measured in the bands an /s/ has and does not. Without the filter a voice
// down 6 semitones has its hiss's energy centre 29 percent low.
#[test]
fn a_hiss_stays_in_its_band_through_a_transposition() {
    let centre_of = |signal: &[f32]| {
        let (windowed, sum) = hann_windowed(&signal[..12_000]);
        let bands: Vec<f64> = (1..=24).map(|k| f64::from(k) * 1_000.0 - 500.0).collect();
        let energy: Vec<f64> =
            bands.iter().map(|hz| amplitude_at(&windowed, sum, *hz).powi(2)).collect();
        let weighted: f64 = bands.iter().zip(&energy).map(|(hz, e)| hz * e).sum();
        weighted / energy.iter().sum::<f64>()
    };
    let input = hiss(48_000, 0.3);
    let in_centre = centre_of(tail(&input, 0.5));
    let run = shifted(&input, -6.0, 1.0);
    let out_centre = centre_of(tail(&run.output, 0.5));
    let moved = out_centre / in_centre;
    // A band-limited line loses the top third of a falling shift, so the centre may fall some
    assert!((0.8..1.1).contains(&moved), "the hiss's centre moved by a factor of {moved:.2}");
}

// A pitch step lands as fast as the delay line reads, not as fast as the detector hears. The
// first 200 ms of the new note are in tune from 30 ms after the step: a grain shifter, spacing its
// grains from the detector's reading, is still on the old note for about 50 ms of that.
#[test]
fn a_pitch_step_is_in_tune_within_30_ms() {
    let step = 48_000;
    let mut contour = vec![220.0; step];
    contour.resize(2 * step, 330.0);
    let input = sung(&contour, 1.0);
    for semitones in [6.0_f32, -6.0] {
        let run = shifted(&input, semitones, 1.0);
        let asked = 330.0 * 2f64.powf(f64::from(semitones) / 12.0);
        let from = step + (0.030 * SR) as usize;
        let stretch = &run.output[from..from + (0.2 * SR) as usize];
        let cents = cents_between(pitch_near(stretch, asked), asked);
        assert!(
            cents.abs() < PITCH_TOLERANCE_CENTS,
            "{semitones:+}: {cents:+.2} cents after a step"
        );
    }
}

// Vibrato is the singer's: its depth comes through, and so its pitch follows the input at the
// delay's own lag. The tolerance, 8 cents RMS against a track of a 50-cent vibrato, leaves room
// for YIN reading a moving pitch over 43 ms, which smears both tracks alike.
#[test]
fn vibrato_comes_through_at_its_own_depth() {
    let samples = 2 * 48_000;
    let contour: Vec<f64> = (0..samples)
        .map(|i| {
            let t = i as f64 / SR;
            220.0 * 2f64.powf(50.0 / 1200.0 * (2.0 * PI * 5.5 * t).sin())
        })
        .collect();
    let input = sung(&contour, 1.0);
    let run = shifted(&input, 4.0, 1.0);
    let ratio = 2f64.powf(4.0 / 12.0);
    let mut yin = YinDetector::new(RATE, 2048);
    let mut errors = Vec::new();
    for end in (24_000..samples).step_by(480) {
        let reading = f64::from(yin.analyze(&run.output[end - 2048..end]).hz);
        // The reading describes the window's centre, and the output trails by about 3 ms
        let at = end - 1024 - 150;
        errors.push(cents_between(reading, contour[at] * ratio));
    }
    let track = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64).sqrt();
    assert!(track < 8.0, "the output's pitch left the vibrato by {track:.1} cents RMS");
}

// A voice that loses periodicity and finds it again, as a singer's does at each consonant. Every
// vowel after the first lands in tune, read from 40 ms in; the hiss holds its level within 4.5 dB
// (measured from 10 ms after the delay has passed the edge, so the vowel's tail is not in it).
#[test]
fn a_voice_that_loses_and_regains_periodicity_comes_back_in_tune() {
    let vowel_samples = (0.2 * SR) as usize;
    let hiss_samples = (0.1 * SR) as usize;
    let period = vowel_samples + hiss_samples;
    let sounds_like = vowel(220.0, 3.0);
    let breath = hiss(sounds_like.len(), 0.1);
    let input: Vec<f32> = sounds_like
        .iter()
        .zip(&breath)
        .enumerate()
        .map(|(i, (v, b))| if i % period < vowel_samples { *v } else { *b })
        .collect();
    for semitones in [6.0_f32, -6.0] {
        let run = shifted(&input, semitones, 1.0);
        let asked = 220.0 * 2f64.powf(f64::from(semitones) / 12.0);
        let edge_room = (0.040 * SR) as usize;
        for repeat in 1..8 {
            let from = repeat * period + edge_room;
            let stretch = &run.output[from..from + vowel_samples - edge_room];
            let cents = cents_between(pitch_near(stretch, asked), asked);
            assert!(cents.abs() < PITCH_TOLERANCE_CENTS, "vowel {repeat}: {cents:+.2} cents");
        }
        let (mut out_power, mut in_power) = (0.0, 0.0);
        let delay_room = (0.025 * SR) as usize;
        for repeat in 1..8 {
            let from = repeat * period + vowel_samples + delay_room;
            let to = repeat * period + period;
            out_power += rms(&run.output[from..to]).powi(2);
            in_power += rms(&input[from..to]).powi(2);
        }
        let change = 10.0 * (out_power / in_power).log10();
        assert!(change.abs() < 4.5, "{semitones:+}: the hiss changed level by {change:+.1} dB");
    }
}

// What a jitter buffer's concealment hands over after a lost packet: the last 20 ms again, fading,
// then a jump back to the voice, here every 100 ms, far more often than a Wi-Fi burst does it. The
// note stays in tune and the level holds within 3 dB.
#[test]
fn concealment_of_lost_packets_stays_in_tune() {
    let mut input = vowel(220.0, 2.0);
    let lost = (0.020 * SR) as usize;
    for at in (14_400..input.len() - lost).step_by(4_800) {
        for i in 0..lost {
            let fade = 1.0 - 0.5 * i as f32 / lost as f32;
            input[at + i] = input[at + i - lost] * fade;
        }
    }
    for (semitones, factor) in [(6.0_f32, 1.0), (-6.0, 1.0), (0.0, 1.2), (4.0, 0.8)] {
        let run = shifted(&input, semitones, factor);
        let asked = 220.0 * 2f64.powf(f64::from(semitones) / 12.0);
        let cents = cents_between(pitch_near(tail(&run.output, 0.7), asked), asked);
        assert!(cents.abs() < PITCH_TOLERANCE_CENTS, "{semitones:+} x{factor}: {cents:+.2} cents");
        let change = 20.0 * (rms(tail(&run.output, 0.7)) / rms(tail(&input, 0.7))).log10();
        assert!(change.abs() < 3.0, "{semitones:+} x{factor}: level changed {change:+.1} dB");
    }
}

#[test]
#[should_panic(expected = "an octave high")]
fn the_pitch_check_catches_a_voice_an_octave_high() {
    pitch_near(&vowel(440.0, 0.5), 220.0);
}

#[test]
#[should_panic(expected = "an octave low")]
fn the_pitch_check_catches_a_voice_an_octave_low() {
    pitch_near(&vowel(110.0, 0.5), 220.0);
}

// The detector reads a wrong octave now and then, and a note must not follow it. The pitch out does
// not come from the reading at all, so it stays within the same tolerance, and the splices keep
// finding the waveform, so all but a sliver of the output's energy stays on its harmonics.
#[test]
fn a_wrong_octave_reading_is_not_heard() {
    let hz = 220.0;
    let true_period = SR / hz;
    let input = vowel(hz, 1.5);
    for reading in [0.5, 2.0] {
        for semitones in [6.0_f32, -6.0] {
            let mut shifter = VoiceShifter::new(RATE);
            shifter.set(semitones, 1.0);
            // The detector stops here, leaving the wrong reading in place for the whole note
            shifter.detector_hop = usize::MAX / 2;
            shifter.until_reading = usize::MAX / 2;
            shifter.period = true_period * reading;
            let run = shifted_by(&mut shifter, &input);

            let asked = hz * 2f64.powf(f64::from(semitones) / 12.0);
            let stretch = tail(&run.output, 0.7);
            let cents = cents_between(pitch_near(stretch, asked), asked);
            assert!(
                cents.abs() < PITCH_TOLERANCE_CENTS,
                "reading x{reading} {semitones:+}: {cents:+.2} cents"
            );

            let (windowed, sum) = hann_windowed(stretch);
            let on_harmonics: f64 = (1..)
                .map(|h| h as f64 * asked)
                .take_while(|f| *f < 20_000.0)
                .map(|f| amplitude_at(&windowed, sum, f).powi(2) / 2.0)
                .sum();
            let all = rms(stretch).powi(2);
            assert!(
                on_harmonics / all > 0.9,
                "reading x{reading} {semitones:+}: {:.3} of the energy on the harmonics",
                on_harmonics / all
            );
        }
    }
}

// A new singer on the slot starts clean: nothing of the last voice comes out.
#[test]
fn reset_forgets_the_last_voice() {
    let mut shifter = VoiceShifter::new(RATE);
    shifter.set(6.0, 1.2);
    shifted_by(&mut shifter, &vowel(220.0, 0.5));
    assert_no_alloc(|| shifter.reset());
    let run = shifted_by(&mut shifter, &[0.0; 12_000]);
    assert!(run.output.iter().all(|s| *s == 0.0), "the last voice came out after a reset");
}

// A bad sample must not end the song: it does not stay in the filters, nothing panics, and once
// the line has played it out the voice is back in tune with its formants still held.
#[test]
fn a_bad_sample_passes_and_the_voice_comes_back() {
    let mut input = vowel(110.0, 1.5);
    input[24_000] = f32::NAN;
    input[24_001] = f32::INFINITY;
    let run = shifted(&input, 4.0, 1.0);
    assert!(run.output.iter().all(|s| s.is_finite()), "a bad sample reached the output");
    let asked = 110.0 * 2f64.powf(4.0 / 12.0);
    let stretch = tail(&run.output, 1.0);
    let cents = cents_between(pitch_near(stretch, asked), asked);
    assert!(cents.abs() < PITCH_TOLERANCE_CENTS, "{cents:+.2} cents after a bad sample");
    let error = envelope_error(stretch, asked, 1.0);
    assert!(
        error.rms_db < 4.0,
        "the formants stayed off after a bad sample: {:.1} dB",
        error.rms_db
    );
}

// The stated bound, in the samples decision 0004's table is worked out from.
#[test]
fn the_bound_is_what_the_doc_says() {
    let samples = |semitones, hz| VoiceShifter::latency_bound_samples(RATE, semitones, hz);
    assert_eq!(samples(6.0, 82.0), 720, "15.0 ms");
    assert_eq!(samples(-6.0, 82.0), 666, "13.9 ms");
    assert_eq!(samples(0.0, 82.0), 0);
    // A voice under E2 is bound as E2, whatever it sings
    assert_eq!(samples(6.0, 50.0), samples(6.0, 82.0));
}
