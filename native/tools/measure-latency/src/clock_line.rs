/// A moment a stream's thread woke: how far the stream had got, on the host's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wake {
    pub frame: u64,
    pub at_ns: u64,
}

/// A stream's frame count against the host's clock, a straight line through its wakes. It is how
/// two streams on different hardware clocks are compared: each has its own line on the one clock
/// they share, and the slope takes up their drift.
#[derive(Clone, Copy, Debug)]
pub struct ClockLine {
    ns_per_frame: f64,
    at_frame_zero: f64,
}

impl ClockLine {
    /// The least-squares line. For an output: a frame's mean render time, however bursty the
    /// callbacks that rendered it.
    pub fn through(wakes: &[Wake]) -> Self {
        let count = wakes.len() as f64;
        let mean_frame = wakes.iter().map(|wake| wake.frame as f64).sum::<f64>() / count;
        let mean_ns = wakes.iter().map(|wake| wake.at_ns as f64).sum::<f64>() / count;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for wake in wakes {
            let frame = wake.frame as f64 - mean_frame;
            covariance += frame * (wake.at_ns as f64 - mean_ns);
            variance += frame * frame;
        }
        let ns_per_frame = if variance > 0.0 { covariance / variance } else { 0.0 };
        Self { ns_per_frame, at_frame_zero: mean_ns - ns_per_frame * mean_frame }
    }

    /// The line under every wake that lies closest to them on average. For an input: a thread
    /// wakes after the frames it receives were captured and never before, so its earliest wakes are
    /// the nearest any of them gets to the moment the frames came off the hardware.
    ///
    /// Lateness only ever adds, so it must not set the slope. A least-squares slope would tilt if
    /// the wakes grew later over a run, and date the early frames before they arrived. This is the
    /// edge of the wakes' lower convex hull at their mean frame, which is what minimising the total
    /// gap under the constraint comes to: Moon, Skelly and Towsley's estimate of a clock's skew from
    /// one-way delays (IEEE INFOCOM 1999).
    pub fn under(wakes: &[Wake]) -> Self {
        let Some(origin) = wakes.first().copied() else {
            return Self { ns_per_frame: 0.0, at_frame_zero: 0.0 };
        };
        // Relative to the first wake, so the hull's cross products keep their precision
        let mut points: Vec<(f64, f64)> = wakes
            .iter()
            .map(|wake| {
                (wake.frame as f64 - origin.frame as f64, wake.at_ns as f64 - origin.at_ns as f64)
            })
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        // Of two wakes on one frame only the earlier can bound it
        points.dedup_by(|later, earlier| later.0 == earlier.0);
        let mut hull: Vec<(f64, f64)> = Vec::with_capacity(points.len());
        for point in points {
            while let [.., before, last] = hull[..] {
                let turn = (last.0 - before.0) * (point.1 - before.1)
                    - (last.1 - before.1) * (point.0 - before.0);
                if turn > 0.0 {
                    break;
                }
                hull.pop();
            }
            hull.push(point);
        }
        let mean = wakes.iter().map(|wake| wake.frame as f64 - origin.frame as f64).sum::<f64>()
            / wakes.len() as f64;
        // Every wake on one frame leaves no slope to find: the line is flat through the earliest
        let Some([(x0, y0), (x1, y1)]) = hull
            .windows(2)
            .find(|edge| edge[1].0 >= mean)
            .or(hull.windows(2).last())
            .map(|edge| [edge[0], edge[1]])
        else {
            return Self { ns_per_frame: 0.0, at_frame_zero: origin.at_ns as f64 + hull[0].1 };
        };
        let ns_per_frame = (y1 - y0) / (x1 - x0);
        let at_origin = y0 - ns_per_frame * x0;
        Self {
            ns_per_frame,
            at_frame_zero: origin.at_ns as f64 + at_origin - ns_per_frame * origin.frame as f64,
        }
    }

    pub fn at(&self, frame: f64) -> f64 {
        self.at_frame_zero + self.ns_per_frame * frame
    }

    /// The frame this line reaches at `ns`, the inverse of `at`.
    pub fn frame_at(&self, ns: f64) -> f64 {
        (ns - self.at_frame_zero) / self.ns_per_frame
    }

