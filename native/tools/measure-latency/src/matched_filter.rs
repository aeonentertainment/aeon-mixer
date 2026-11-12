use std::ops::Range;

use engine::block::RATE;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

/// An arrival this far under the strongest one still counts as the first sound: the direct path
/// from a speaker can be weaker than a reflection off a nearby wall.
const ONSET_RATIO: f64 = 0.1;

/// How far ahead of the strongest arrival the direct sound may be: 10 m of extra path.
const ONSET_REACH: usize = RATE as usize * 30 / 1000;

/// A sound found in a recording: where it starts, to a fraction of a frame, and how strong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrival {
    pub frame: f64,
    pub strength: f64,
}

/// Finds a known sound in a recording by correlating against it. Correlating gathers the sound's
/// whole energy into one peak, which is what lets a chirp under room noise be timed to a frame.
pub struct MatchedFilter {
    reference: Vec<f32>,
}

impl MatchedFilter {
    pub fn new(reference: Vec<f32>) -> Self {
        Self { reference }
    }

    /// The correlation's envelope at each start position in `recording`: position `i` says how
    /// much of the reference begins at `recording[i]`. The envelope, not the raw correlation,
    /// because the raw one oscillates at the band's centre, so a speaker's phase or a wire with
    /// its polarity swapped would move its peak by up to half a cycle.
    pub fn envelope(&self, recording: &[f32]) -> Vec<f64> {
        let size = (recording.len() + self.reference.len()).next_power_of_two();
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let spectrum = |signal: &[f32]| {
            let mut bins = vec![Complex64::ZERO; size];
            for (bin, sample) in bins.iter_mut().zip(signal) {
                bin.re = f64::from(*sample);
            }
            forward.process(&mut bins);
            bins
        };
        let reference = spectrum(&self.reference);
        let mut bins = spectrum(recording);
        for (bin, reference) in bins.iter_mut().zip(&reference) {
            *bin *= reference.conj();
        }
        // Only the positive frequencies: what comes back is analytic, and its magnitude the envelope
        bins[1..size / 2].iter_mut().for_each(|bin| *bin *= 2.0);
        bins[size / 2 + 1..].fill(Complex64::ZERO);
        inverse.process(&mut bins);
        let scale = 1.0 / size as f64;
        bins.iter().take(recording.len()).map(|bin| bin.norm() * scale).collect()
    }
}

/// The first sound in `window` of an envelope: the earliest peak within 20 dB of the strongest
/// and at most 30 ms ahead of it, placed between frames by a parabola through its log. None when
/// the window holds nothing, or the strongest peak sits on the window's edge, which says the sound
/// lies outside it.
pub fn first_arrival(envelope: &[f64], window: Range<usize>) -> Option<Arrival> {
    let window = window.start.max(1)..window.end.min(envelope.len().saturating_sub(1));
    let strongest = window.clone().max_by(|a, b| envelope[*a].total_cmp(&envelope[*b]))?;
    let strength = envelope[strongest];
    if strength <= 0.0 || strongest + 1 == window.end {
        return None;
    }
    let is_peak =
        |at: &usize| envelope[*at] >= envelope[at - 1] && envelope[*at] > envelope[at + 1];
    let at = (strongest.saturating_sub(ONSET_REACH).max(window.start)..=strongest)
        .find(|at| envelope[*at] >= ONSET_RATIO * strength && is_peak(at))?;
    let [before, peak, after] =
        [at - 1, at, at + 1].map(|at| envelope[at].max(f64::MIN_POSITIVE).ln());
    let curve = before - 2.0 * peak + after;
    let offset = if curve < 0.0 { 0.5 * (before - after) / curve } else { 0.0 };
    Some(Arrival { frame: at as f64 + offset, strength: envelope[at] })
}

/// The sound's strength over the envelope's level in `quiet`, in dB.
pub fn signal_to_noise_db(arrival: &Arrival, envelope: &[f64], quiet: Range<usize>) -> f64 {
    let quiet = &envelope[quiet.start.min(envelope.len())..quiet.end.min(envelope.len())];
    let power = quiet.iter().map(|level| level * level).sum::<f64>() / quiet.len().max(1) as f64;
    10.0 * (arrival.strength * arrival.strength / power.max(f64::MIN_POSITIVE)).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chirp::Chirp;

    const FRAMES: usize = RATE as usize;

    /// A deterministic white noise of the given RMS.
    fn noise(rms: f32, seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let uniform = (state >> 33) as f32 / (1u64 << 31) as f32 * 2.0 - 1.0;
            uniform * rms * 3.0f32.sqrt()
        }
    }

    fn find(recording: &[f32]) -> (Arrival, Vec<f64>) {
        let filter = MatchedFilter::new(Chirp::voice_band(0.25).samples());
        let envelope = filter.envelope(recording);
        let arrival = first_arrival(&envelope, 0..recording.len()).expect("an arrival");
        (arrival, envelope)
    }

    /// A delay of any fraction of a frame comes back to within 0.05 of a frame, 1 microsecond,
    /// with the polarity either way round.
    #[test]
    fn a_delay_comes_back_to_a_twentieth_of_a_frame_in_either_polarity() {
        let chirp = Chirp::voice_band(0.25);
        for delay in [1000.0, 1000.25, 1000.5, 4321.75, 20_000.1] {
            for polarity in [1.0f32, -1.0] {
                let recording: Vec<f32> =
                    (0..FRAMES).map(|i| polarity * chirp.at(i as f64 - delay)).collect();
                let (arrival, _) = find(&recording);
                assert!(
                    (arrival.frame - delay).abs() < 0.05,
                    "delay {delay}, polarity {polarity}: found {}",
                    arrival.frame
                );
            }
        }
    }

    /// A speaker across a room: a direct sound 10 dB under a wall's reflection 4 ms behind it, a
    /// ceiling at 9 ms, and noise as loud as the chirp's own average level. The direct sound is
    /// what gets timed, to within a frame, with the reflections and the noise all in the way.
    #[test]
    fn the_first_sound_is_timed_under_louder_reflections_and_noise() {
        let chirp = Chirp::voice_band(0.25);
        let delay = 2000.0;
        let paths = [(0.0, 0.3f32), (192.0, 1.0), (432.0, 0.5)];
        let rms = chirp.samples().iter().map(|s| s * s).sum::<f32>() / chirp.frames as f32;
        let mut noise = noise(rms.sqrt(), 7);
        let recording: Vec<f32> = (0..FRAMES)
            .map(|i| {
                let sound: f32 =
                    paths.iter().map(|(at, gain)| gain * chirp.at(i as f64 - delay - at)).sum();
                sound + noise()
            })
            .collect();
        let (arrival, envelope) = find(&recording);
        assert!((arrival.frame - delay).abs() < 1.0, "found {}", arrival.frame);
        let snr = signal_to_noise_db(&arrival, &envelope, 0..1000);
        assert!(snr > 16.0, "the direct sound stands {snr} dB above the noise");
    }

    /// The window's edge is not an arrival, and neither is an empty recording.
    #[test]
    fn nothing_is_found_where_nothing_is() {
        let chirp = Chirp::voice_band(0.25);
        let recording: Vec<f32> = (0..FRAMES).map(|i| chirp.at(i as f64 - 5000.0)).collect();
        let (_, envelope) = find(&recording);
        assert_eq!(first_arrival(&envelope, 0..5000), None, "the true peak lies past the window");
        let silence = vec![0.0; FRAMES];
        let filter = MatchedFilter::new(chirp.samples());
        assert_eq!(first_arrival(&filter.envelope(&silence), 0..FRAMES), None);
    }
}
