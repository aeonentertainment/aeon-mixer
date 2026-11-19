//! CoreAudio's transport type as a `Transport`, by decision 0025's macOS table. Pure, so every OS
//! tests it.

use crate::transport::Transport;

/// `code` is a device's `kAudioDevicePropertyTransportType`, a four-character code. A read that
/// fails is `Transport::Unknown` without calling this.
pub fn core_audio_transport(code: u32) -> Transport {
    match &code.to_be_bytes() {
        b"bltn" => Transport::BuiltIn,
        b"usb " => Transport::Usb,
        b"blue" | b"blea" => Transport::Bluetooth,
        b"hdmi" | b"dprt" => Transport::Display,
        b"airp" => Transport::Network,
        b"grup" => Transport::Aggregate,
        b"virt" => Transport::Virtual,
        b"pci " | b"1394" | b"thun" | b"eavb" | b"ccap" | b"ccwd" | b"ccwl" => Transport::Other,
        // `Unknown` (0), the deprecated `AutoAggregate` (`fgrp`), and whatever a later macOS adds
        _ => Transport::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(fourcc: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*fourcc)
    }

    #[test]
    fn every_constant_in_the_headers_maps_as_decision_0025_says() {
        let table = [
            (b"bltn", Transport::BuiltIn),
            (b"usb ", Transport::Usb),
            (b"blue", Transport::Bluetooth),
            (b"blea", Transport::Bluetooth),
            (b"hdmi", Transport::Display),
            (b"dprt", Transport::Display),
            (b"airp", Transport::Network),
            (b"grup", Transport::Aggregate),
            (b"virt", Transport::Virtual),
            (b"pci ", Transport::Other),
            (b"1394", Transport::Other),
            (b"thun", Transport::Other),
            (b"eavb", Transport::Other),
            (b"ccap", Transport::Other),
            (b"ccwd", Transport::Other),
            (b"ccwl", Transport::Other),
            (b"fgrp", Transport::Unknown),
        ];
        for (fourcc, transport) in table {
            assert_eq!(core_audio_transport(code(fourcc)), transport, "{fourcc:?}");
        }
        assert_eq!(core_audio_transport(0), Transport::Unknown);
    }
}
