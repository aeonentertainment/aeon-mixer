use engine::slot::tone_report::ToneFindings;
use napi::bindgen_prelude::Null;
use napi_derive::napi;

/// What a slot's tone stage did, as JavaScript reads it.
#[napi(object)]
pub struct ToneInfo {
    /// The blind match, which does not run until it has a target to aim at (decision 0029).
    #[napi(js_name = "match")]
    pub blind_match: Null,
    /// The most the de-esser took off in the last second.
    pub de_ess_db: f64,
    /// Pops the guard has ducked.
    pub plosives: u32,
}

impl From<ToneFindings> for ToneInfo {
    fn from(findings: ToneFindings) -> Self {
        Self {
            blind_match: Null,
            de_ess_db: f64::from(findings.de_ess_db),
            plosives: findings.plosives,
        }
    }
}
