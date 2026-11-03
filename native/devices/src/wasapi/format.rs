use engine::block::RATE;
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0};
use windows::Win32::Media::KernelStreaming::{
    KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, WAVE_FORMAT_EXTENSIBLE,
};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};

/// A stereo 48 kHz format for exclusive mode, by sample type and valid bits.
#[derive(Clone, Copy, Debug)]
pub enum Format {
    F32,
    I32,
    /// 24 valid bits, high-aligned in a 32-bit container, so it takes i32 samples.
    I24In32,
    I16,
}

impl Format {
    /// What exclusive mode asks the device for, best first.
    pub const PREFERRED: [Format; 4] = [Format::F32, Format::I32, Format::I24In32, Format::I16];

    pub fn name(self) -> &'static str {
        match self {
            Format::F32 => "f32",
            Format::I32 => "s32",
            Format::I24In32 => "s24 in 32",
            Format::I16 => "s16",
        }
    }

    pub fn wave(self) -> WAVEFORMATEXTENSIBLE {
        let (bits, valid) = match self {
            Format::F32 | Format::I32 => (32, 32),
            Format::I24In32 => (32, 24),
            Format::I16 => (16, 16),
        };
        let block = 2 * bits / 8;
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: 2,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * u32::from(block),
                nBlockAlign: block,
                wBitsPerSample: bits,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: valid },
            dwChannelMask: SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
            SubFormat: match self {
                Format::F32 => KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                _ => KSDATAFORMAT_SUBTYPE_PCM,
            },
        }
    }
}

/// Whether a shared-mode mix format is what the engine renders, f32 at 48 kHz, so the shared
/// engine takes it with no conversion.
///
/// # Safety
/// `format` must point to a valid WAVEFORMATEX, and to a WAVEFORMATEXTENSIBLE when its tag says so.
pub unsafe fn is_float_48k(format: *const WAVEFORMATEX) -> bool {
    // SAFETY: the caller's promise. The structs are packed, so they are read unaligned.
    let head = unsafe { format.read_unaligned() };
    let float = match u32::from(head.wFormatTag) {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_EXTENSIBLE => {
            let full = unsafe { format.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() };
            let sub = full.SubFormat;
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        }
        _ => false,
    };
    float && head.wBitsPerSample == 32 && head.nSamplesPerSec == RATE
}
