use crate::block::RATE;

/// The share of the remaining distance a one-pole smoother covers each sample, for a time
/// constant of `ms`: after `ms` it has covered 63 % of a step. The gate and the compressor build
/// every attack and release from it.
pub fn one_pole(ms: f32) -> f32 {
    1.0 - (-1000.0 / (ms * RATE as f32)).exp()
}

/// `db` decibels as a linear amplitude factor.
pub fn amplitude(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// A mean square in decibels, with silence at a floor instead of minus infinity.
pub fn power_db(mean_square: f32) -> f32 {
    10.0 * mean_square.max(1e-12).log10()
}

/// Eases `block` from a stage's last gain, `from`, to unity across the block: how a gain stage
/// turned off lets go without a click, before it stops touching the voice.
pub fn fade_to_unity(from: f32, block: &mut [f32]) {
    let step = (1.0 - from) / block.len().max(1) as f32;
    let mut gain = from;
    for sample in block {
        gain += step;
        *sample *= gain;
    }
}
