//! The forced feedback check on real speakers: the default input through a slot with the howl
//! guard on, to the default output, the slot's gain rising a decibel a second until the loop
//! howls and the guard catches it. A person runs it with `--open-device`; nothing else opens a
//! device. If the guard does not hold the howl the render path mutes the room itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use devices::{CaptureRequest, Captured, Request, Stats};
use engine::block::{BLOCK, RATE, Stereo};
use engine::command::event_ring;
use engine::graph::Graph;
use engine::ingest::push::push_voice_with_depth;
use engine::params::{EchoCancelParams, HowlGuardParams, params_channel};
use engine::slot::Slot;

/// Where the slot's gain starts and the most it is raised to.
const START_DB: f32 = -20.0;
const MOST_DB: f32 = 12.0;
const RISE_DB_PER_SECOND: f32 = 1.0;
/// The render path mutes for good when the mix peaks over this for `TRIP_BLOCKS` in a row,
/// 107 ms: a howl the guard did not hold.
const TRIP_PEAK: f32 = 0.5;
const TRIP_BLOCKS: u32 = 40;
/// After the first catch the check holds the gain there this long, to see the loop stay quiet.
const AFTER_CATCH: Duration = Duration::from_secs(10);
/// At the top gain the check watches this long before it reports no howl. The guard needs 107 ms
/// to settle on a tone and then 6 dB of growth: a loop 0.5 dB over unity on an 80 ms round trip
/// grows that in about a second, so ten seconds covers a loop ten times slower than that.
const AT_THE_TOP: Duration = Duration::from_secs(10);

pub struct Options {
    pub input: Option<String>,
    pub output: Option<String>,
    pub cancel: bool,
}

pub fn run(options: Options) -> Result<(), String> {
    let (mut producer, voice) = push_voice_with_depth(4096, 3 * BLOCK);
    let slot = Slot::new(Box::new(voice));
    let report = slot.echo_report();
    let mut graph = Graph::new(vec![slot]);
    let (mut writer, mut reader) = params_channel();
    let set_gain = |writer: &mut engine::params::ParamsWriter, db: f32| {
        writer.update(|p| {
            p.slots[0].gain = 10f32.powf(db / 20.0);
            p.slots[0].effects.howl_guard = HowlGuardParams { enabled: true };
            p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: options.cancel };
        });
    };
    set_gain(&mut writer, START_DB);

    let tripped = Arc::new(AtomicBool::new(false));
    // The mix's level over the last 50 ms, in hundredths of a dB, for the report
    let level = Arc::new(AtomicU32::new(0));
    let (mut events, _drain) = event_ring();
    let render = {
        let (tripped, level) = (Arc::clone(&tripped), Arc::clone(&level));
        let (mut hot, mut energy, mut frames) = (0u32, 0.0f32, 0usize);
        Box::new(move |out: Stereo<'_>| {
            let Stereo { left, right } = out;
            graph.render(
                reader.latest(),
                Stereo { left: &mut *left, right: &mut *right },
                &mut events,
            );
            let peak = left.iter().chain(right.iter()).fold(0.0f32, |m, s| m.max(s.abs()));
            hot = if peak > TRIP_PEAK { hot + 1 } else { 0 };
            if hot >= TRIP_BLOCKS {
                tripped.store(true, Ordering::Relaxed);
            }
            if tripped.load(Ordering::Relaxed) {
                left.fill(0.0);
                right.fill(0.0);
            }
            energy += left.iter().map(|s| s * s).sum::<f32>();
            frames += left.len();
            if frames >= RATE as usize / 20 {
                let db = 10.0 * (energy / frames as f32).max(1e-12).log10();
                level.store(((db + 200.0) * 100.0) as u32, Ordering::Relaxed);
                (energy, frames) = (0.0, 0);
            }
        })
    };
    let output = devices::driver(
        Request { device: options.output, ..Request::default() },
        Arc::new(Stats::new()),
    );
    let running = output.start(render).map_err(|e| format!("the output did not open: {e}"))?;
    let mut interleaved = vec![0.0f32; 2 * 4096];
    let sink = Box::new(move |captured: Captured<'_>| {
        for (frame, sample) in interleaved.as_chunks_mut::<2>().0.iter_mut().zip(captured.samples) {
            frame.fill(*sample);
        }
        let _ = producer.push(&interleaved[..2 * captured.samples.len().min(4096)]);
    });
    let input = devices::capture(
        CaptureRequest { device: options.input, ..CaptureRequest::default() },
        Arc::new(Stats::new()),
    );
    let capturing = input.start(sink).map_err(|e| format!("the input did not open: {e}"))?;
    println!(
        "playing on {} frames of output, hearing {} frames of input; the slot rises from {START_DB} dB",
        running.info.period_frames, capturing.info.period_frames
    );

    let started = Instant::now();
    let mut levels = Vec::new();
    let mut caught = None;
    let mut gain_db = START_DB;
    let mut topped = None;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let at = started.elapsed().as_secs_f32();
        levels.push((at, level.load(Ordering::Relaxed) as f32 / 100.0 - 200.0));
        if tripped.load(Ordering::Relaxed) {
            return Err(format!(
                "safety mute at {at:.2} s, gain {gain_db:+.1} dB: the mix stayed over -6 dBFS and the \
                 guard did not hold it. Levels: {}",
                recent(&levels, at - 3.0)
            ));
        }
        match caught {
            None if report.howl_catches() > 0 => {
                caught = Some((at, gain_db));
                println!(
                    "caught at {at:.2} s, gain {gain_db:+.1} dB; the mix, 50 ms at a time: {}",
                    recent(&levels, at - 3.0)
                );
            }
            None if gain_db >= MOST_DB => {
                let since = *topped.get_or_insert(at);
                if Duration::from_secs_f32(at - since) >= AT_THE_TOP {
                    println!(
                        "no howl up to {MOST_DB:+.1} dB of gain, held {} s; the loop never ran away",
                        AT_THE_TOP.as_secs()
                    );
                    break;
                }
            }
            None => {
                gain_db = (START_DB + RISE_DB_PER_SECOND * at).min(MOST_DB);
                set_gain(&mut writer, gain_db);
            }
            Some((when, _)) if Duration::from_secs_f32(at - when) >= AFTER_CATCH => break,
            Some(_) => {}
        }
    }
    drop(capturing);
    drop(running);
    if let Some((when, gain)) = caught {
        println!(
            "the guard caught the loop at {gain:+.1} dB of slot gain, {when:.2} s in, and ducked {} \
             times in all; the mix after: {}",
            report.howl_catches(),
            recent(&levels, when)
        );
    }
    Ok(())
}

fn recent(levels: &[(f32, f32)], since: f32) -> String {
    levels
        .iter()
        .filter(|(at, _)| *at >= since)
        .map(|(_, db)| format!("{db:.0}"))
        .collect::<Vec<_>>()
        .join(" ")
}
