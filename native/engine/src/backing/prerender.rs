use std::sync::atomic::{AtomicU32, Ordering};

use kernels::{Preset, Stretch};

use crate::backing::{Planar, Stems};
use crate::block::RATE;

/// Input frames per stretcher call: long enough that a call's overhead vanishes, short enough
/// that progress moves smoothly.
const CHUNK: usize = 4096;

/// The stems at `speed` times their tempo and moved by `semitones`, through the stretcher at its
/// best quality. Both stems go through one four-channel stretcher, so they stay sample-aligned.
/// Writes the fraction done to `progress` as `f32` bits. Runs on the worker thread.
pub fn prerender(stems: Stems, semitones: i8, speed: f32, progress: &AtomicU32) -> Stems {
    let mut inputs: Vec<&[f32]> = vec![&stems.instrumental.left, &stems.instrumental.right];
    if let Some(guide) = &stems.guide {
        inputs.extend([guide.left.as_slice(), guide.right.as_slice()]);
    }
    let frames = stems.frames as usize;
    let speed = f64::from(speed);
    let mut stretch = Stretch::new(inputs.len(), RATE, Preset::Default);
    stretch.set_transpose_semitones(f32::from(semitones));

    // The output trails the input by the analysis half of the latency, heard at the new tempo,
    // plus the synthesis half. Signalsmith splits its latency evenly at this preset; the shim
    // reports only the sum. That much lead is dropped, and silence fed after the end flushes
    // the tail out.
    let half = stretch.latency_frames() as f64 / 2.0;
    let lead = (half / speed + half).round() as usize;
    let length = (frames as f64 / speed).round() as usize;
    let total = lead + length;

    let mut outputs = vec![vec![0.0f32; length]; inputs.len()];
    let mut scratch = vec![vec![0.0f32; (CHUNK as f64 / speed).ceil() as usize + 1]; inputs.len()];
    let silence = vec![0.0f32; CHUNK];
    let (mut consumed, mut produced) = (0usize, 0usize);
    while produced < total {
        let take = match frames.saturating_sub(consumed) {
            0 => CHUNK,
            left => left.min(CHUNK),
        };
        let chunk: Vec<&[f32]> = inputs
            .iter()
            .map(|channel| channel.get(consumed..consumed + take).unwrap_or(&silence[..take]))
            .collect();
        consumed += take;
        let give = (consumed as f64 / speed).round() as usize - produced;
        let mut out: Vec<&mut [f32]> = scratch.iter_mut().map(|c| &mut c[..give]).collect();
        stretch.process(&chunk, &mut out);

        // The part of this call's output that falls after the lead and inside the song
        let keep_from = lead.max(produced);
        let keep_to = total.min(produced + give);
        if keep_from < keep_to {
            for (output, made) in outputs.iter_mut().zip(&scratch) {
                output[keep_from - lead..keep_to - lead]
                    .copy_from_slice(&made[keep_from - produced..keep_to - produced]);
            }
        }
        produced += give;
        progress.store((produced as f32 / total as f32).min(1.0).to_bits(), Ordering::Relaxed);
    }

    let mut channels = outputs.into_iter();
    let mut next_stem = || Some(Planar { left: channels.next()?, right: channels.next()? });
    let instrumental = next_stem().expect("the instrumental is always rendered");
    Stems::new(instrumental, next_stem())
}
