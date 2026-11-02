/// One detector reading, every 10 ms, placed at its window's centre on the guide's clock. The
/// slot turns it into `Event::Pitch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub position_ms: f64,
    pub hz: f32,
    pub confidence: f32,
}
