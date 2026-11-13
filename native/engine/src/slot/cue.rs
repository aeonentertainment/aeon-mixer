/// What a slot's corrector follows, built by the graph once per block: the backing frame the
/// room hears now, less the engine's output latency, the backing's total key shift, and whether
/// the playhead moves on across this block. A held clock, before the start, paused or ended,
/// stays on its frame for the whole block, so a pause just short of a note edge never crosses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cue {
    pub frame: i64,
    pub semitones: i8,
    pub advancing: bool,
}
