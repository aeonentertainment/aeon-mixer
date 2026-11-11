use engine::block::RATE;
use engine::ingest::SlotStats;
use engine::slot::effects::Corrector;
use napi_derive::napi;

/// `SlotStats` as JavaScript reads it.
#[napi(object)]
pub struct SlotStatsInfo {
    pub pushed: u32,
    pub lost: u32,
    pub late: u32,
    pub concealed: u32,
    pub resyncs: u32,
    pub jitter_ms: f64,
    pub corrector_delay_ms: f64,
    pub corrector_max_delay_ms: f64,
}

impl SlotStatsInfo {
    /// Both corrector fields read 0 with correction off, the state the delay's own 0 marks.
    pub fn new(stats: SlotStats, corrector_delay_frames: u32) -> Self {
        let max_frames = if corrector_delay_frames > 0 { Corrector::MAX_LATENCY_FRAMES } else { 0 };
        let ms = |frames: u32| f64::from(frames) * 1000.0 / f64::from(RATE);
        Self {
            pushed: stats.pushed,
            lost: stats.lost,
            late: stats.late,
            concealed: stats.concealed,
            resyncs: stats.resyncs,
            jitter_ms: f64::from(stats.jitter_ms),
            corrector_delay_ms: ms(corrector_delay_frames),
            corrector_max_delay_ms: ms(max_frames),
        }
    }
}
