use std::time::Duration;

use devices::Playing;

/// What the host last heard of where the engine plays: the moves and the refused output. The
/// events thread asks it on every drain whether `outputChanged` is due.
#[derive(Default)]
pub struct OutputSeen {
    moves: u32,
    refused: Option<String>,
}

impl OutputSeen {
    /// The gap and the hold to report when `playing` differs from what the host last heard: the
    /// move's after a move, and none when only a refusal changed, as when it clears with no move.
    pub fn change(&mut self, playing: &Playing) -> Option<(Duration, Duration)> {
        let moved = playing.moves != self.moves;
        let refused = playing.refused.as_ref().map(|refused| &refused.output.id);
        if !moved && refused == self.refused.as_ref() {
            return None;
        }
        self.moves = playing.moves;
        self.refused = refused.cloned();
        Some(if moved { (playing.gap, playing.held) } else { (Duration::ZERO, Duration::ZERO) })
    }
}

#[cfg(test)]
mod tests {
    use devices::{Output, Refused, Transport};
    use engine::driver::StreamInfo;

    use super::*;

    const GAP: Duration = Duration::from_millis(41);
    const NONE: (Duration, Duration) = (Duration::ZERO, Duration::ZERO);

    fn playing(moves: u32, refused: Option<&str>) -> Playing {
        let output = |id: &str| Output {
            id: id.into(),
            name: id.into(),
            is_default: true,
            transport: Transport::BuiltIn,
        };
        Playing {
            info: StreamInfo { sample_rate: 48_000, period_frames: 128, output_latency_frames: 0 },
            output: output("speakers"),
            moves,
            gap: GAP,
            held: GAP,
            refused: refused.map(|id| Refused { output: output(id), error: "refused".into() }),
        }
    }

    #[test]
    fn a_move_reports_its_gap_and_hold_once() {
        let mut seen = OutputSeen::default();
        assert_eq!(seen.change(&playing(0, None)), None, "the open is not a change");
        assert_eq!(seen.change(&playing(1, None)), Some((GAP, GAP)));
        assert_eq!(seen.change(&playing(1, None)), None);
        let paused = Playing { held: Duration::ZERO, ..playing(2, None) };
        assert_eq!(seen.change(&paused), Some((GAP, Duration::ZERO)), "a paused song is not held");
    }

    /// The fallback is a move that carries the refusal; the clear that follows is not a move.
    #[test]
    fn a_refusal_that_clears_with_no_move_is_reported_with_no_gap() {
        let mut seen = OutputSeen::default();
        assert_eq!(seen.change(&playing(1, Some("monitor"))), Some((GAP, GAP)));
        assert_eq!(seen.change(&playing(1, Some("monitor"))), None);
        assert_eq!(seen.change(&playing(1, None)), Some(NONE));
        assert_eq!(seen.change(&playing(1, None)), None);
    }
}
