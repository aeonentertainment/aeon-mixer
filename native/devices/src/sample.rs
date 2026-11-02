/// A sample format a device takes. The engine renders f32; this is the one conversion at the edge.
pub trait Sample: Copy + Default {
    fn from_f32(value: f32) -> Self;
}

impl Sample for f32 {
    fn from_f32(value: f32) -> Self {
        value
    }
}

// Full scale, so a 24-bit device in a 32-bit container, which reads the high bytes, gets it too.
impl Sample for i32 {
    fn from_f32(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) * i32::MAX as f32) as i32
    }
}

impl Sample for i16 {
    fn from_f32(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
    }
}
