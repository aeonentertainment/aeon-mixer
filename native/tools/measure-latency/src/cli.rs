use engine::block::RATE;

use crate::physical::Physical;

pub const USAGE: &str = "\
usage:
  measure-latency [--playout-ms MS]
      The engine's share of the mic path, per ingest kind, on the offline driver. Opens nothing.

  measure-latency device --open-device --rig cable|mic [--air-cm CM] [options]
      Plays chirps through a real output and records them at a real input. Needs a build with
      --features hardware. Without --open-device it refuses and opens nothing.

  measure-latency list
      Lists inputs and outputs and their ids. Opens no stream.

rig:
  --rig mic --air-cm CM   a mic CM cm from the speaker. What a room hears: the figure includes
                          the speaker's own processing. CM of air is taken off the figure
  --rig cable             a cable from the output's left channel to the input. Stops at the output
                          jack, so it leaves out anything a powered speaker adds

options:
  --output ID          an id from list; the default output otherwise
  --input ID           an id from list; the default input otherwise
  --input-channel N    the input channel the rig feeds, from 0 (default 0)
  --periods A,B        output periods to ask for, in frames (default 128,64)
  --openings N         times to open the streams at each period (default 3)
  --level-db DB        chirp peak in dBFS (default -12). Set the system volume to speech level
  --machine TEXT       the machine's name in the report, if the OS's name for it is no use
  --playout-ms MS      the jitter buffer's delay in the path table (default 15)
  --shared             Windows: IAudioClient3 shared only
  --exclusive          Windows: exclusive only (default: shared, exclusive if shared cannot open)";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Auto,
    Shared,
    Exclusive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceArgs {
    pub physical: Physical,
    pub output: Option<String>,
    pub input: Option<String>,
    pub input_channel: u16,
    pub periods: Vec<u32>,
    pub openings: u32,
    pub level_db: f32,
    pub machine: Option<String>,
    pub playout_frames: u32,
    pub mode: OutputMode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Offline { playout_frames: u32 },
    Device(DeviceArgs),
    List,
}

const PLAYOUT_MS: f32 = 15.0;

/// Just under the soft clipper's knee of 0.75: the chirp reaches the output unbent.
const MAX_LEVEL_DB: f32 = -3.0;

/// Reads the command line. Opening a device takes both the `device` command and `--open-device`:
/// nothing here touches the sound card of the machine it runs on unless a person says so.
pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let first = args.next();
    if first.as_deref() == Some("list") {
        return match args.next() {
            Some(other) => Err(format!("unknown argument {other}")),
            None => Ok(Command::List),
        };
    }
    let device = first.as_deref() == Some("device");
    let mut args = first.filter(|_| !device).into_iter().chain(args);

    let mut open = false;
    let mut rig = None;
    let mut air_cm = None;
    let mut parsed = DeviceArgs {
        physical: Physical::Cable,
        output: None,
        input: None,
        input_channel: 0,
        periods: vec![128, 64],
        openings: 3,
        level_db: -12.0,
        machine: None,
        playout_frames: playout_frames(PLAYOUT_MS),
        mode: OutputMode::Auto,
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--open-device" => open = true,
            "--rig" => rig = Some(value()?),
            "--air-cm" => air_cm = Some(finite::<f64>(&arg, value()?)?),
            "--output" => parsed.output = Some(value()?),
            "--input" => parsed.input = Some(value()?),
            "--input-channel" => parsed.input_channel = number(&arg, value()?)?,
            "--periods" => {
                parsed.periods = value()?
                    .split(',')
                    .map(|period| number(&arg, period.to_owned()))
                    .collect::<Result<_, _>>()?;
            }
            "--openings" => parsed.openings = number(&arg, value()?)?,
            "--level-db" => parsed.level_db = finite(&arg, value()?)?,
            "--machine" => parsed.machine = Some(value()?),
            "--playout-ms" => {
                let ms = finite(&arg, value()?)?;
                if ms < 0.0 {
                    return Err("--playout-ms is at least 0".into());
                }
                parsed.playout_frames = playout_frames(ms);
            }
            "--shared" => parsed.mode = OutputMode::Shared,
            "--exclusive" => parsed.mode = OutputMode::Exclusive,
            other => return Err(format!("unknown argument {other}")),
        }
    }

    if !device {
        return Ok(Command::Offline { playout_frames: parsed.playout_frames });
    }
    if !open {
        return Err("refusing to open a device: add --open-device to play through it".into());
    }
    if parsed.level_db > MAX_LEVEL_DB {
        return Err(format!("--level-db is at most {MAX_LEVEL_DB}, under the master's clipper"));
    }
    if parsed.periods.is_empty() || parsed.openings == 0 {
        return Err("--periods and --openings need at least one".into());
    }
    parsed.physical = match (rig.as_deref(), air_cm) {
        (Some("cable"), None) => Physical::Cable,
        (Some("mic"), Some(air_cm)) if air_cm >= 0.0 => Physical::Mic { air_cm },
        (Some("cable"), Some(_)) => return Err("--air-cm is for --rig mic".into()),
        (Some("mic"), _) => return Err("--rig mic needs the distance, --air-cm CM".into()),
        (Some(other), _) => return Err(format!("--rig is cable or mic, not {other}")),
        (None, _) => {
            return Err("--rig says what the figure includes: --rig cable or --rig mic".into());
        }
    };
    Ok(Command::Device(parsed))
}

fn playout_frames(ms: f32) -> u32 {
    (ms * RATE as f32 / 1000.0).round() as u32
}

fn number<T: std::str::FromStr>(flag: &str, text: String) -> Result<T, String> {
    text.parse().map_err(|_| format!("{flag} takes a number, not {text}"))
}

// `inf` and `NaN` parse as floats, and none of a delay, a distance or a level can be either.
fn finite<T: std::str::FromStr + Copy + Into<f64>>(flag: &str, text: String) -> Result<T, String> {
    let value: T = number(flag, text.clone())?;
    if value.into().is_finite() {
        Ok(value)
    } else {
        Err(format!("{flag} takes a finite number, not {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Command, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn nothing_opens_a_device_without_being_told_twice() {
        assert_eq!(parse_line(""), Ok(Command::Offline { playout_frames: 720 }));
        assert_eq!(parse_line("--playout-ms 5"), Ok(Command::Offline { playout_frames: 240 }));
        assert_eq!(parse_line("--open-device"), Ok(Command::Offline { playout_frames: 720 }));
        let refused = parse_line("device --rig cable").unwrap_err();
        assert!(refused.contains("--open-device"), "{refused}");
        assert!(matches!(
            parse_line("device --open-device --rig mic --air-cm 10"),
            Ok(Command::Device(DeviceArgs { physical: Physical::Mic { air_cm: 10.0 }, .. }))
        ));
    }

    #[test]
    fn a_figure_cannot_be_taken_without_saying_what_it_includes() {
        assert!(parse_line("device --open-device").is_err());
        assert!(parse_line("device --open-device --rig mic").is_err());
        assert!(parse_line("device --open-device --rig cable --air-cm 5").is_err());
        assert!(parse_line("device --open-device --rig cable --level-db -1").is_err());
    }

    #[test]
    fn a_number_no_one_can_measure_at_is_refused_before_anything_opens() {
        assert!(parse_line("device --open-device --rig mic --air-cm inf").is_err());
        assert!(parse_line("device --open-device --rig mic --air-cm -2").is_err());
        assert!(parse_line("device --open-device --rig cable --level-db NaN").is_err());
        assert!(parse_line("--playout-ms inf").is_err());
        assert!(parse_line("--playout-ms -1").is_err());
    }
}
