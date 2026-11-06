use std::mem::size_of;
use std::slice;
use std::sync::Arc;
use std::time::Instant;

use objc2_core_audio_types::{AudioBuffer, AudioTimeStamp, AudioTimeStampFlags};

use crate::capture_feed::CaptureFeed;
use crate::stats::Stats;

/// What one input IOProc runs with, on the HAL's IO thread.
pub struct CaptureCallback {
    feed: CaptureFeed,
    channel: usize,
    stats: Arc<Stats>,
    next_sample_time: Option<f64>,
}

impl CaptureCallback {
    pub fn new(feed: CaptureFeed, channel: usize, stats: Arc<Stats>) -> Self {
        Self { feed, channel, stats, next_sample_time: None }
    }

    /// One IO cycle: finds the buffer that holds the channel and hands it over. A device's streams
    /// may give one interleaved buffer or one buffer a channel; channels count across buffers.
    pub fn run(&mut self, buffers: &[AudioBuffer], time: &AudioTimeStamp) {
        let at = Instant::now();
        self.stats.callback();
        let mut first_channel = 0;
        for buffer in buffers {
            let channels = buffer.mNumberChannels as usize;
            if (first_channel..first_channel + channels).contains(&self.channel)
                && !buffer.mData.is_null()
            {
                // SAFETY: the HAL's buffer holds mDataByteSize bytes of f32 for this cycle.
                let samples = unsafe {
                    slice::from_raw_parts(
                        buffer.mData.cast::<f32>(),
                        buffer.mDataByteSize as usize / size_of::<f32>(),
                    )
                };
                self.note_gap(time, samples.len() / channels);
                self.feed.deliver(samples, channels, self.channel - first_channel, at);
                return;
            }
            first_channel += channels;
        }
    }

    // A cycle that does not start where the last one ended lost frames: the count no longer says
    // how far the device's clock has run.
    fn note_gap(&mut self, time: &AudioTimeStamp, frames: usize) {
        if time.mFlags.contains(AudioTimeStampFlags::SampleTimeValid) {
            if self.next_sample_time.is_some_and(|next| (time.mSampleTime - next).abs() >= 1.0) {
                self.stats.underrun();
            }
            self.next_sample_time = Some(time.mSampleTime + frames as f64);
        }
    }
}
