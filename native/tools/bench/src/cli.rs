use std::path::PathBuf;

use measure_latency::cli::{finite, physical, under_clipper};
use replay::bundle::report::Options as Replay;

use crate::loopback::Loopback;

pub const USAGE: &str = "\
usage: bench play BUNDLE [--backing STEMS] [--profile FILE] [--out DIR] [--baseline FILE] [--write FILE]
                         [--open-device [--output ID] [--period 64|128]]
       bench loopback --open-device --rig cable|mic [--air-cm CM] [--output ID] [--input ID]
                      [--input-channel N] [--period 64|128] [--level-db DB] [--baseline FILE] [--write FILE]

  play: plays a session bundle through the mixer on a real clock, as replay plays it on the
  offline driver, and says whether the mix kept up. Without --open-device it runs on the timer
  clock, which opens nothing and takes the song's length.

  loopback: plays chirps out of a real output and reads them back on a house mic's capture slot,
  through a cable or a mic at the speaker, and says how long a house mic's voice takes to reach
  the room on this machine. It always opens a device, so it needs --open-device.

  BUNDLE             a session bundle's directory, as createMixer({ bundle }) writes it
  --backing STEMS    a directory holding the song's stems, found by hash; without it the mic path
                     plays alone
  --profile FILE     play under this settings profile, not the bundle's own
  --out DIR          write the bundle of this run into DIR, which must not exist yet
  --baseline FILE    an earlier bench result: report each number that moved, and fail on any it
                     does not allow. A bench figure belongs to its machine
  --write FILE       write this run's result, with the baseline's tolerances where it has them
  --open-device      use real devices. Needs a build with --features hardware
  --output ID        an output id from `measure-latency list`; the default output otherwise
  --input ID         an input id from `measure-latency list`; the default input otherwise
  --input-channel N  the input channel the rig feeds, from 0 (default 0)
  --period N         the output's period in frames, 64 or 128 (default 128)
  --rig cable        a cable from the output's left channel to the input: stops at the jack, so
                     it leaves out anything a powered speaker adds
  --rig mic          a mic --air-cm CM from the speaker: what a room hears, the air taken off
  --level-db DB      the chirps' peak in dBFS (default -12). Set the volume to speech level";

/// Where the mix goes: the timer clock, or a real output.
#[derive(Debug, PartialEq)]
pub enum Clock {
    Timer,
    Device { output: Option<String>, period: u32 },
}

pub enum Command {
    Play(Args),
    Loopback(LoopbackArgs),
    /// `--help`, `-h` or `help`: the usage, and nothing run.
    Help,
}

pub struct Args {
    pub bundle: PathBuf,
    pub replay: Replay,
    pub out: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub write: Option<PathBuf>,
    pub clock: Clock,
}

pub struct LoopbackArgs {
    pub loopback: Loopback,
    pub output: Option<String>,
    /// The output's period to ask for; the slot is set up for whatever the output grants.
    pub period: u32,
    pub input: Option<String>,
    pub input_channel: u16,
    pub baseline: Option<PathBuf>,
    pub write: Option<PathBuf>,
}

/// The flags `bench play` takes; each other flag is refused as the other command's.
const PLAY_TAKES: [&str; 8] = [
    "--backing",
    "--profile",
    "--out",
    "--baseline",
    "--write",
    "--open-device",
    "--output",
    "--period",
];

/// The flags `bench loopback` takes.
const LOOPBACK_TAKES: [&str; 10] = [
    "--open-device",
    "--rig",
    "--air-cm",
    "--output",
    "--input",
    "--input-channel",
    "--period",
    "--level-db",
    "--baseline",
    "--write",
];

