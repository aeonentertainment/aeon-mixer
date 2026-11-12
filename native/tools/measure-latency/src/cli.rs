use engine::block::RATE;

pub const USAGE: &str = "\
usage:
  measure-latency [--playout-ms MS]
      The engine's share of the mic path, per ingest kind, on the offline driver. Opens nothing.

options:
  --playout-ms MS      the jitter buffer's delay (default 15)";

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Offline { playout_frames: u32 },
}

const PLAYOUT_MS: f32 = 15.0;

/// Reads the command line.
pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut playout_ms = PLAYOUT_MS;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--playout-ms" => playout_ms = finite(&arg, value()?)?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if playout_ms < 0.0 {
        return Err("--playout-ms is at least 0".into());
    }
    Ok(Command::Offline { playout_frames: playout_frames(playout_ms) })
}

fn playout_frames(ms: f32) -> u32 {
    (ms * RATE as f32 / 1000.0).round() as u32
}

fn number<T: std::str::FromStr>(flag: &str, text: String) -> Result<T, String> {
    text.parse().map_err(|_| format!("{flag} takes a number, not {text}"))
}

// `inf` and `NaN` parse as floats, and neither is a delay or a level anyone can measure at.
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
    fn the_playout_delay_is_read_in_milliseconds_and_anything_else_is_refused() {
        assert_eq!(parse_line(""), Ok(Command::Offline { playout_frames: 720 }));
        assert_eq!(parse_line("--playout-ms 5"), Ok(Command::Offline { playout_frames: 240 }));
        for refused in ["five", "inf", "NaN", "-1"] {
            assert!(parse_line(&format!("--playout-ms {refused}")).is_err(), "{refused}");
        }
        assert!(parse_line("device").is_err());
    }
}
