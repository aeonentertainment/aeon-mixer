//! Aeon Karaoke's `npm run aec:measure` measures GStreamer's canceller on synthetic bleed and
//! keeps what it fed it: `in-<scenario>.wav`, the playback and the mic as two channels, and
//! `near-speech.wav`, the singer. This runs the same files through our canceller and scores
//! them with the harness's own `measure`, so the two tables compare like for like.

use std::path::Path;

use engine::block::RATE;
use kernels::fixtures::{db, rms};

use crate::cancel::cancel;
use crate::wav;

const SECONDS: usize = 24;
const TALK_FROM: usize = 9;
const TALK_TO: usize = 17;

pub struct Score {
    pub scenario: String,
    pub erle_db: f32,
    pub converged_at: Option<usize>,
    pub talker_delta_db: f32,
    pub path_ms: Option<f32>,
}

pub fn run(dir: &Path) -> Result<Vec<Score>, String> {
    let talker = talker(&dir.join("near-speech.wav"))?;
    let mut inputs: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("in-") && name.ends_with(".wav"))
        })
        .collect();
    inputs.sort();
    if inputs.is_empty() {
        return Err(format!("{}: no in-*.wav; run `npm run aec:measure` first", dir.display()));
    }
    inputs
        .iter()
        .map(|path| {
            let channels = wav::read(path)?;
            let [played, mic] = &channels[..] else {
                return Err(format!("{}: expected playback and mic", path.display()));
            };
            let cancelled = cancel(played, mic);
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let out_path = dir.join(format!("out-{}-aeon.wav", &name[3..]));
            wav::write_mono(&out_path, &cancelled.out)?;
            let mut score = measure(mic, &cancelled.out, &talker);
            score.scenario = name[3..].to_string();
            score.path_ms = cancelled.findings.map(|f| f.path_frames as f32 * 1000.0 / RATE as f32);
            Ok(score)
        })
        .collect()
}

/// The singer as the harness builds them: `say`'s speech at -20 dBFS RMS from 9 s to 17 s.
fn talker(path: &Path) -> Result<Vec<f32>, String> {
    let speech = wav::read(path)?.swap_remove(0);
    let gain = 10f32.powf(-20.0 / 20.0) / rms(&speech);
    let rate = RATE as usize;
    let mut talker = vec![0.0; SECONDS * rate];
    for (i, sample) in speech.iter().take((TALK_TO - TALK_FROM) * rate).enumerate() {
        talker[TALK_FROM * rate + i] = sample * gain;
    }
    Ok(talker)
}

/// The harness's `measure`: steady-state removal over the echo-only seconds from 3 s, leaving out
/// the second after the singer stops; the first echo-only second at 15 dB; and how far the
/// output sits from the clean singer while both talk, from 10 s.
fn measure(mic: &[f32], out: &[f32], talker: &[f32]) -> Score {
    let rate = RATE as usize;
    let (mut echo_only, mut double_talk) = (Vec::new(), Vec::new());
    let mut converged_at = None;
    for s in 0..SECONDS {
        let range = s * rate..((s + 1) * rate).min(out.len());
        if range.is_empty() {
            break;
        }
        let in_db = db(rms(&mic[range.clone()]));
        let out_db = db(rms(&out[range.clone()]));
        let talking = (TALK_FROM..TALK_TO).contains(&s);
        if !talking && converged_at.is_none() && in_db - out_db >= 15.0 {
            converged_at = Some(s);
        }
        if !talking && s >= 3 && s != TALK_TO {
            echo_only.push(in_db - out_db);
        }
        if talking && s > TALK_FROM {
            double_talk.push(out_db - db(rms(&talker[range])));
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    Score {
        scenario: String::new(),
        erle_db: mean(&echo_only),
        converged_at,
        talker_delta_db: mean(&double_talk),
        path_ms: None,
    }
}
