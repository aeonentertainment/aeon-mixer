use rtrb::Consumer;

use crate::clock_line::Wake;

/// The far end of a tap: what a stream delivered, on the audio thread's rings, for the control
/// thread to take once the stream has stopped.
pub struct Recording {
    pub(crate) samples: Consumer<f32>,
    pub(crate) wakes: Consumer<Wake>,
}

impl Recording {
    /// Everything recorded, oldest first.
    pub fn take(&mut self) -> (Vec<f32>, Vec<Wake>) {
        (drain(&mut self.samples), drain(&mut self.wakes))
    }
}

fn drain<T>(ring: &mut Consumer<T>) -> Vec<T> {
    std::iter::from_fn(|| ring.pop().ok()).collect()
}
