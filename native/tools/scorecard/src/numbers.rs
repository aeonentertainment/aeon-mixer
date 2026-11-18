//! A replayed song as numbers: what each recorded slot's stages held, counted and heard, and the
//! master's level. Every number is read from what the replay's own engine reported, as a live
//! mixer's `stats()`, `level` and `pitch` report it, so the scorecard measures nothing a host
//! could not see.

use std::collections::BTreeMap;

use engine::block::RATE;
use engine::slot::effects::Reading;
use replay::bundle::song::{Played, PlayedSlot};
use replay::levels::Levels;

/// A reading this sure, at or above this pitch, is a voice the corrector pulls: its own bar.
const VOICED_CONFIDENCE: f32 = 0.9;
const LOWEST_CORRECTED_HZ: f32 = 82.0;

/// Each number by name, rounded to a hundredth.
pub type Numbers = BTreeMap<String, f64>;

/// The numbers `played` scores to. A slot that recorded nothing scores none, and a number with
/// nothing to read, such as pitch error with correction off, is left out.
pub fn of(played: &Played) -> Numbers {
    let mut numbers = Numbers::new();
    let mut put = |name: String, value: f64| {
        numbers.insert(name, (value * 100.0).round() / 100.0);
    };
    for (index, slot) in played.slots.iter().enumerate() {
        if !slot.recorded {
            continue;
        }
        let name = |number: &str| format!("slot{index}.{number}");
        put(name("late"), f64::from(slot.stats.late));
        put(name("concealed"), f64::from(slot.stats.concealed));
        put(name("playoutDelayMs"), f64::from(slot.stats.playout_delay_ms));
        // A sawtooth while correcting, so its mean says what it held and one block's peak does not
        let held: Vec<f64> = slot
            .corrector_delay_frames
            .iter()
            .filter(|&&frames| frames > 0)
            .map(|&frames| ms(frames))
            .collect();
        if !held.is_empty() {
            put(name("correctorDelayMs"), held.iter().sum::<f64>() / held.len() as f64);
        }
        if let Some(cents) = median(errors_cents(slot, |r| r.error_semitones)) {
            put(name("pitchErrorCents"), cents);
        }
        let corrected = |r: &Reading| r.error_semitones - r.shift_semitones;
        if let Some(cents) = median(errors_cents(slot, corrected)) {
            put(name("correctedErrorCents"), cents);
        }
        if let Some(gaps) = let_goes(&slot.readings) {
            put(name("letGoes"), f64::from(gaps));
        }
        if let Some(removed) = median(slot.echo.iter().map(|f| f64::from(f.removed_db)).collect()) {
            put(name("echoRemovedDb"), removed);
        }
        if let Some(bleed) = median(slot.echo.iter().map(|f| f64::from(f.bleed_db)).collect()) {
            put(name("bleedDb"), bleed);
        }
        put(name("howlCatches"), f64::from(slot.howl_catches));
        for (number, value) in levels(&slot.meter_db) {
            put(name(number), value);
        }
    }
    for (number, value) in levels(&played.master_db) {
        put(format!("master.{number}"), value);
    }
    numbers
}

/// How far each voiced reading the corrector pulled toward a guide note sat from it, in cents,
/// by `error`: the voice as it reached the corrector, or as it came out.
fn errors_cents(slot: &PlayedSlot, error: impl Fn(&Reading) -> f32) -> Vec<f64> {
    slot.readings
        .iter()
        .filter(|r| r.target_midi.is_some())
        .filter(|r| r.confidence >= VOICED_CONFIDENCE && r.hz >= LOWEST_CORRECTED_HZ)
        .map(|r| f64::from(error(r).abs()) * 100.0)
        .collect()
}

/// Times correction let go of a note and took it again while the guide still held it: each gap
/// in the pull with a pulled reading either side, within one run of readings on one guide note.
/// `None` when no reading had a guide note to pull toward.
pub(crate) fn let_goes(readings: &[Reading]) -> Option<u32> {
    let mut gaps = None;
    for note in readings.chunk_by(|a, b| a.target_midi == b.target_midi) {
        if note[0].target_midi.is_none() {
            continue;
        }
        let pulls = note.chunk_by(|a, b| a.pulling == b.pulling).filter(|run| run[0].pulling);
        *gaps.get_or_insert(0) += (pulls.count() as u32).saturating_sub(1);
    }
    gaps
}

/// The level 10 percent of 100 ms readings are above, and the median. The loudest reading is one
/// window and moves with where the windows fall, so it is left to `replay`.
fn levels(readings: &[f32]) -> [(&'static str, f64); 2] {
    let levels = Levels::of_readings(readings);
    [
        ("levelTopTenthDb", f64::from(levels.top_tenth_db)),
        ("levelMedianDb", f64::from(levels.median_db)),
    ]
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

fn ms(frames: u32) -> f64 {
    f64::from(frames) * 1000.0 / f64::from(RATE)
}
