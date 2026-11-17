use crate::slot::effects::tone::band::Band;

/// How far one band of the voice stands over another, in dB: what the guard and the de-esser
/// each cut by, so a level judged against the voice's own needs no threshold per phone.
pub struct BandRatio {
    band: Band,
    against: Band,
}

impl BandRatio {
    pub fn new(band: Band, against: Band) -> Self {
        Self { band, against }
    }

    pub fn stands_db(&mut self, x: f64) -> f32 {
        self.band.level_db(x) - self.against.level_db(x)
    }
}
