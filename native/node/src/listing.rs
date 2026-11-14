//! The OS's devices with no engine open, so a host whose default output will not start can choose
//! another. Listing opens no stream and changes no device setting.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::clock::OutputEntry;

/// Every output the OS offers, the default marked.
#[napi]
pub fn outputs() -> Result<Vec<OutputEntry>> {
    devices::outputs()
        .map(|outputs| outputs.into_iter().map(OutputEntry::from).collect())
        .map_err(|error| Error::from_reason(error.to_string()))
}

/// Every input the OS offers, the default marked: the devices a `capture` slot may name.
#[napi]
pub fn inputs() -> Result<Vec<OutputEntry>> {
    devices::inputs()
        .map(|inputs| inputs.into_iter().map(OutputEntry::from).collect())
        .map_err(|error| Error::from_reason(error.to_string()))
}
