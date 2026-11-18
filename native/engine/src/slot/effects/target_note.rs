/// The guide note the corrector pulls toward, after the key shift: its pitch, and its first frame
/// on the guide's clock, which tells two notes in a row on one pitch apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetNote {
    pub midi: i16,
    pub start_frame: u64,
}
