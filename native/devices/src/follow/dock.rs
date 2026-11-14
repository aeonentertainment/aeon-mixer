use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use engine::block::Stereo;
use engine::driver::{Movable, Render};

use super::handoff::Handoff;

/// Lends a `Movable` to a stream as its `Render`, and takes it back when the stream drops the
/// render: when it stops, or when it fails to start. Every backend drops its render off the audio
/// thread, so the hand back never runs there. A render that panicked mid-block is torn and never
/// comes back. Until the stream first renders, the follower may take the `Movable` back from a
/// start that hangs, and the render plays silence from then on.
pub struct Dock<M> {
    unrendered: Arc<Handoff<M>>,
    back: Receiver<M>,
}

impl<M: Movable> Dock<M> {
    pub fn lend(movable: M) -> (Render, Self) {
        let unrendered = Arc::new(Handoff::new(movable));
        let (back, dock) = sync_channel(1);
        let mut lent =
            Lent { unrendered: Arc::clone(&unrendered), movable: None, back, rendering: false };
        (Box::new(move |out| lent.render(out)), Self { unrendered, back: dock })
    }

    /// The `Movable`, once the stream has let go of it or while it has never rendered.
    pub fn take(&self) -> Option<M> {
        self.back.try_recv().ok().or_else(|| self.unrendered.take().map(|movable| *movable))
    }
}

struct Lent<M: Movable> {
    unrendered: Arc<Handoff<M>>,
    movable: Option<Box<M>>,
    back: SyncSender<M>,
    rendering: bool,
}

impl<M: Movable> Lent<M> {
    fn render(&mut self, out: Stereo<'_>) {
        if self.movable.is_none() {
            self.movable = self.unrendered.take();
        }
        let Some(movable) = &mut self.movable else {
            out.left.fill(0.0);
            out.right.fill(0.0);
            return;
        };
        // Left set if the render unwinds, which is how `drop` knows it is torn
        self.rendering = true;
        movable.render(out);
        self.rendering = false;
    }
}

impl<M: Movable> Drop for Lent<M> {
    fn drop(&mut self) {
        if !self.rendering
            && let Some(movable) = self.movable.take().or_else(|| self.unrendered.take())
        {
            let _ = self.back.try_send(*movable);
        }
    }
}
