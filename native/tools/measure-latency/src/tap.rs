use std::time::Instant;

use engine::block::{BLOCK, Stereo};
use engine::driver::Render;
use rtrb::{Producer, RingBuffer};

use crate::clock_line::Wake;
use crate::recording::Recording;

/// Wraps a render so it keeps the left channel and notes when each block was rendered, in rings
/// sized for `frames` so the audio thread never allocates and nothing a run reads is lost. Blocks
/// past `frames` play but are not kept. The clock read stays in userspace, as `Stats::callback`
/// says of its own.
pub fn tap_render(mut render: Render, frames: usize, epoch: Instant) -> (Render, Recording) {
    let (mut samples, sample_out) = RingBuffer::new(frames);
    let (mut wakes, wake_out) = RingBuffer::new(frames / BLOCK + 1);
    let mut frame = 0u64;
    let tapped: Render = Box::new(move |out: Stereo<'_>| {
        let at_ns = epoch.elapsed().as_nanos() as u64;
        let Stereo { left, right } = out;
        render(Stereo { left: &mut *left, right: &mut *right });
        if frame + (BLOCK as u64) <= frames as u64 {
            push_all(&mut samples, left);
            let _ = wakes.push(Wake { frame, at_ns });
        }
        frame += BLOCK as u64;
    });
    (tapped, Recording { samples: sample_out, wakes: wake_out })
}

/// Pushes what fits. A full ring means the run is over, so the rest was never going to be read.
pub(crate) fn push_all(ring: &mut Producer<f32>, samples: &[f32]) {
    for sample in samples {
        if ring.push(*sample).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;
    use engine::driver::OfflineDriver;

    use super::*;
    use crate::ingest::Ingest;
    use crate::offline::pump;
    use crate::programme::Programme;
    use crate::rig::Rig;

    /// A render through the tap matches the render without it, frame for frame, and allocates
    /// nothing on the audio thread.
    #[test]
    fn the_tap_keeps_what_passes_and_allocates_nothing() {
        let programme = Programme::offline();
        let signal = programme.signal();
        let ingest = Ingest::L16 { frame: 240 };
        let plain = pump(Rig::new(ingest, 720, &signal, 0).unwrap(), signal.len());

        let Rig { render, controls: _controls, .. } = Rig::new(ingest, 720, &signal, 0).unwrap();
        let (tapped, mut recording) = tap_render(render, signal.len(), Instant::now());
        let mut driver = OfflineDriver::new(tapped);
        let mut block = [0.0f32; 2 * BLOCK];
        for _ in 0..signal.len().div_ceil(BLOCK) {
            assert_no_alloc(|| driver.pump_into(&mut block));
        }
        let (kept, wakes) = recording.take();
        let whole = signal.len() / BLOCK * BLOCK;
        assert_eq!(kept, plain[..whole]);
        assert_eq!(wakes.len(), whole / BLOCK);
        assert_eq!(wakes[3].frame, 3 * BLOCK as u64);
    }
}
