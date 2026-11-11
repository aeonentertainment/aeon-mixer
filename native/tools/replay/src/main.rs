//! replay: a slot's recording through the real chain on the offline driver, beside libopus's
//! own decode of the same packets (docs/design.md 8.9, decision 0017). It opens no device and
//! no socket. The same file replays to the same samples every time.

mod capture;
mod levels;
mod reference;
mod slot_replay;
mod stream;
mod wav;

#[cfg(test)]
mod round_trip;

use std::path::PathBuf;
use std::process::ExitCode;

use engine::block::RATE;
use engine::ingest::Codec;

use crate::levels::Levels;
use crate::reference::Kind;
use crate::stream::Stream;

const USAGE: &str = "\
usage: replay RECORDING.pcap [--port N] [--codec opus|l16] [--playout-delay-ms MS] [--wav DIR]

  RECORDING.pcap      what Mixer.recordSlot wrote, or any classic pcap of one RTP stream
  --port N            the UDP port the stream was sent to; needed when the capture has several
  --codec             opus (default) for a phone or mediasoup, l16 for a house mic
  --playout-delay-ms  the jitter buffer's delay, pinned as Aeon Karaoke pins it (default 15)
  --wav DIR           also write slot.wav and reference.wav into DIR";

/// How long the slot plays on after the last packet: past its delay and its longest frame.
const TAIL_FRAMES: i64 = RATE as i64 / 4;

struct Options {
    recording: PathBuf,
    port: Option<u16>,
    kind: Kind,
    playout_delay_ms: f64,
    wav: Option<PathBuf>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut recording = None;
    let mut options = Options {
        recording: PathBuf::new(),
        port: None,
        kind: Kind::Opus,
        playout_delay_ms: 15.0,
        wav: None,
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--port" => options.port = Some(value()?.parse().map_err(|_| "--port takes a port")?),
            "--codec" => {
                options.kind = match value()?.as_str() {
                    "opus" => Kind::Opus,
                    "l16" => Kind::L16,
                    other => return Err(format!("no codec {other:?}")),
                }
            }
            "--playout-delay-ms" => {
                options.playout_delay_ms =
                    value()?.parse().map_err(|_| "--playout-delay-ms takes milliseconds")?;
            }
            "--wav" => options.wav = Some(PathBuf::from(value()?)),
            flag if flag.starts_with("--") => return Err(format!("no flag {flag}")),
            path => recording = Some(PathBuf::from(path)),
        }
    }
    options.recording = recording.ok_or("name a recording")?;
    Ok(options)
}

fn run(options: &Options) -> Result<(), String> {
    let bytes = std::fs::read(&options.recording)
        .map_err(|e| format!("{}: {e}", options.recording.display()))?;
    let stream = Stream::of(&capture::read(&bytes)?, options.port)?;
    if stream.arrivals.is_empty() {
        return Err(format!("no RTP reached port {}", stream.port));
    }
    let codec = match options.kind {
        Kind::Opus => Codec::opus().map_err(|e| e.to_string())?,
        Kind::L16 => Codec::L16,
    };
    let delay = (options.playout_delay_ms * f64::from(RATE) / 1000.0).round() as u32;
    let replayed = slot_replay::replay(&stream.arrivals, codec, delay, TAIL_FRAMES);
    let reference = reference::decode(&stream.arrivals, options.kind)?;

    let seconds = |frames: usize| frames as f64 / f64::from(RATE);
    let last = stream.arrivals.last().map_or(0, |a| a.at);
    println!(
        "{}: port {}, {} RTP packets over {:.1} s, {} other datagrams left out",
        options.recording.display(),
        stream.port,
        stream.arrivals.len(),
        seconds(last as usize),
        stream.dropped,
    );
    let senders: Vec<String> =
        stream.senders.iter().map(|(ip, port)| format!("{ip}:{port}")).collect();
    println!("from {}", senders.join(", "));
    let s = replayed.stats;
    println!(
        "slot stats: pushed {} lost {} late {} concealed {} resyncs {} jitter {:.2} ms, ring full {}",
        s.pushed, s.lost, s.late, s.concealed, s.resyncs, s.jitter_ms, replayed.ring_full
    );
    // The slot's output on the reference's clock, so their windows hold the same stretch of voice
    let anchor = slot_replay::anchor(&stream.arrivals, delay).min(replayed.out.len());
    let aligned = &replayed.out[anchor..(anchor + reference.len()).min(replayed.out.len())];
    let rows = [
        ("libopus alone", Levels::of_signal(&reference)),
        ("slot output", Levels::of_signal(aligned)),
        ("slot meter", Levels::of_readings(&replayed.meter_db)),
    ];
    println!("\n100 ms RMS, dBFS     top tenth   median   loudest   readings");
    for (name, levels) in rows {
        println!(
            "{name:<18} {:>9.1} {:>8.1} {:>9.1} {:>10}",
            levels.top_tenth_db, levels.median_db, levels.loudest_db, levels.readings
        );
    }
    let (reference_levels, slot_levels) = (rows[0].1, rows[1].1);
    println!(
        "slot - libopus     {:>9.1} {:>8.1} {:>9.1}",
        slot_levels.top_tenth_db - reference_levels.top_tenth_db,
        slot_levels.median_db - reference_levels.median_db,
        slot_levels.loudest_db - reference_levels.loudest_db,
    );

    if let Some(dir) = &options.wav {
        wav::write_mono(&dir.join("slot.wav"), &replayed.out)?;
        wav::write_mono(&dir.join("reference.wav"), &reference)?;
        println!("\nwrote slot.wav and reference.wav in {}", dir.display());
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)).and_then(|options| run(&options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

// Tests wrap every pump in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
