/// What a slot's tone stage did, for `stats()`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ToneFindings {
    /// The most the de-esser took off in the last second.
    pub de_ess_db: f32,
    /// Pops the guard has ducked since the slot was built.
    pub plosives: u32,
}
