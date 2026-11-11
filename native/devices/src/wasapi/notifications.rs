use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    DEVICE_STATE, EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl, eConsole,
    eRender,
};
use windows::core::{PCWSTR, Result, implement};

use crate::wake::Wake;

/// The endpoint notifications the follower needs, as a COM object MMDevice calls on a thread of
/// its own. Each raises the wake and returns: a callback must not register or unregister
/// callbacks, nor release the last reference to an MMDevice object.
#[implement(IMMNotificationClient)]
pub struct Notifications {
    wake: Wake,
}

impl Notifications {
    pub fn client(wake: Wake) -> IMMNotificationClient {
        Self { wake }.into()
    }
}

impl IMMNotificationClient_Impl for Notifications_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        self.wake.raise();
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        self.wake.raise();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        self.wake.raise();
        Ok(())
    }

    // Called once for each role; the console role's render default is the one the engine plays on
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eConsole {
            self.wake.raise();
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}
