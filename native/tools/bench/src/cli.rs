use std::path::PathBuf;

use measure_latency::physical::Physical;
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

pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    if args.peek().is_some_and(|first| matches!(first.as_str(), "help" | "--help" | "-h")) {
        return Ok(Command::Help);
    }
    let command = args.next();
    let mut bundle = None;
    let (mut replay, mut out, mut baseline, mut write) = (Replay::default(), None, None, None);
    let (mut open_device, mut output, mut period) = (false, None, None);
    let (mut rig, mut air_cm, mut input, mut input_channel, mut level_db) =
        (None, None, None, 0, -12.0);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--backing" => replay.backing = Some(PathBuf::from(value()?)),
            "--profile" => replay.profile = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--baseline" => baseline = Some(PathBuf::from(value()?)),
            "--write" => write = Some(PathBuf::from(value()?)),
            "--open-device" => open_device = true,
            "--help" | "-h" => return Ok(Command::Help),
            "--output" => output = Some(value()?),
            "--input" => input = Some(value()?),
            "--period" => match value()?.as_str() {
                "64" => period = Some(64),
                "128" => period = Some(128),
                other => return Err(format!("--period takes 64 or 128, not {other}")),
            },
            "--rig" => rig = Some(value()?),
            "--air-cm" => {
                air_cm = Some(value()?.parse::<f64>().map_err(|_| "--air-cm takes centimetres")?)
            }
            "--input-channel" => {
                input_channel = value()?.parse().map_err(|_| "--input-channel takes a number")?
            }
            "--level-db" => level_db = value()?.parse().map_err(|_| "--level-db takes dBFS")?,
            flag if flag.starts_with("--") => return Err(format!("no flag {flag}")),
            path if bundle.is_none() => bundle = Some(PathBuf::from(path)),
            path => return Err(format!("one bundle at a time, not {path} as well")),
        }
    }
    match command.as_deref() {
        Some("play") => {
            let clock = if open_device {
                Clock::Device { output, period: period.unwrap_or(128) }
            } else if output.is_some() || period.is_some() {
                return Err(
                    "--output and --period choose a real output, so they need --open-device".into(),
                );
            } else {
                Clock::Timer
            };
            let bundle = bundle.ok_or("name a bundle")?;
            Ok(Command::Play(Args { bundle, replay, out, baseline, write, clock }))
        }
        Some("loopback") => {
            if !open_device {
                return Err(
                    "loopback plays through a real output and input: add --open-device".into()
                );
            }
            let rig = match (rig.as_deref(), air_cm) {
                (Some("cable"), None) => Physical::Cable,
                (Some("mic"), Some(air_cm)) => Physical::Mic { air_cm },
                (Some("mic"), None) => return Err("--rig mic needs --air-cm".into()),
                (Some("cable"), Some(_)) => return Err("--air-cm is for --rig mic".into()),
                _ => return Err("name the rig: --rig cable, or --rig mic --air-cm CM".into()),
            };
            let loopback = Loopback { rig, level_db };
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
        _ => Err("name a command: play or loopback".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(line: &str) -> Result<Clock, String> {
        match parse(line.split_whitespace().map(str::to_owned))? {
            Command::Play(args) => Ok(args.clock),
            Command::Loopback(_) => Ok(Clock::Device { output: None, period: 0 }),
            Command::Help => Err("help".into()),
        }
    }

    #[test]
    fn a_real_device_is_only_ever_opened_by_name() {
        assert_eq!(clock("play song").unwrap(), Clock::Timer);
        assert!(clock("play song --output x").is_err());
        assert!(clock("play song --period 64").is_err());
        assert_eq!(
            clock("play song --open-device --period 64").unwrap(),
            Clock::Device { output: None, period: 64 }
        );
        assert!(clock("play song --open-device --period 256").is_err());
        assert!(clock("loopback --rig cable").is_err());
        assert!(clock("loopback --open-device --rig cable").is_ok());
        assert!(
            clock("").is_err_and(|error| error != "help"),
            "nothing asked is a bad command line"
        );
        for line in ["--help", "help", "play song --help", "loopback -h"] {
            assert_eq!(clock(line).err().as_deref(), Some("help"), "{line:?} asks for the usage");
        }
    }
}
