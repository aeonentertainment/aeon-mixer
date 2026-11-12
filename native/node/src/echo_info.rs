use engine::block::RATE;
use engine::slot::echo_report::EchoFindings;
use napi_derive::napi;

/// What a slot's echo canceller has found, as JavaScript reads it.
#[napi(object)]
pub struct EchoInfo {
    /// From the master being rendered to its echo reaching the slot.
    pub path_ms: f64,
    /// How far under the master the echo arrives, over the last second.
    pub bleed_db: f64,
    /// How much quieter the slot came out than it went in, over the last second.
    pub removed_db: f64,
}

impl From<EchoFindings> for EchoInfo {
    fn from(findings: EchoFindings) -> Self {
        Self {
            path_ms: f64::from(findings.path_frames) * 1000.0 / f64::from(RATE),
            bleed_db: f64::from(findings.bleed_db),
            removed_db: f64::from(findings.removed_db),
        }
    }
}
