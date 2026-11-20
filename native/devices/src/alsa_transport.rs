//! An ALSA `hw:` device's transport, by decision 0025's Linux table. Pure, so every OS tests it.

use crate::transport::Transport;

/// `driver` is the card's driver string, as `snd_ctl_card_info_get_driver` gives it, or None when
/// it could not be read. `pcm_id` is the pcm's id, as `snd_pcm_info_get_id` gives it: `HDMI 0`.
pub fn alsa_transport(driver: Option<&str>, pcm_id: &str) -> Transport {
    match driver {
        // The drivers in the kernel's `sound/drivers/` with no hardware behind them
        Some("Loopback" | "Dummy" | "PCM-TEST Driver") => Transport::Virtual,
        Some("USB-Audio") => Transport::Usb,
        Some("vc4-hdmi") => Transport::Display,
        // A codec's HDMI and DisplayPort pins, on a GPU's card or the chipset's, or SOF's on a laptop
        _ if is_hdmi(pcm_id) => Transport::Display,
        Some("HDA-Intel") => Transport::BuiltIn,
        // A kernel driver that makes a pcm card is hardware, a Raspberry Pi's headphone jack among them
        Some(_) => Transport::Other,
        None => Transport::Unknown,
    }
}

// HD-Audio's `HDMI 0`, or an ASoC front end's `HDMI1 (*)`, as SOF's topology names its pcms
fn is_hdmi(pcm_id: &str) -> bool {
    let id = pcm_id.strip_suffix(" (*)").unwrap_or(pcm_id);
    id.strip_prefix("HDMI")
        .map(|index| index.strip_prefix(' ').unwrap_or(index))
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
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

    // SOF's topology names the pcms (hdmi-generic.conf:156 in thesofproject/sof), ASoC adds the
    // ` (*)` (soc-pcm.c:2924), and the card driver is `sof-` and the board's name (soc-card.c:257)
    #[test]
    fn an_sof_laptops_hdmi_pcms_are_displays_and_its_others_are_other() {
        for pcm_id in ["HDMI1 (*)", "HDMI2 (*)", "HDMI3 (*)", "HDMI4 (*)"] {
            assert_eq!(alsa_transport(Some("sof-hda-dsp"), pcm_id), Transport::Display, "{pcm_id}");
        }
        assert_eq!(alsa_transport(Some("sof-soundwire"), "HDMI1 (*)"), Transport::Display);
        assert_eq!(alsa_transport(Some("sof-hda-dsp"), "HDA Analog (*)"), Transport::Other);
        assert_eq!(alsa_transport(Some("sof-hda-dsp"), "HDMI (*)"), Transport::Other);
    }

    #[test]
    fn a_card_whose_driver_cannot_be_read_is_unknown() {
        assert_eq!(alsa_transport(None, "ALC3246 Analog"), Transport::Unknown);
    }
}
