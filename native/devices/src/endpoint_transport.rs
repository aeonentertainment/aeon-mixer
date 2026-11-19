//! A Windows endpoint's transport, by decision 0025's Windows table. Pure, so every OS tests it.

use crate::transport::Transport;

// EndpointFormFactor in mmdeviceapi.h
const REMOTE_NETWORK_DEVICE: u32 = 0;
const DIGITAL_AUDIO_DISPLAY_DEVICE: u32 = 9;

/// `form_factor` is the endpoint's `PKEY_AudioEndpoint_FormFactor` and `enumerator` its adapter's
/// `DEVPKEY_Device_EnumeratorName`, each None when it could not be read. The form factor comes
/// first, because a display hangs off `HDAUDIO` on most machines.
pub fn endpoint_transport(form_factor: Option<u32>, enumerator: Option<&str>) -> Transport {
    match form_factor {
        Some(DIGITAL_AUDIO_DISPLAY_DEVICE) => return Transport::Display,
        Some(REMOTE_NETWORK_DEVICE) => return Transport::Network,
        _ => {}
    }
    // Device instance ids are case-insensitive
    match enumerator.map(str::to_ascii_uppercase).as_deref() {
        Some("ROOT") => Transport::Virtual,
        Some(bus) if bus.starts_with("BTH") => Transport::Bluetooth,
        Some("USB") => Transport::Usb,
        Some("HDAUDIO" | "INTELAUDIO") => Transport::BuiltIn,
        Some("PCI") => Transport::Other,
        // `SWD` may be either, and a guess of hardware is what the field exists to prevent
        _ => Transport::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEAKERS: Option<u32> = Some(1);

    #[test]
    fn the_form_factor_says_display_or_network_whatever_the_bus() {
        let display = Some(DIGITAL_AUDIO_DISPLAY_DEVICE);
        assert_eq!(endpoint_transport(display, Some("HDAUDIO")), Transport::Display);
        let network = Some(REMOTE_NETWORK_DEVICE);
        assert_eq!(endpoint_transport(network, None), Transport::Network);
    }

    #[test]
    fn otherwise_the_adapters_enumerator_says_it() {
        let table = [
            ("ROOT", Transport::Virtual),
            ("BTHENUM", Transport::Bluetooth),
            ("BTHHFENUM", Transport::Bluetooth),
            ("BTHLEDEVICE", Transport::Bluetooth),
            ("USB", Transport::Usb),
            ("usb", Transport::Usb),
            ("HDAUDIO", Transport::BuiltIn),
            ("INTELAUDIO", Transport::BuiltIn),
            ("PCI", Transport::Other),
            ("SWD", Transport::Unknown),
            ("", Transport::Unknown),
        ];
        for (enumerator, transport) in table {
            assert_eq!(endpoint_transport(SPEAKERS, Some(enumerator)), transport, "{enumerator}");
        }
    }

    #[test]
    fn an_endpoint_whose_properties_cannot_be_read_is_unknown() {
        assert_eq!(endpoint_transport(None, None), Transport::Unknown);
        assert_eq!(endpoint_transport(SPEAKERS, None), Transport::Unknown);
    }
}
