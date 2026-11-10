use crate::sung_clip::MeantNote;

/// Across a gap between notes a whole-note move goes in a line, never in under this.
const SHORTEST_GLIDE_MS: f64 = 20.0;

/// A pitch move in cents over a clip's time, every `STEP_MS`, read in a line between steps: what
/// the degrader asks a voice to sing off by, and what builds the answer key.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    cents: Vec<f64>,
}

impl Curve {
    pub const STEP_MS: f64 = 5.0;

    /// `cents_at(ms)` sampled every step over `length_ms`.
    pub fn sampled(length_ms: f64, cents_at: impl Fn(f64) -> f64) -> Self {
        let steps = (length_ms / Self::STEP_MS).ceil() as usize + 1;
        Self { cents: (0..steps).map(|step| cents_at(step as f64 * Self::STEP_MS)).collect() }
    }

    /// `cents[i]` all through `notes[i]`, in a line across the gap to the next note, held before
    /// the first and after the last. `notes` are in order and do not overlap.
    pub fn by_note(length_ms: f64, notes: &[MeantNote], cents: &[f64]) -> Self {
        Self::sampled(length_ms, |ms| {
            let next = notes.partition_point(|note| note.start_ms <= ms);
            let Some(before) = next.checked_sub(1) else {
                return cents.first().copied().unwrap_or(0.0);
            };
            let (Some(after), true) = (notes.get(next), ms >= notes[before].end_ms) else {
                return cents[before];
            };
            let glide_from = notes[before].end_ms.min(after.start_ms - SHORTEST_GLIDE_MS);
            let through = ((ms - glide_from) / (after.start_ms - glide_from)).clamp(0.0, 1.0);
            cents[before] + (cents[next] - cents[before]) * through
        })
    }

    /// Held at the ends.
    pub fn at(&self, ms: f64) -> f64 {
        let position = (ms / Self::STEP_MS).max(0.0);
        let step = position.floor() as usize;
        let Some(&before) = self.cents.get(step) else {
            return self.cents.last().copied().unwrap_or(0.0);
        };
        let after = self.cents.get(step + 1).copied().unwrap_or(before);
        before + (after - before) * position.fract()
    }

    /// The move that undoes this one.
    pub fn undone(&self) -> Self {
        Self { cents: self.cents.iter().map(|cents| -cents).collect() }
    }

    pub fn plus(&self, other: &Self) -> Self {
        let length = self.cents.len().max(other.cents.len());
        Self {
            cents: (0..length)
                .map(|step| {
                    let ms = step as f64 * Self::STEP_MS;
                    self.at(ms) + other.at(ms)
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_reads_in_a_line_between_steps_and_holds_at_its_ends() {
        let curve = Curve::sampled(10.0, |ms| ms * 2.0);
        assert_eq!(curve.at(2.5), 5.0);
        assert_eq!(curve.at(-4.0), 0.0);
        assert_eq!(curve.at(1000.0), 20.0);
        assert_eq!(curve.plus(&curve.undone()).at(7.0), 0.0);
    }

    #[test]
    fn a_note_move_is_whole_inside_each_note_and_glides_across_the_gap() {
        let note = |start_ms: f64, end_ms: f64| MeantNote { midi: 60, start_ms, end_ms };
        let curve =
            Curve::by_note(1000.0, &[note(100.0, 300.0), note(400.0, 600.0)], &[10.0, -10.0]);
        assert_eq!(
            [curve.at(0.0), curve.at(200.0), curve.at(350.0), curve.at(500.0)],
            [10.0, 10.0, 0.0, -10.0]
        );
        assert_eq!(curve.at(900.0), -10.0);
    }
}
