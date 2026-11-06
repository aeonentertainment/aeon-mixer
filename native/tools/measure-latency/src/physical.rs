/// How the signal gets from output to input, which is what the figure includes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Physical {
    Cable,
    Mic { air_cm: f64 },
}

impl Physical {
    /// What the sound spends in the air, in ms, which is not the output's to answer for.
    pub fn air_ms(&self) -> f64 {
        const CM_PER_MS: f64 = 34.3;
        match self {
            Self::Cable => 0.0,
            Self::Mic { air_cm } => air_cm / CM_PER_MS,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Cable => "cable from output left to input, stopping at the jack".into(),
            Self::Mic { air_cm } => format!(
                "mic {air_cm} cm from the speaker, {:.2} ms of air taken off",
                self.air_ms()
            ),
        }
    }
}
