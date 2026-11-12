use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use engine::block::Stereo;
use engine::driver::{Movable, Render};

/// Lends a `Movable` to a stream as its `Render`, and takes it back when the stream drops the
/// render: when it stops, or when it fails to start. Every backend drops its render off the audio
/// thread, so the hand back never runs there. A render that panicked mid-block is torn and never
/// comes back.
pub struct Dock<M> {
    back: Receiver<M>,
}

impl<M: Movable> Dock<M> {
    pub fn lend(movable: M) -> (Render, Self) {
        let (back, dock) = sync_channel(1);
        let mut lent = Lent { movable: Some(movable), back, rendering: false };
        (Box::new(move |out| lent.render(out)), Self { back: dock })
    }

    /// The `Movable`, once the stream has let go of it.
    pub fn take(&self) -> Option<M> {
        self.back.try_recv().ok()
    }
}

struct Lent<M: Movable> {
    movable: Option<M>,
    back: SyncSender<M>,
    rendering: bool,
}

impl<M: Movable> Lent<M> {
    fn render(&mut self, out: Stereo<'_>) {
        let Some(movable) = &mut self.movable else { return };
        // Left set if the render unwinds, which is how `drop` knows it is torn
        self.rendering = true;
        movable.render(out);
        self.rendering = false;
    }
}

impl<M: Movable> Drop for Lent<M> {
    fn drop(&mut self) {
        if !self.rendering
            && let Some(movable) = self.movable.take()
        {
            let _ = self.back.try_send(movable);
        }
    }
}
