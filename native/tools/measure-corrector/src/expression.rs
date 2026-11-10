//! Objective reads on what correction does to a voice's movement. Melodyne's split is the model:
//! a note is a centre, a slow drift and a fast modulation. A corrector should move the centre and
//! leave the rest. These measures say how much movement survives, and how much appeared that the
//! singer did not sing, which is the physical half of what a listener calls warble.
//!
//! How far to trust them is docs/decisions/0005-corrector-measurement.md. In short: they are
//! measured against known signals, not yet against listeners, so they are a regression alarm
//! beside the listen, never a substitute for it.

use std::f64::consts::TAU;

use crate::contour::Point;
use crate::music::{cents_off_class, median, percentile};
use crate::sung_clip::MeantNote;

/// Rates a pitch wobble is heard as a wobble: vibrato sits at 4 to 8 Hz, and fluctuation of any
/// kind is loudest near 4 Hz and fades by the mid teens.
pub const WARBLE_BAND: (f64, f64) = (3.0, 12.0);
pub const VIBRATO_BAND: (f64, f64) = (4.0, 8.0);
/// RMS cents in the vibrato band above which a note is said to have vibrato: about 14 cents of
/// extent either side, the narrow end of what singers use on purpose.
const HAS_VIBRATO_CENTS: f64 = 10.0;
/// A note needs a second of contour to resolve 3 Hz with a window on it; less still resolves
/// vibrato, so this is the shortest note read at all. Half a second is three cycles at 6 Hz.
const SHORTEST_NOTE_MS: f64 = 500.0;
/// Past the note's attack, where any corrector is arriving, and before its release.
const NOTE_LEAD_MS: f64 = 60.0;
const NOTE_TAIL_MS: f64 = 30.0;

/// How much of `series`, sampled at `rate` Hz, moves between `low` and `high` Hz, as an RMS in its
/// own units: linear trend off, a Hann window on, the band's bins of its DFT summed.
pub fn band_rms(series: &[f64], rate: f64, low: f64, high: f64) -> f64 {
    let n = series.len();
    if n < 4 {
        return 0.0;
    }
    let detrended = without_trend(series);
    let window: Vec<f64> =
        (0..n).map(|i| 0.5 - 0.5 * (TAU * (i as f64 + 0.5) / n as f64).cos()).collect();
    let window_energy: f64 = window.iter().map(|w| w * w).sum();
    let bin_hz = rate / n as f64;
    let mut power = 0.0;
    for bin in (low / bin_hz).ceil() as usize..=(high / bin_hz).floor() as usize {
        let step = TAU * bin as f64 / n as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, (x, w)) in detrended.iter().zip(&window).enumerate() {
            let angle = step * i as f64;
            re += x * w * angle.cos();
            im -= x * w * angle.sin();
        }
        power += 2.0 * (re * re + im * im) / (n as f64 * window_energy);
    }
    power.sqrt()
}

fn without_trend(series: &[f64]) -> Vec<f64> {
    let n = series.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = series.iter().sum::<f64>() / n;
    let (mut top, mut bottom) = (0.0, 0.0);
    for (i, y) in series.iter().enumerate() {
        top += (i as f64 - mean_x) * (y - mean_y);
        bottom += (i as f64 - mean_x).powi(2);
    }
    let slope = if bottom > 0.0 { top / bottom } else { 0.0 };
    series.iter().enumerate().map(|(i, y)| y - mean_y - slope * (i as f64 - mean_x)).collect()
}

/// A note's contour every 10 ms, in cents from the note in whatever octave it was sung. A short
/// hole is bridged in a line. `None` when under four fifths of it is voiced.
fn note_contour(points: &[Point], note: &MeantNote) -> Option<Vec<f64>> {
    let from = note.start_ms + NOTE_LEAD_MS;
    let to = note.end_ms - NOTE_TAIL_MS;
    let meant = f64::from(note.midi);
    let start = points.partition_point(|point| point.ms < from);
    let end = points.partition_point(|point| point.ms < to);
    let frames = points.get(start..end)?;
    if frames.is_empty() {
        return None;
    }
    let raw: Vec<Option<f64>> = frames
        .iter()
        .map(|point| point.voiced().then(|| cents_off_class(point.midi(), meant)))
        .collect();
    let voiced: Vec<f64> = raw.iter().flatten().copied().collect();
    if (voiced.len() as f64) < 0.8 * raw.len() as f64 {
        return None;
    }
    // A reading far from the rest of the note is the detector slipping, not the singer
    let centre = median(&voiced);
    let kept: Vec<Option<f64>> = raw
        .into_iter()
        .map(|reading| reading.filter(|cents| (cents - centre).abs() <= 300.0))
        .collect();
    let first = kept.iter().position(Option::is_some)?;
    let mut filled = vec![kept[first]?; kept.len()];
    let mut last = (first, kept[first]?);
    for index in first + 1..kept.len() {
        if let Some(value) = kept[index] {
            let span = (index - last.0) as f64;
            for (offset, slot) in filled[last.0 + 1..=index].iter_mut().enumerate() {
                *slot = last.1 + (value - last.1) * (offset + 1) as f64 / span;
            }
            last = (index, value);
        } else {
            filled[index] = last.1;
        }
    }
    Some(filled)
}

