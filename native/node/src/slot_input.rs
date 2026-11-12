use napi_derive::napi;

/// One slot as `createMixer` asks for it. `device` and `channel` belong to a `capture` slot: an id
/// from `inputs`, or the default input, and the device's channel from 0.
#[napi(object)]
pub struct SlotInput {
    pub kind: String,
    pub device: Option<String>,
    pub channel: Option<u32>,
}
