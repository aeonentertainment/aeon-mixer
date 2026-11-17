//! `replay`: prints what a slot's recording or a session bundle's song played as (lib.rs).

use std::path::PathBuf;
use std::process::ExitCode;

use engine::block::RATE;
use engine::ingest::Codec;

use replay::levels::Levels;
use replay::reference::Kind;
use replay::stream::Stream;
use replay::{bundle, capture, reference, slot_replay, wav};

const USAGE: &str = "\
usage: replay RECORDING.pcap [--port N] [--codec opus|l16] [--playout-delay-ms MS] [--wav DIR]
       replay BUNDLE_DIR [--profile FILE] [--backing STEMS] [--wav DIR] [--json]

  RECORDING.pcap      what Mixer.recordSlot wrote, or any classic pcap of one RTP stream
  --port N            the UDP port the stream was sent to; needed when the capture has several
  --codec             opus (default) for a phone or mediasoup, l16 for a house mic
  --playout-delay-ms  the jitter buffer's delay, pinned as Aeon Karaoke pins it (default 15)
  --wav DIR           also write slot.wav and reference.wav, or a bundle's mix.wav, into DIR

  BUNDLE_DIR          a session bundle, as createMixer({ bundle }) writes it
  --profile FILE      replay under this settings profile, not the bundle's own
  --backing STEMS     a directory holding the song's stems, found by hash; without it the mic
                      path plays alone
  --json              print the replay's stats and levels as JSON";

/// How long the slot plays on after the last packet: past its delay and its longest frame.
const TAIL_FRAMES: i64 = RATE as i64 / 4;

struct Options {
    recording: PathBuf,
    port: Option<u16>,
    kind: Kind,
    playout_delay_ms: f64,
    wav: Option<PathBuf>,
    bundle: bundle::report::Options,
    /// The single-recording flags given, which a bundle refuses.
    recording_flags: Vec<String>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut recording = None;
    let mut options = Options {
        recording: PathBuf::new(),
        port: None,
        kind: Kind::Opus,
        playout_delay_ms: 15.0,
        wav: None,
        bundle: bundle::report::Options::default(),
        recording_flags: Vec::new(),
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        if matches!(arg.as_str(), "--port" | "--codec" | "--playout-delay-ms") {
            options.recording_flags.push(arg.clone());
        }
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
            "--profile" => options.bundle.profile = Some(PathBuf::from(value()?)),
            "--backing" => options.bundle.backing = Some(PathBuf::from(value()?)),
            "--json" => options.bundle.json = true,
            flag if flag.starts_with("--") => return Err(format!("no flag {flag}")),
            path => recording = Some(PathBuf::from(path)),
        }
    }
    options.recording = recording.ok_or("name a recording")?;
    Ok(options)
}

fn run(options: &Options) -> Result<(), String> {
    if options.recording.is_dir() {
        if let Some(flag) = options.recording_flags.first() {
            return Err(format!("{flag} is for a single recording; a bundle says its own"));
        }
        return bundle::report::run(&options.recording, &options.bundle, options.wav.as_deref());
    }
    if options.bundle != bundle::report::Options::default() {
        return Err("--profile, --backing and --json are for a bundle's directory".into());
    }
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
    if senders.len() > 1 {
        // Decision 0028: the slot takes one address at a time, and the capture holds no resetSlot
        println!(
            "several senders: the slot plays one at a time and libopus alone decodes them all, so \
             their levels can differ by what the slot refused"
        );
    }
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
        wav::write(&dir.join("slot.wav"), &replayed.out, 1)?;
        wav::write(&dir.join("reference.wav"), &reference, 1)?;
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
