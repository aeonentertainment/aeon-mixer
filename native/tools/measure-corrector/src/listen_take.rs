/// One of the three renderings of a source a listener ranks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Take {
    Off,
    Key,
    Melody,
    /// The melody take as the build before a change rendered it, read back from `listen --export`.
    Before,
}

impl Take {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Key => "key",
            Self::Melody => "melody",
            Self::Before => "before",
        }
    }
}
