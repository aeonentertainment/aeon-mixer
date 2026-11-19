mod asound;
mod capture;
mod direction;
mod driver;
mod format;
mod pcm;

use std::ffi::{CStr, CString, c_void};
use std::ptr::null_mut;

use engine::driver::DriverError;

pub use capture::AlsaCapture;
pub use driver::AlsaDriver;

use crate::alsa_transport::alsa_transport;
use crate::output::Output;
use asound::{Asound, CTL_READONLY, STREAM_PLAYBACK, asound};

/// The hardware playback devices, `hw:CARD=...,DEV=...`. The first is the default: card 0's
/// first device, which is where ALSA's own default points on a box with no sound server.
pub fn outputs() -> Result<Vec<Output>, DriverError> {
    devices("Input")
}

/// The hardware capture devices, listed the way `outputs` lists playback ones. A laptop's digital
/// mics are usually not the first of them, so name the one you mean.
pub fn inputs() -> Result<Vec<crate::Input>, DriverError> {
    devices("Output")
}

// Every hw: device but those that run only `other_way`. A Linux with no libasound has no ALSA
// device to list, so it lists none, and an open there finds no output rather than failing to look.
fn devices(other_way: &str) -> Result<Vec<Output>, DriverError> {
    let Ok(lib) = asound() else { return Ok(Vec::new()) };
    let mut hints = null_mut();
    // SAFETY: `hints` receives a null-terminated array freed below.
    lib.check(
        unsafe { (lib.snd_device_name_hint)(-1, c"pcm".as_ptr(), &mut hints) },
        "list devices",
    )?;
    let mut outputs = Vec::new();
    let mut at = hints;
    // SAFETY: the array ends at the first null entry, and each entry is a hint.
    while let Some(hint) = unsafe { (*at).as_ref() } {
        let hint: *const c_void = hint;
        let name = take_hint(lib, hint, c"NAME");
        let direction = take_hint(lib, hint, c"IOID");
        let description = take_hint(lib, hint, c"DESC").unwrap_or_default();
        if let Some(id) = name.filter(|name| name.starts_with("hw:"))
            && direction.as_deref() != Some(other_way)
        {
            let (driver, pcm_id) = card_and_pcm(lib, &id);
            let transport =
                alsa_transport(driver.as_deref(), pcm_id.as_deref().unwrap_or_default());
            let name = label(&id, &description);
            outputs.push(Output { is_default: outputs.is_empty(), id, name, transport });
        }
        // SAFETY: still inside the array: this entry was not the terminator.
        at = unsafe { at.add(1) };
    }
    // SAFETY: allocated by snd_device_name_hint above.
    unsafe { (lib.snd_device_name_free_hint)(hints) };
    Ok(outputs)
}

fn default_input() -> Result<String, DriverError> {
    inputs()?
        .into_iter()
        .find(|input| input.is_default)
        .map(|input| input.id)
        .ok_or(DriverError::NoDevice)
}

fn default_output() -> Result<String, DriverError> {
    outputs()?
        .into_iter()
        .find(|output| output.is_default)
        .map(|output| output.id)
        .ok_or(DriverError::NoDevice)
}

// The driver string the kernel gave `id`'s card and the id it gave the pcm, read through the
// card's control device: queries that open no pcm. The id, not the name, because ASoC names no
// pcm: an SOF laptop's HDMI pcm has the id `HDMI1 (*)` and an empty name. The id is asked of the
// playback stream, the one an HDMI pcm has, so a capture-only pcm's reads as None.
fn card_and_pcm(lib: &Asound, id: &str) -> (Option<String>, Option<String>) {
    let Some((card, device)) = id.split_once(",DEV=") else { return (None, None) };
    let (Ok(card), Ok(device)) = (CString::new(card), device.parse()) else { return (None, None) };
    let mut ctl = null_mut();
    // SAFETY: `ctl` and both infos are opened and allocated here and freed before return, and each
    // string is copied out of its info before the info is freed.
    unsafe {
        if (lib.snd_ctl_open)(&mut ctl, card.as_ptr(), CTL_READONLY) < 0 {
            return (None, None);
        }
        let mut driver = None;
        let mut card_info = null_mut();
        if (lib.snd_ctl_card_info_malloc)(&mut card_info) >= 0 {
            if (lib.snd_ctl_card_info)(ctl, card_info) >= 0 {
                let name = CStr::from_ptr((lib.snd_ctl_card_info_get_driver)(card_info));
                driver = Some(name.to_string_lossy().into_owned());
            }
            (lib.snd_ctl_card_info_free)(card_info);
        }
        let mut pcm_id = None;
        let mut pcm_info = null_mut();
        if (lib.snd_pcm_info_malloc)(&mut pcm_info) >= 0 {
            (lib.snd_pcm_info_set_device)(pcm_info, device);
            (lib.snd_pcm_info_set_stream)(pcm_info, STREAM_PLAYBACK);
            if (lib.snd_ctl_pcm_info)(ctl, pcm_info) >= 0 {
                let pcm = CStr::from_ptr((lib.snd_pcm_info_get_id)(pcm_info));
                pcm_id = Some(pcm.to_string_lossy().into_owned());
            }
            (lib.snd_pcm_info_free)(pcm_info);
        }
        (lib.snd_ctl_close)(ctl);
        (driver, pcm_id)
    }
}

// ALSA describes a device as "card, pcm name", and many cards leave the analog pcm unnamed.
fn label(id: &str, description: &str) -> String {
    let label = description.lines().next().unwrap_or_default().trim_end_matches([',', ' ']);
    match (label.contains(", "), id.rsplit_once("DEV=")) {
        (false, Some((_, device))) => format!("{label}, device {device}"),
        _ => label.to_owned(),
    }
}

fn take_hint(lib: &Asound, hint: *const c_void, field: &CStr) -> Option<String> {
    // SAFETY: `hint` is a live hint; the result is null or a malloc'd string we free.
    unsafe {
        let value = (lib.snd_device_name_get_hint)(hint, field.as_ptr());
        if value.is_null() {
            return None;
        }
        let text = CStr::from_ptr(value).to_string_lossy().into_owned();
        libc::free(value.cast());
        Some(text)
    }
}
