use std::mem::size_of;
use std::slice;
use std::sync::Arc;

use objc2_core_audio_types::{AudioBuffer, AudioTimeStamp, AudioTimeStampFlags};

use crate::feed::{Feed, interleave};
use crate::stats::Stats;

/// What one IOProc renders with, on the HAL's IO thread.
pub struct Callback {
    feed: Feed,
    stats: Arc<Stats>,
    next_sample_time: Option<f64>,
}

impl Callback {
    pub fn new(feed: Feed, stats: Arc<Stats>) -> Self {
        Self { feed, stats, next_sample_time: None }
    }

    /// One IO cycle: fills every output buffer the HAL hands over. A device's f32 stream may be
    /// one interleaved buffer or one buffer a channel; channels count across buffers either way.
    pub fn run(&mut self, buffers: &mut [AudioBuffer], time: &AudioTimeStamp) {
        self.stats.callback();
        let Some(first) = buffers.first() else { return };
        let frames = first.mDataByteSize as usize
            / (size_of::<f32>() * (first.mNumberChannels as usize).max(1));

        // After an overload the HAL skips the device's clock ahead, so a cycle that does not
        // start where the last one ended is a dropout the listener heard.
        if time.mFlags.contains(AudioTimeStampFlags::SampleTimeValid) {
            if self.next_sample_time.is_some_and(|next| (time.mSampleTime - next).abs() >= 1.0) {
                self.stats.underrun();
            }
            self.next_sample_time = Some(time.mSampleTime + frames as f64);
        }

        self.feed.take(frames, |at, left, right| {
            let mut first_channel = 0;
            for buffer in buffers.iter_mut() {
                let channels = buffer.mNumberChannels as usize;
                if !buffer.mData.is_null() {
                    // SAFETY: the HAL's buffer holds mDataByteSize bytes of f32 for this cycle.
                    let samples = unsafe {
                        slice::from_raw_parts_mut(
                            buffer.mData.cast::<f32>(),
                            buffer.mDataByteSize as usize / size_of::<f32>(),
                        )
                    };
                    if let Some(span) = samples.get_mut(at * channels..(at + left.len()) * channels)
                    {
                        interleave(left, right, span, channels, first_channel);
                    }
                }
                first_channel += channels;
            }
        });
    }
}
