use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};

use crate::backing::{BackingError, StemSource, Stems, decode, prerender};

/// Where a song's stems are, each on disk or in memory.
pub struct StemSources {
    pub instrumental: StemSource,
    pub guide: Option<StemSource>,
}

/// Decodes a song's stems on the worker thread and, with an armed key or speed, renders them
/// through the stretcher. The control thread starts it beside `createMixer`, reads `progress` on
/// each events drain, and takes the stems with `finish` once the job is finished.
pub struct RenderJob {
    progress: Arc<AtomicU32>,
    worker: JoinHandle<Result<Stems, BackingError>>,
}

impl RenderJob {
    /// Panics unless `speed` is positive and finite; the binding checks ranges before this.
    pub fn start(sources: StemSources, semitones: i8, speed: f32) -> Self {
        Self::spawn(semitones, speed, move || {
            let instrumental = decode(sources.instrumental, "instrumental")?;
            let guide = sources.guide.map(|guide| decode(guide, "guideVocals")).transpose()?;
            Ok(Stems::new(instrumental, guide))
        })
    }

    /// For stems already in memory, such as a test's.
    pub fn from_stems(stems: Stems, semitones: i8, speed: f32) -> Self {
        Self::spawn(semitones, speed, move || Ok(stems))
    }

    fn spawn(
        semitones: i8,
        speed: f32,
        load: impl FnOnce() -> Result<Stems, BackingError> + Send + 'static,
    ) -> Self {
        assert!(speed.is_finite() && speed > 0.0, "speed {speed} is not a positive factor");
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        let reporter = Arc::clone(&progress);
        let worker = thread::Builder::new()
            .name("backing-render".to_owned())
            .spawn(move || {
                let stems = load()?;
                let stems = if semitones == 0 && speed == 1.0 {
                    stems
                } else {
                    prerender(stems, semitones, speed, &reporter)
                };
                reporter.store(1f32.to_bits(), Ordering::Relaxed);
                Ok(stems)
            })
            .expect("the render worker could not start");
        Self { progress, worker }
    }

