use std::fmt;

use engine::block::{BLOCK, RATE};

/// Which encoder a phone runs. The engine decodes both the same; what differs is the lookahead
/// the sender pays before a packet leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    /// What mediasoup and the fixtures send: SILK or hybrid, 6.5 ms of lookahead.
    Voip,
    /// What aeon-karaoke#335's native module would send: CELT alone, 2.5 ms of lookahead.
    RestrictedLowDelay,
}

/// One way audio reaches a slot, with the sender's chunk of it in frames. Docs/design.md 8.4 is
/// the engine's side of each; `Capture` is a house mic on this machine, on the output's clock. A
/// new ingest kind is one more variant here, and it gets a row of the table and a guard on its
/// number from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingest {
    Rtp { frame: usize, encoder: Encoder },
    L16 { frame: usize },
    Push { chunk: usize },
    Capture { period: usize },
}

impl Ingest {
    /// Every path the engine takes today: each Opus frame it accepts, what the house-mic sender
    /// sends, a host handing over 10 ms chunks, which section 8.4 assumes, and a house mic captured
    /// a block at a time.
    pub fn every() -> [Self; 9] {
        let voip = |frame| Self::Rtp { frame, encoder: Encoder::Voip };
        let low_delay = |frame| Self::Rtp { frame, encoder: Encoder::RestrictedLowDelay };
        [
            voip(960),
            voip(480),
            voip(240),
            voip(120),
            low_delay(240),
            low_delay(120),
            Self::L16 { frame: HOUSE_MIC_FRAME },
            Self::Push { chunk: RATE as usize / 100 },
            Self::Capture { period: BLOCK },
        ]
    }

    /// Frames the sender gathers before any of them can leave.
    pub fn gather_frames(&self) -> usize {
        match *self {
            Self::Rtp { frame, .. } | Self::L16 { frame } => frame,
            Self::Push { chunk } => chunk,
            Self::Capture { period } => period,
        }
    }

    /// Frames from the stream's start within which the slot plays it. A capture slot plays silence
    /// while it primes its ring and reads the ring's trough over a window, 0.35 s together, so a
    /// chirp sent sooner would come out cut; every other slot plays the first packet it can.
    pub fn settles_within(&self) -> usize {
        match self {
            Self::Capture { .. } => RATE as usize / 2,
            _ => 0,
        }
    }

    /// Whether the engine's jitter buffer, and so its playout delay, is on this path.
    pub fn has_playout_delay(&self) -> bool {
        matches!(self, Self::Rtp { .. } | Self::L16 { .. })
    }
}

/// `rtpL16pay` at GStreamer's default 1400-byte MTU: 350 stereo 16-bit frames, 7.3 ms.
pub const HOUSE_MIC_FRAME: usize = 350;

impl fmt::Display for Ingest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = f64::from(self.gather_frames() as u32) * 1000.0 / f64::from(RATE);
        match self {
            Self::Rtp { encoder: Encoder::Voip, .. } => write!(f, "rtp {ms} ms"),
            Self::Rtp { encoder: Encoder::RestrictedLowDelay, .. } => {
                write!(f, "rtp {ms} ms low-delay")
            }
            Self::L16 { .. } => write!(f, "l16 {ms:.1} ms"),
            Self::Push { .. } => write!(f, "push {ms} ms"),
            Self::Capture { .. } => write!(f, "capture {ms:.1} ms"),
        }
    }
}