/// Reads the command line. A flag the command does not take is refused by name, since a run that
/// quietly ignored one would file a number for something other than what was asked.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    if args.peek().is_some_and(|first| matches!(first.as_str(), "help" | "--help" | "-h")) {
        return Ok(Command::Help);
    }
    let (takes, other) = match args.next().as_deref() {
        Some("play") => (&PLAY_TAKES[..], "loopback"),
        Some("loopback") => (&LOOPBACK_TAKES[..], "play"),
        _ => return Err("name a command: play or loopback".into()),
    };
    let playing = other == "loopback";
    let mut bundle = None;
    let (mut replay, mut out, mut baseline, mut write) = (Replay::default(), None, None, None);
    let (mut open_device, mut output, mut period) = (false, None, None);
    let (mut rig, mut air_cm, mut input, mut input_channel, mut level_db) =
        (None, None, None, 0, -12.0);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(Command::Help);
        }
        if arg.starts_with("--") && !takes.contains(&arg.as_str()) {
            let known =
                PLAY_TAKES.contains(&arg.as_str()) || LOOPBACK_TAKES.contains(&arg.as_str());
            return Err(if known {
                format!("{arg} is for {other}")
            } else {
                format!("no flag {arg}")
            });
        }
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--backing" => replay.backing = Some(PathBuf::from(value()?)),
            "--profile" => replay.profile = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--baseline" => baseline = Some(PathBuf::from(value()?)),
            "--write" => write = Some(PathBuf::from(value()?)),
            "--open-device" => open_device = true,
            "--output" => output = Some(value()?),
            "--input" => input = Some(value()?),
            "--period" => match value()?.as_str() {
                "64" => period = Some(64),
                "128" => period = Some(128),
                other => return Err(format!("--period takes 64 or 128, not {other}")),
            },
            "--rig" => rig = Some(value()?),
            "--air-cm" => air_cm = Some(finite::<f64>(&arg, value()?)?),
            "--input-channel" => {
                let text = value()?;
                input_channel = text
                    .parse()
                    .map_err(|_| format!("--input-channel takes 0 to 65535, not {text}"))?
            }
            "--level-db" => level_db = under_clipper(finite(&arg, value()?)?)?,
            path if !playing => return Err(format!("loopback takes no bundle, not {path}")),
            path if bundle.is_none() => bundle = Some(PathBuf::from(path)),
            path => return Err(format!("one bundle at a time, not {path} as well")),
        }
    }
    if playing {
        let clock = if open_device {
            Clock::Device { output, period: period.unwrap_or(128) }
        } else if output.is_some() || period.is_some() {
            return Err(
                "--output and --period choose a real output, so they need --open-device".into()
            );
        } else {
            Clock::Timer
        };
        let bundle = bundle.ok_or("name a bundle")?;
        return Ok(Command::Play(Args { bundle, replay, out, baseline, write, clock }));
    }
    if !open_device {
        return Err("loopback plays through a real output and input: add --open-device".into());
    }
    let loopback = Loopback { rig: physical(rig.as_deref(), air_cm)?, level_db };
    Ok(Command::Loopback(LoopbackArgs {
        loopback,
        output,
        period: period.unwrap_or(128),
        input,
        input_channel,
        baseline,
        write,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Command, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn a_real_device_is_only_ever_opened_by_name() {
        assert!(matches!(parsed("play song"), Ok(Command::Play(Args { clock: Clock::Timer, .. }))));
        assert!(parsed("play song --output x").is_err());
        assert!(parsed("play song --period 64").is_err());
        assert!(matches!(
            parsed("play song --open-device --period 64"),
            Ok(Command::Play(Args { clock: Clock::Device { output: None, period: 64 }, .. }))
        ));
        assert!(parsed("play song --open-device --period 256").is_err());
        assert!(parsed("loopback --rig cable").is_err());
        assert!(matches!(parsed("loopback --open-device --rig cable"), Ok(Command::Loopback(_))));
        assert!(parsed("").is_err(), "nothing asked is a bad command line");
        for line in ["--help", "help", "play song --help", "loopback -h"] {
            assert!(matches!(parsed(line), Ok(Command::Help)), "{line:?} asks for the usage");
        }
    }

    #[test]
    fn each_command_takes_its_own_flags_and_refuses_the_others_by_name() {
        let play = "play song";
        let loopback = "loopback --open-device --rig mic --air-cm 10";
        // Each flag with a value it takes, and whether play and loopback take it
        let flags = [
            ("--backing stems", true, false),
            ("--profile singer.json", true, false),
            ("--out run", true, false),
            ("--baseline before.json", true, true),
            ("--write after.json", true, true),
            ("--output id", true, true),
            ("--period 64", true, true),
            ("--input id", false, true),
            ("--input-channel 1", false, true),
            ("--level-db -20", false, true),
            ("--rig cable", false, true),
            ("--air-cm 5", false, true),
        ];
        for (flag, by_play, by_loopback) in flags {
            let name = flag.split(' ').next().unwrap_or(flag);
            // `--output` and `--period` choose a real output, and play needs it asked for
            let play_line = format!("{play} --open-device {flag}");
            match (parsed(&play_line), by_play) {
                (Ok(_), true) => {}
                (Err(error), false) => assert_eq!(error, format!("{name} is for loopback")),
                (got, _) => panic!("{play_line}: {:?}", got.err()),
            }
            // A second --rig or --air-cm is still a loopback line; cable takes no --air-cm
            let loopback_line = match name {
                "--rig" => format!("loopback --open-device {flag}"),
                _ => format!("{loopback} {flag}"),
            };
            match (parsed(&loopback_line), by_loopback) {
                (Ok(_), true) => {}
                (Err(error), false) => assert_eq!(error, format!("{name} is for play")),
                (got, _) => panic!("{loopback_line}: {:?}", got.err()),
            }
        }
        assert_eq!(
            parsed(&format!("{loopback} song")).err().as_deref(),
            Some("loopback takes no bundle, not song")
        );
    }

    #[test]
    fn a_level_or_distance_no_one_can_measure_at_is_refused_before_anything_opens() {
        let loopback = "loopback --open-device --rig";
        for (line, says) in [
            ("cable --level-db 12", "--level-db is at most -3"),
            ("cable --level-db -2", "--level-db is at most -3"),
            ("cable --level-db NaN", "--level-db takes a finite number"),
            ("cable --level-db inf", "--level-db takes a finite number"),
            ("mic --air-cm nan", "--air-cm takes a finite number"),
            ("mic --air-cm inf", "--air-cm takes a finite number"),
            ("mic --air-cm -5", "--rig mic needs the distance"),
            ("cable --input-channel -1", "--input-channel takes 0 to 65535"),
        ] {
            let error = parsed(&format!("{loopback} {line}")).err().unwrap_or_default();
            assert!(error.starts_with(says), "{line}: {error}");
        }
        assert!(matches!(
            parsed(&format!("{loopback} cable --level-db -3")),
            Ok(Command::Loopback(_))
        ));
    }
}
