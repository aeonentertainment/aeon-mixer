//! The OS's devices with no engine open, so a host whose default output will not start can choose
//! another. Listing opens no stream and changes no device setting.

use devices::System;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::clock::OutputEntry;
use crate::test_os::TestOs;

/// Every output the OS offers, the default marked, or the test OS's when it stands in.
#[napi]
pub fn outputs() -> Result<Vec<OutputEntry>> {
    TestOs::from_env()
        .map_or_else(devices::outputs, |test_os| test_os.outputs())
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
