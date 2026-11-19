use std::path::PathBuf;

use replay::bundle::report::Options as Replay;

pub const USAGE: &str = "\
usage: bench play BUNDLE [--backing STEMS] [--profile FILE] [--out DIR] [--baseline FILE] [--write FILE]
                         [--open-device [--output ID] [--period 64|128]]

  Plays a session bundle through the mixer on a real clock, as replay plays it on the offline
  driver, and says whether the mix kept up. Without --open-device it runs on the timer clock,
  which opens nothing and takes the song's length.

  BUNDLE           a session bundle's directory, as createMixer({ bundle }) writes it
  --backing STEMS  a directory holding the song's stems, found by hash; without it the mic path
                   plays alone
  --profile FILE   play under this settings profile, not the bundle's own
  --out DIR        write the bundle of this run into DIR, which must not exist yet
  --baseline FILE  an earlier bench result: report each number that moved, and fail on any it
                   does not allow. A bench figure belongs to its machine
  --write FILE     write this run's result, with the baseline's tolerances where it has them
  --open-device    play through a real output. Needs a build with --features hardware
  --output ID      an output id from `measure-latency list`; the default output otherwise
  --period N       the output's period in frames, 64 or 128 (default 128)";

/// Where the mix goes: the timer clock, or a real output.
#[derive(Debug, PartialEq)]
pub enum Clock {
    Timer,
    Device { output: Option<String>, period: u32 },
}

pub struct Args {
    pub bundle: PathBuf,
    pub replay: Replay,
    pub out: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub write: Option<PathBuf>,
    pub clock: Clock,
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("play") {
        return Err("name a command: play".into());
    }
    let mut bundle = None;
    let (mut replay, mut out, mut baseline, mut write) = (Replay::default(), None, None, None);
    let (mut open_device, mut output, mut period) = (false, None, None);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--backing" => replay.backing = Some(PathBuf::from(value()?)),
            "--profile" => replay.profile = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--baseline" => baseline = Some(PathBuf::from(value()?)),
            "--write" => write = Some(PathBuf::from(value()?)),
            "--open-device" => open_device = true,
            "--output" => output = Some(value()?),
            "--period" => match value()?.as_str() {
                "64" => period = Some(64),
                "128" => period = Some(128),
                other => return Err(format!("--period takes 64 or 128, not {other}")),
            },
            flag if flag.starts_with("--") => return Err(format!("no flag {flag}")),
            path if bundle.is_none() => bundle = Some(PathBuf::from(path)),
            path => return Err(format!("one bundle at a time, not {path} as well")),
        }
    }
    let clock = if open_device {
        Clock::Device { output, period: period.unwrap_or(128) }
    } else if output.is_some() || period.is_some() {
        return Err("--output and --period choose a real output, so they need --open-device".into());
    } else {
        Clock::Timer
    };
    let bundle = bundle.ok_or("name a bundle")?;
    Ok(Args { bundle, replay, out, baseline, write, clock })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Args, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn a_real_output_is_only_ever_opened_by_name() {
        assert_eq!(parsed("play song").unwrap().clock, Clock::Timer);
        assert!(parsed("play song --output x").is_err());
        assert!(parsed("play song --period 64").is_err());
        assert_eq!(
            parsed("play song --open-device --period 64").unwrap().clock,
            Clock::Device { output: None, period: 64 }
        );
        assert!(parsed("play song --open-device --period 256").is_err());
    }
}
