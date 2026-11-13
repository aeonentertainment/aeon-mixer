//! Reads the master's history at a lag that falls between frames, as the reference of an echo
//! whose path has drifted a fraction of a frame needs.

use super::history::EchoHistory;

/// Frames either side of the point: a Hann-windowed sinc 16 frames wide. Reading the reference
/// of a pure delay through it takes the same echo out as reading it whole, to 0.2 dB.
const HALF: usize = 8;
const TAPS: usize = 2 * HALF;
/// The longest read: the canceller's window and the frame before it.
const LONGEST: usize = 2 * super::FRAME + 1;

/// A lag in frames, its whole part and the taps that read its fraction.
pub struct Between {
    whole: usize,
    taps: Option<[f32; TAPS]>,
}

impl Between {
    /// A negative lag reads from the newest frame on, as a lag of nothing does.
    pub fn new(lag: f64) -> Self {
        let lag = lag.max(0.0);
        let whole = lag.floor();
        let fraction = (lag - whole) as f32;
        // The newest frames have nothing after them to read a fraction from
        let taps = (fraction > 0.0 && whole as usize >= HALF).then(|| {
            std::array::from_fn(|k| {
                // Tap k weighs the frame `k + 1 - HALF` frames older than the point
                let x = (k as f32 + 1.0 - HALF as f32) - fraction;
                let window = 0.5 + 0.5 * (std::f32::consts::PI * x / HALF as f32).cos();
                let angle = std::f32::consts::PI * x;
                window * angle.sin() / angle
            })
        });
        Self { whole: whole as usize, taps }
    }

    /// What `history.read` would give at this lag and `more` whole frames, `out` at most a
    /// window and a frame long.
    pub fn read(&self, history: &EchoHistory, more: usize, out: &mut [f32]) {
        let lag = self.whole + more;
        let Some(taps) = &self.taps else {
            return history.read(lag, out);
        };
        let mut span = [0.0; LONGEST + TAPS - 1];
        let span = &mut span[..out.len() + TAPS - 1];
        history.read(lag + 1 - HALF, span);
        for (i, sample) in out.iter_mut().enumerate() {
            let around = &span[i..i + TAPS];
            *sample = taps.iter().rev().zip(around).map(|(tap, x)| tap * x).sum();
        }
    }
}
