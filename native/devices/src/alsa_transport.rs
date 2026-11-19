//! An ALSA `hw:` device's transport, by decision 0025's Linux table. Pure, so every OS tests it.

use crate::transport::Transport;

/// `driver` is the card's driver string, as `snd_ctl_card_info_get_driver` gives it, or None when
/// it could not be read. `pcm` is the device's pcm name, as `HDMI 0`.
pub fn alsa_transport(driver: Option<&str>, pcm: &str) -> Transport {
    match driver {
        // The drivers in the kernel's `sound/drivers/` with no hardware behind them
        Some("Loopback" | "Dummy" | "PCM-TEST Driver") => Transport::Virtual,
        Some("USB-Audio") => Transport::Usb,
        Some("vc4-hdmi") => Transport::Display,
        // HD-Audio names a codec's HDMI and DisplayPort pins `HDMI n`, on a GPU's card or the chipset's
        _ if is_hdmi(pcm) => Transport::Display,
        Some("HDA-Intel") => Transport::BuiltIn,
        // A kernel driver that makes a pcm card is hardware, a Raspberry Pi's headphone jack among them
        Some(_) => Transport::Other,
        None => Transport::Unknown,
    }
}

fn is_hdmi(pcm: &str) -> bool {
    pcm.strip_prefix("HDMI ").is_some_and(|index| index.parse::<u32>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_card_driver_maps_as_decision_0025_says() {
        let table = [
            ("Loopback", Transport::Virtual),
            ("Dummy", Transport::Virtual),
            ("PCM-TEST Driver", Transport::Virtual),
            ("USB-Audio", Transport::Usb),
            ("vc4-hdmi", Transport::Display),
            ("HDA-Intel", Transport::BuiltIn),
            ("bcm2835_headpho", Transport::Other),
        ];
        for (driver, transport) in table {
            assert_eq!(alsa_transport(Some(driver), "ALC3246 Analog"), transport, "{driver}");
        }
    }

    #[test]
    fn an_hdmi_pcm_is_a_display_on_any_card_but_a_virtual_or_usb_one() {
        assert_eq!(alsa_transport(Some("HDA-Intel"), "HDMI 0"), Transport::Display);
        assert_eq!(alsa_transport(Some("HDA-Intel"), "HDMI 3"), Transport::Display);
        assert_eq!(alsa_transport(None, "HDMI 0"), Transport::Display);
        assert_eq!(alsa_transport(Some("USB-Audio"), "HDMI 0"), Transport::Usb);
        assert_eq!(alsa_transport(Some("HDA-Intel"), "HDMI Out"), Transport::BuiltIn);
    }

    #[test]
    fn a_card_whose_driver_cannot_be_read_is_unknown() {
        assert_eq!(alsa_transport(None, "ALC3246 Analog"), Transport::Unknown);
    }
}
