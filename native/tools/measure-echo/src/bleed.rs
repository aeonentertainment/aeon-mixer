//! The bleed measurement a person makes with a phone at the mouth and the speakers at room
//! volume. `programme` writes what to play: two seconds of quiet, fourteen of a backing alone,
//! and fourteen more for the person to sing over. `bleed` takes the phone's recording of it,
//! finds where the programme sits in it, and says how loud the speakers are at the phone against
//! the voice, and what the canceller makes of that bleed offline. Opens no device: the person
//! plays the programme with any player.

use std::path::Path;

use engine::block::RATE;
use kernels::fixtures::{at_db, db, pink_noise, rms, talker};
use realfft::RealFftPlanner;
use realfft::num_complex::Complex32;

use crate::cancel::cancel;
use crate::wav;

const QUIET: usize = 2;
const ALONE: usize = 16;
const END: usize = 30;
/// The coarse search runs at 3 kHz, over the take's first `SEARCH` seconds.
const DECIMATION: usize = 16;
const SEARCH: usize = 24;

pub fn programme(path: &Path) -> Result<(), String> {
    wav::write_mono(path, &programme_samples())
}

fn programme_samples() -> Vec<f32> {
    let rate = RATE as usize;
    let backing: Vec<f32> = at_db(talker((END - QUIET) * rate, 110.0, 3), -18.0)
        .iter()
        .zip(&at_db(pink_noise((END - QUIET) * rate, 7), -26.0))
        .map(|(v, b)| v + b)
        .collect();
    let mut out = vec![0.0; QUIET * rate];
    out.extend(backing);
    out
}

pub struct Bleed {
    /// Where the programme is heard in the take: when recording started, the player's own
    /// output latency and the air together, which nothing here can split.
    pub heard_at_ms: f32,
    /// The take while only the backing played, and the voice over it once the person sang.
    pub bleed_dbfs: f32,
    pub voice_dbfs: Option<f32>,
    /// The canceller found the echo and learned it.
    pub found: bool,
    /// What the canceller took out of the backing-only stretch, from 4 s to 15 s.
    pub removed_db: f32,
}

pub fn measure(programme: &Path, take: &Path) -> Result<Bleed, String> {
    measure_samples(&wav::read(programme)?.swap_remove(0), &wav::read(take)?.swap_remove(0))
}

fn measure_samples(played: &[f32], take: &[f32]) -> Result<Bleed, String> {
    let rate = RATE as usize;
    let offset = locate(played, take);
    if offset + END * rate > take.len() {
        return Err("the take ends before the programme does; record all 30 s of it".into());
    }
    // The take from 100 ms before the programme is heard, against the programme from its start:
    // the reference leads its echo by those 100 ms, as the master leads a slot's echo, and the
    // canceller finds the rest
    let start = offset.saturating_sub(rate / 10);
    let heard = &take[start..start + END * rate];
    let cancelled = cancel(&played[..END * rate], heard);

    let seconds = |signal: &[f32], from: usize, to: usize| -> f32 {
        let at = |s: usize| (s * rate + offset - start).min(signal.len());
        rms(&signal[at(from)..at(to)])
    };
    // Each stretch a second clear of its edges, for a person who starts singing a little early
    let bleed = seconds(heard, QUIET + 1, ALONE - 1);
    let sung = seconds(heard, ALONE + 1, END);
    let voice = (sung * sung - bleed * bleed).max(0.0).sqrt();
    Ok(Bleed {
        heard_at_ms: offset as f32 * 1000.0 / RATE as f32,
        bleed_dbfs: db(bleed),
        voice_dbfs: (voice > bleed * 0.1).then(|| db(voice)),
        found: cancelled.findings.is_some(),
        removed_db: db(seconds(heard, 4, ALONE - 1)) - db(seconds(&cancelled.out, 4, ALONE - 1)),
    })
}

/// Where `played` starts in `take`, to within the decimation: the peak of their cross-correlation
/// on low-passed, decimated copies.
fn locate(played: &[f32], take: &[f32]) -> usize {
    let decimate = |signal: &[f32], seconds: usize| -> Vec<f32> {
        let frames = (seconds * RATE as usize).min(signal.len());
        signal[..frames]
            .chunks(DECIMATION)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    };
    let a = decimate(played, ALONE);
    let b = decimate(take, SEARCH + ALONE);
    let len = (a.len() + b.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let (forward, inverse) = (planner.plan_fft_forward(len), planner.plan_fft_inverse(len));
    let spectrum = |signal: &[f32]| {
        let mut time = signal.to_vec();
        time.resize(len, 0.0);
        let mut out = forward.make_output_vec();
        let _ = forward.process(&mut time, &mut out);
        out
    };
    let (sa, sb) = (spectrum(&a), spectrum(&b));
    let mut product: Vec<Complex32> = sa.iter().zip(&sb).map(|(x, y)| x.conj() * y).collect();
    let mut correlation = inverse.make_output_vec();
    let _ = inverse.process(&mut product, &mut correlation);
    let lag = correlation[..b.len()]
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map_or(0, |(lag, _)| lag);
    lag * DECIMATION
}

#[cfg(test)]
mod tests {
    use kernels::fixtures::{room, through};

    use super::*;

    // A take that starts 1.3 s before the programme, hears the speakers 60 ms after the player
    // through a living room 10 dB down, and a voice 15 dB over that from 16 s. Tolerances: where
    // it is heard within the coarse search's 16 frames, the levels within a decibel, and 20 dB
    // of the bleed taken out.
    #[test]
    fn a_take_is_found_and_its_bleed_and_voice_read() {
        let rate = RATE as usize;
        let played = programme_samples();
        let path = room(60 * 48, 0.316, 0.3, 0.25, 11);
        let echo = through(&played, &path);
        let lead = 13 * rate / 10;
        let mut take = vec![0.0; lead];
        take.extend_from_slice(&echo);
        take.resize(lead + END * rate + rate, 0.0);
        let bleed = db(rms(&echo[(QUIET + 1) * rate..(ALONE - 1) * rate]));
        let voice = at_db(talker((END - ALONE) * rate, 180.0, 9), bleed + 15.0);
        for (t, v) in take[lead + ALONE * rate..].iter_mut().zip(&voice) {
            *t += v;
        }

        let found = measure_samples(&played, &take).expect("measured");
        let heard = found.heard_at_ms * RATE as f32 / 1000.0;
        let expected = (lead + 60 * 48) as f32;
        assert!((heard - expected).abs() <= 16.0, "the programme heard at {heard} frames");
        assert!((found.bleed_dbfs - bleed).abs() < 1.0, "bleed {:.1}", found.bleed_dbfs);
        let over = found.voice_dbfs.expect("a voice") - found.bleed_dbfs;
        assert!((over - 15.0).abs() < 1.0, "the voice {over:.1} dB over the bleed");
        assert!(found.found);
        assert!(found.removed_db > 20.0, "removed {:.1} dB", found.removed_db);
    }
}
