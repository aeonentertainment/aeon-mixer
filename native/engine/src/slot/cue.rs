/// What a slot's corrector follows, built by the graph once per block: the backing frame the
/// room hears now, less the engine's output latency, and the backing's total key shift.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cue {
    pub frame: i64,
    pub semitones: i8,
}
