use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// Wakes the device follower. An OS listener raises it when the default output or the device list
/// changes, and `Stats::lose` when a stream's device goes. Raising never blocks, and wakes that
/// arrive before the follower looks fold into one, so a burst of notifications costs one look.
#[derive(Clone, Debug)]
pub struct Wake(SyncSender<()>);

impl Wake {
    /// A wake and the end the follower waits on.
    pub fn new() -> (Self, Receiver<()>) {
        let (sender, receiver) = sync_channel(1);
        (Self(sender), receiver)
    }

    pub fn raise(&self) {
        let _ = self.0.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_wakes_is_one() {
        let (wake, woken) = Wake::new();
        for _ in 0..3 {
            wake.clone().raise();
        }
        assert!(woken.try_recv().is_ok());
        assert!(woken.try_recv().is_err(), "the burst folded into one");
    }
}
