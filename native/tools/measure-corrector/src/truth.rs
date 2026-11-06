/// Human-checked f0, for scoring a detector.
pub struct Truth {
    rows: Vec<(f64, f64)>,
}

impl Truth {
    pub fn new(rows: Vec<(f64, f64)>) -> Self {
        Self { rows }
    }

    /// The annotation row at or after `ms`, as the harness's scorer picks it; 0 where unvoiced.
    pub fn hz_at(&self, ms: f64) -> f64 {
        let index = self.rows.partition_point(|(seconds, _)| seconds * 1000.0 < ms);
        self.rows.get(index).or(self.rows.last()).map_or(0.0, |(_, hz)| *hz)
    }
}
