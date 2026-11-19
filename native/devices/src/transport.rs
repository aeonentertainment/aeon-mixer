use std::fmt;

/// How a device reaches the machine, as its OS or driver says; decision 0025 maps each OS. A host
/// ranks a fallback output by it. The engine never does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    BuiltIn,
    Usb,
    Bluetooth,
    Display,
    Network,
    Aggregate,
    Virtual,
    Other,
    Unknown,
}

impl Transport {
    pub const ALL: [Self; 9] = [
        Self::BuiltIn,
        Self::Usb,
        Self::Bluetooth,
        Self::Display,
        Self::Network,
        Self::Aggregate,
        Self::Virtual,
        Self::Other,
        Self::Unknown,
    ];

    /// The TypeScript name, which the binding sends and the tools print.
    pub fn name(self) -> &'static str {
        match self {
            Self::BuiltIn => "builtIn",
            Self::Usb => "usb",
            Self::Bluetooth => "bluetooth",
            Self::Display => "display",
            Self::Network => "network",
            Self::Aggregate => "aggregate",
            Self::Virtual => "virtual",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