/// How much more `wet` moves than `dry`, in their RMS: 0 when it moves less. A plain difference
/// rather than a difference of powers, since correction's added motion follows the voice's own:
/// 78 cents of vibrato grown to 83 is five cents more, where powers would call it 29.
fn added(dry: f64, wet: f64) -> f64 {
    (wet - dry).max(0.0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub sustained_notes: usize,
    /// The median, over sustained notes, of how much of the voice's warble-band movement came
    /// through. Under 1 flattens; over 1 adds.
    pub motion_kept: f64,
    /// Cents RMS of warble-band movement that was not in the voice, on the worst notes: the 95th
    /// percentile, because one warbling note fails the bar and a median would hide it.
    pub motion_added_p95: f64,
    pub vibrato_notes: usize,
    /// As `motion_kept`, over notes with clear vibrato, in the vibrato band.
    pub vibrato_kept: Option<f64>,
    /// Decibels RMS of warble-band level movement that was not in the voice, on the worst notes:
    /// a shifter's splices would show here.
    pub envelope_added_p95_db: f64,
}

/// Compares the voice with what came out, over `notes`.
pub fn compare(
    dry: &[Point],
    wet: &[Point],
    dry_envelope: &Envelope,
    wet_envelope: &Envelope,
    notes: &[MeantNote],
) -> Expression {
    let mut kept = Vec::new();
    let mut extra = Vec::new();
    let mut vibrato = Vec::new();
    let mut level = Vec::new();
    for note in notes.iter().filter(|note| note.end_ms - note.start_ms >= SHORTEST_NOTE_MS) {
        let (Some(dry_cents), Some(wet_cents)) = (note_contour(dry, note), note_contour(wet, note))
        else {
            continue;
        };
        let (low, high) = WARBLE_BAND;
        let dry_motion = band_rms(&dry_cents, 100.0, low, high);
        let wet_motion = band_rms(&wet_cents, 100.0, low, high);
        kept.push(wet_motion / dry_motion.max(1.0));
        extra.push(added(dry_motion, wet_motion));

        let (low, high) = VIBRATO_BAND;
        let dry_vibrato = band_rms(&dry_cents, 100.0, low, high);
        if dry_vibrato >= HAS_VIBRATO_CENTS {
            vibrato.push(band_rms(&wet_cents, 100.0, low, high) / dry_vibrato);
        }

        let (low, high) = WARBLE_BAND;
        let dry_level = band_rms(&dry_envelope.within(note), Envelope::RATE, low, high);
        let wet_level = band_rms(&wet_envelope.within(note), Envelope::RATE, low, high);
        level.push(added(dry_level, wet_level));
    }
    Expression {
        sustained_notes: kept.len(),
        motion_kept: median(&kept),
        motion_added_p95: percentile(&extra, 0.95),
        vibrato_notes: vibrato.len(),
        vibrato_kept: (!vibrato.is_empty()).then(|| median(&vibrato)),
        envelope_added_p95_db: percentile(&level, 0.95),
    }
}

/// A signal's level in decibels every 5 ms, on the clip's clock.
pub struct Envelope {
    decibels: Vec<f64>,
}

impl Envelope {
    const FRAME: usize = 240;
    pub const RATE: f64 = 200.0;

    pub fn of(signal: &[f32], lag_frames: i64) -> Self {
        let lag = usize::try_from(lag_frames).unwrap_or(0);
        let decibels = signal
            .get(lag..)
            .unwrap_or_default()
            .as_chunks::<{ Self::FRAME }>()
            .0
            .iter()
            .map(|frame| {
                let energy: f32 = frame.iter().map(|s| s * s).sum::<f32>() / Self::FRAME as f32;
                10.0 * f64::from(energy.max(1e-9)).log10()
            })
            .collect();
        Self { decibels }
    }

    fn within(&self, note: &MeantNote) -> Vec<f64> {
        let start = ((note.start_ms + NOTE_LEAD_MS) / 5.0) as usize;
        let end = (((note.end_ms - NOTE_TAIL_MS) / 5.0) as usize).min(self.decibels.len());
        self.decibels.get(start..end).unwrap_or_default().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: f64, hz: f64, cents: f64, seconds: f64) -> Vec<f64> {
        let n = (rate * seconds) as usize;
        (0..n).map(|i| cents * (TAU * hz * i as f64 / rate).sin()).collect()
    }

    // A known wobble reads as itself: amplitude over root two, within 10 percent, on the shortest
    // note the row reads and on a long one. The tolerance is the Hann window's leakage on a bin grid.
    #[test]
    fn band_rms_reads_a_wobble_at_its_level() {
        for seconds in [0.5, 2.0] {
            let rms = band_rms(&sine(100.0, 6.0, 40.0, seconds), 100.0, 3.0, 12.0);
            let expected = 40.0 / 2f64.sqrt();
            assert!((rms - expected).abs() < 0.1 * expected, "{seconds} s read {rms}");
        }
    }

    // What is outside the band stays outside it: a slow drift and a linear trend read near zero
    #[test]
    fn a_trend_and_a_slow_drift_are_not_wobble() {
        let trend: Vec<f64> = (0..200).map(|i| i as f64).collect();
        assert!(band_rms(&trend, 100.0, 3.0, 12.0) < 0.01);
        let drift = sine(100.0, 0.5, 40.0, 2.0);
        let rms = band_rms(&drift, 100.0, 3.0, 12.0);
        assert!(rms < 3.0, "a half-Hz drift read as {rms} cents of wobble");
    }

    fn contour_of(cents_at: impl Fn(f64) -> f64, midi: f64, seconds: f64) -> Vec<Point> {
        (0..(seconds * 100.0) as usize)
            .map(|i| {
                let ms = i as f64 * 10.0;
                let hz = 440.0 * 2f64.powf((midi + cents_at(ms) / 100.0 - 69.0) / 12.0);
                Point { ms, hz: hz as f32, confidence: 1.0 }
            })
            .collect()
    }

    fn envelope() -> Envelope {
        Envelope { decibels: vec![-20.0; 400] }
    }

    const NOTE: MeantNote = MeantNote { midi: 57, start_ms: 0.0, end_ms: 2000.0 };

    fn scored(dry: impl Fn(f64) -> f64, wet: impl Fn(f64) -> f64) -> Expression {
        compare(
            &contour_of(dry, 57.0, 2.0),
            &contour_of(wet, 57.0, 2.0),
            &envelope(),
            &envelope(),
            &[NOTE],
        )
    }

    fn wobble(depth: f64, hz: f64) -> impl Fn(f64) -> f64 {
        move |ms| depth * (TAU * hz * ms / 1000.0).sin()
    }

    // Dose and response: a wobble injected into a still voice reads back as the wobble added
    #[test]
    fn added_motion_reads_the_wobble_injected() {
        for depth in [5.0, 10.0, 20.0, 40.0] {
            let score = scored(|_| 0.0, wobble(depth, 6.0));
            let expected = depth / 2f64.sqrt();
            assert!(
                (score.motion_added_p95 - expected).abs() < 0.1 * expected,
                "{depth} cents read {}",
                score.motion_added_p95
            );
        }
    }

    // A voice left alone adds nothing, whatever it sings, and a corrector that flattens vibrato
    // adds nothing either: that shows in what it kept
    #[test]
    fn the_voice_itself_adds_nothing_and_flattening_shows_as_kept() {
        let same = scored(wobble(50.0, 5.5), wobble(50.0, 5.5));
        assert!(same.motion_added_p95 < 0.1, "{same:?}");
        assert!((same.motion_kept - 1.0).abs() < 0.01, "{same:?}");
        assert_eq!(same.vibrato_notes, 1);
        assert!((same.vibrato_kept.unwrap() - 1.0).abs() < 0.01, "{same:?}");

        let flat = scored(wobble(50.0, 5.5), |_| 0.0);
        assert!(flat.motion_added_p95 < 0.1, "{flat:?}");
        assert!(flat.motion_kept < 0.05, "{flat:?}");
        assert!(flat.vibrato_kept.unwrap() < 0.05, "{flat:?}");
    }

    // A note with no pitch under it is not a note to read
    #[test]
    fn a_note_with_too_little_voice_is_left_out() {
        let mut dry = contour_of(|_| 0.0, 57.0, 2.0);
        dry.iter_mut().skip(50).for_each(|point| point.confidence = 0.5);
        let score =
            compare(&dry, &contour_of(|_| 0.0, 57.0, 2.0), &envelope(), &envelope(), &[NOTE]);
        assert_eq!(score.sustained_notes, 0);
    }
}