    /// How much is done, 0 to 1. One atomic load, so the events thread reads it on every drain.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    /// The stems, joining the worker. Blocks until it is done, so the binding calls it from
    /// napi's task pool, never on the event loop.
    pub fn finish(self) -> Result<Stems, BackingError> {
        self.worker.join().unwrap_or(Err(BackingError::Worker))
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;
    use std::path::{Path, PathBuf};

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::backing::test_support::{SECOND, cents_between, median_pitch};
    use crate::backing::{Backing, Transport};
    use crate::block::{BLOCK, Stereo};
    use crate::command::event_ring;
    use crate::params::StemGains;

    fn write_wav(path: &Path, rate: u32, channels: &[Vec<f32>]) {
        std::fs::write(path, wav(rate, channels)).unwrap();
    }

    /// A 16-bit PCM WAV of `channels`, which must be one length.
    fn wav(rate: u32, channels: &[Vec<f32>]) -> Vec<u8> {
        let (count, frames) = (channels.len() as u16, channels[0].len() as u32);
        let data = frames * u32::from(count) * 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * u32::from(count) * 2).to_le_bytes());
        bytes.extend((count * 2).to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data.to_le_bytes());
        for frame in 0..frames as usize {
            for channel in channels {
                bytes.extend(((channel[frame] * f32::from(i16::MAX)) as i16).to_le_bytes());
            }
        }
        bytes
    }

    fn sine(hz: f32, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames).map(|i| 0.5 * (TAU * hz * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn files_decode_to_stereo_at_48_khz_padded_to_the_longest_stem() {
        let dir = std::env::temp_dir().join(format!("aeon-backing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (instrumental, guide) = (dir.join("instrumental.wav"), dir.join("guide.wav"));
        // A mono 44.1 kHz second, and a stereo 48 kHz second and a half with a silent right side
        write_wav(&instrumental, 44_100, &[sine(441.0, 44_100, 44_100)]);
        let guide_frames = 3 * SECOND / 2;
        write_wav(&guide, 48_000, &[sine(300.0, 48_000, guide_frames), vec![0.0; guide_frames]]);
        let sources = StemSources { instrumental: instrumental.into(), guide: Some(guide.into()) };
        let stems = RenderJob::start(sources, 0, 1.0).finish();
        std::fs::remove_dir_all(&dir).unwrap();
        let stems = stems.unwrap();

        assert_eq!(stems.frames, guide_frames as u64);
        let instrumental = &stems.instrumental;
        assert_eq!(instrumental.left, instrumental.right, "mono plays on both sides");
        let cents = cents_between(median_pitch(&instrumental.left[..SECOND]), 441.0);
        assert!(cents.abs() < 5.0, "resampled {cents:+.1} cents off");
        let tail = &instrumental.left[SECOND + SECOND / 100..];
        assert!(tail.iter().all(|s| s.abs() < 1e-3), "padded with silence past its second");

        let guide = stems.guide.unwrap();
        assert!(guide.left.iter().any(|s| s.abs() > 0.4));
        assert!(guide.right.iter().all(|s| *s == 0.0), "the sides stay apart");
    }

    /// Two seconds of what `backing` plays from its start, every block under the allocation guard.
    fn played(mut backing: Backing) -> Vec<f32> {
        let (mut events, _drain) = event_ring();
        let gains = StemGains { instrumental: 1.0, guide: 1.0 };
        backing.steer(1, Transport::Start, &mut events);
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        let mut played = Vec::new();
        for _ in 0..2 * SECOND / BLOCK {
            left.fill(0.0);
            right.fill(0.0);
            assert_no_alloc(|| {
                backing.render(
                    &gains,
                    &mut Stereo { left: &mut left, right: &mut right },
                    &mut events,
                );
            });
            played.extend(left.iter().chain(&right));
        }
        played
    }

    #[test]
    fn a_stem_from_bytes_plays_the_samples_its_file_does() {
        // 44.1 kHz, so the bytes take the resampler too, and at an armed key, so the stretcher
        let instrumental = wav(44_100, &[sine(441.0, 44_100, 44_100)]);
        let guide = wav(48_000, &[sine(300.0, 48_000, SECOND), sine(450.0, 48_000, SECOND)]);
        let dir = std::env::temp_dir().join(format!("aeon-bytes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: [PathBuf; 2] = [dir.join("instrumental.wav"), dir.join("guide.wav")];
        std::fs::write(&files[0], &instrumental).unwrap();
        std::fs::write(&files[1], &guide).unwrap();
        let [instrumental_file, guide_file] = files;
        let render = |sources| Backing::new(RenderJob::start(sources, 2, 1.0).finish().unwrap(), 2);
        let by_path = render(StemSources {
            instrumental: instrumental_file.into(),
            guide: Some(guide_file.into()),
        });
        std::fs::remove_dir_all(&dir).unwrap();
        let by_bytes = render(StemSources {
            instrumental: StemSource::Bytes(instrumental),
            guide: Some(StemSource::Bytes(guide)),
        });

        let (by_path, by_bytes) = (played(by_path), played(by_bytes));
        assert!(by_path.iter().any(|sample| sample.abs() > 0.1), "the stems play");
        assert_eq!(by_path, by_bytes);
    }

    #[test]
    fn bytes_that_will_not_decode_fail_naming_their_stem() {
        let sources = StemSources {
            instrumental: StemSource::Bytes(wav(48_000, &[sine(440.0, 48_000, SECOND)])),
            guide: Some(StemSource::Bytes(b"not a song".to_vec())),
        };
        let error = RenderJob::start(sources, 0, 1.0).finish().err().unwrap().to_string();
        assert!(error.starts_with("the guideVocals stem's bytes: "), "{error}");
    }
}
