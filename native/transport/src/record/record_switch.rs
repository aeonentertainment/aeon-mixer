//! The control thread's hold on one receiver's recording. The receiver is on the network thread,
//! so a new tap, or none, crosses in a channel the receiver reads before it services its socket
//! and on every tick: a recording starts or stops within the network thread's 5 ms tick.

use std::sync::mpsc;

use super::tap::Tap;

pub struct RecordSwitch(pub(crate) mpsc::Sender<Option<Tap>>);

impl RecordSwitch {
    /// Starts recording into `tap`, ending any recording before it, or ends the recording for
    /// `None`. A receiver already gone drops the tap, which ends that recording too.
    pub fn set(&self, tap: Option<Tap>) {
        let _ = self.0.send(tap);
    }
}