    /// How far the wakes stand from the line, least and most, in nanoseconds.
    pub fn spread(&self, wakes: &[Wake]) -> (f64, f64) {
        wakes
            .iter()
            .map(|wake| wake.at_ns as f64 - self.at(wake.frame as f64))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), off| {
                (low.min(off), high.max(off))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device 100 ppm fast whose thread wakes up to 800 µs late, on a 3 ms period, and a few
    /// wakes much later than that. The line under them is the device's own clock to within 15 µs,
    /// under a sample, over a whole run.
    #[test]
    fn the_line_under_late_wakes_is_the_devices_clock() {
        let ns_per_frame = 1e9 / 48_000.0 / 1.0001;
        let truth = |frame: u64| 5e8 + frame as f64 * ns_per_frame;
        let mut state = 3u64;
        let wakes: Vec<Wake> = (1..2000u64)
            .map(|k| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let late = (state >> 40) as f64 / (1u64 << 24) as f64 * 800_000.0;
                let spike = if k % 500 == 0 { 8_000_000.0 } else { 0.0 };
                let frame = k * 144;
                Wake { frame, at_ns: (truth(frame) + late + spike) as u64 }
            })
            .collect();
        let line = ClockLine::under(&wakes);
        for frame in [144.0, 144_000.0, 287_856.0] {
            let error = line.at(frame) - truth(frame as u64);
            assert!(error.abs() < 15_000.0, "at frame {frame} the line is off by {error} ns");
            assert!((line.frame_at(line.at(frame)) - frame).abs() < 1e-3);
        }
    }

    /// A capture thread that starts waking a millisecond later for the last fifth of a run, with
    /// no frame lost. The line under the wakes keeps the device's clock to within 15 µs at both
    /// ends, so no frame is dated before it arrived and `output at most` stays a bound. A
    /// least-squares slope tilts here, and moves the early frames hundreds of microseconds early.
    #[test]
    fn wakes_that_grow_later_do_not_tilt_the_line_under_them() {
        let ns_per_frame = 1e9 / 48_000.0;
        let truth = |frame: u64| 2e8 + frame as f64 * ns_per_frame;
        let mut state = 5u64;
        let wakes: Vec<Wake> = (1..2400u64)
            .map(|k| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let jitter = (state >> 40) as f64 / (1u64 << 24) as f64 * 200_000.0;
                let late = if k > 1920 { 1_000_000.0 } else { 0.0 };
                let frame = k * 120;
                Wake { frame, at_ns: (truth(frame) + jitter + late) as u64 }
            })
            .collect();
        let line = ClockLine::under(&wakes);
        for frame in [120u64, 120_000, 287_880] {
            let error = line.at(frame as f64) - truth(frame);
            assert!((-1.0..15_000.0).contains(&error), "at frame {frame}: {error} ns");
        }
        let fitted = ClockLine::through(&wakes);
        let tilt = (fitted.at(287_880.0) - fitted.at(120.0)) - (truth(287_880) - truth(120));
        assert!(tilt > 300_000.0, "least squares tilts by {tilt} ns, so this case is a real one");
    }

    /// A 480-frame callback renders its 128-frame blocks in a burst. The line through them still
    /// runs at the device's rate and through their mean, and the spread shows the burst.
    #[test]
    fn the_line_through_a_burst_is_the_mean_render_time() {
        let ns_per_frame = 1e9 / 48_000.0;
        let wakes: Vec<Wake> = (0..4000u64)
            .map(|block| {
                let frame = block * 128;
                let callback = frame / 480;
                Wake { frame, at_ns: (callback as f64 * 480.0 * ns_per_frame) as u64 }
            })
            .collect();
        let line = ClockLine::through(&wakes);
        let drift = (line.at(1000.0 * 128.0) - line.at(0.0)) / (1000.0 * 128.0) - ns_per_frame;
        assert!(drift.abs() < 1.0, "the slope is off by {drift} ns a frame");
        let (low, high) = line.spread(&wakes);
        assert!((high - low - 8.0e6).abs() < 2.0e6, "the burst spans {} ms", (high - low) / 1e6);
    }
}
