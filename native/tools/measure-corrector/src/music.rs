//! Pitch arithmetic and the order statistics every row scores with.

use kernels::{EVERY_PITCH_CLASS, PitchClassMask};

pub fn midi_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

pub fn hz_of(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

pub fn cents(hz: f64, reference: f64) -> f64 {
    1200.0 * (hz / reference).log2()
}

/// The harness's percentile: the value `p` of the way through, rounded down. `NaN` when empty.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * p).floor() as usize).min(sorted.len().saturating_sub(1));
    sorted.get(index).copied().unwrap_or(f64::NAN)
}

pub fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

/// How far a sung pitch sits from the note meant, in cents, in whatever octave it was sung: the
/// corrector pulls to a pitch class, so an octave off is not an error here. Signed, -600 to 600.
pub fn cents_off_class(midi: f64, meant: f64) -> f64 {
    let semitones = (midi - meant).rem_euclid(12.0);
    let wrapped = if semitones > 6.0 { semitones - 12.0 } else { semitones };
    wrapped * 100.0
}

/// A key's pitch classes as bits, C in bit 0: `"G"`, `"F#m"`, `"Bb"`. The chromatic mask for
/// `"chromatic"`.
pub fn key_mask(key: &str) -> Result<PitchClassMask, String> {
    if key == "chromatic" {
        return Ok(EVERY_PITCH_CLASS);
    }
    let (name, scale): (&str, [u16; 7]) = match key.strip_suffix('m') {
        Some(name) => (name, [0, 2, 3, 5, 7, 8, 10]),
        None => (key, [0, 2, 4, 5, 7, 9, 11]),
    };
    let mut letters = name.chars();
    let natural = match letters.next() {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(format!("{key} is not a key")),
    };
    let tonic = match letters.as_str() {
        "" => natural,
        "#" => natural + 1,
        "b" => natural + 11,
        _ => return Err(format!("{key} is not a key")),
    };
    Ok(scale.iter().fold(0, |mask, step| mask | 1 << ((tonic + step) % 12)))
}

/// A small deterministic generator, so a run or a noise condition is the same every time.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_u32(&mut self) -> u32 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    /// Uniform in -1 to 1.
    pub fn signed(&mut self) -> f32 {
        self.next_u32() as f32 / (1u32 << 30) as f32 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_its_scales_classes() {
        assert_eq!(key_mask("C").unwrap(), 0b1010_1011_0101);
        // G major is C major with F# for F
        assert_eq!(key_mask("G").unwrap(), 0b1010_1101_0101);
        // F# minor is A major: A B C# D E F# G#
        assert_eq!(key_mask("F#m").unwrap(), key_mask("A").unwrap());
        assert_eq!(key_mask("A#m").unwrap(), key_mask("Bbm").unwrap());
        assert!(key_mask("H").is_err());
    }

    #[test]
    fn an_octave_away_is_on_the_note() {
        assert!((cents_off_class(72.3, 60.0) - 30.0).abs() < 1e-9);
        assert!((cents_off_class(59.7, 72.0) + 30.0).abs() < 1e-9);
    }
}
