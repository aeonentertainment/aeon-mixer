use crate::reading::Reading;

/// One opening of a real output and input: what was asked, what the OS gave, and what came of it.
#[derive(Clone, Debug)]
pub struct Opening {
    pub asked: u32,
    pub output_period: u32,
    pub output_latency_frames: u32,
    pub input_period: u32,
    pub input_latency_frames: Option<u32>,
    pub output_opened: String,
    pub input_opened: String,
    pub underruns: u64,
    pub realtime: bool,
    /// The figures, or why there are none.
    pub reading: Result<Reading, String>,
}
