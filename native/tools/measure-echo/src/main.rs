//! The echo canceller's numbers. Opens no device.
//!
//! `measure-echo harness <dir>` replays Aeon Karaoke's `aec:measure` files.
//! `measure-echo cpu` times the canceller per slot. Build with `--release`.
//! `measure-echo programme <out.wav>` and `measure-echo bleed <programme.wav> <take.wav>` are the
//! bleed measurement with a phone at the mouth.

mod bleed;
mod cancel;
mod cpu;
mod harness;
mod wav;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: measure-echo harness <dir> | cpu | programme <out.wav> | bleed <programme.wav> <take.wav>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["harness", dir] => harness::run(&PathBuf::from(dir)).map(|scores| {
            println!(
                "{:<22} {:>8} {:>10} {:>10} {:>9}",
                "scenario", "ERLE dB", "15 dB at", "singer dB", "path ms"
            );
            for s in scores {
                let at = s.converged_at.map_or("never".into(), |at| format!("{at} s"));
                let path = s.path_ms.map_or("none".into(), |ms| format!("{ms:.1}"));
                println!(
                    "{:<22} {:>8.1} {:>10} {:>+10.1} {:>9}",
                    s.scenario, s.erle_db, at, s.talker_delta_db, path
                );
            }
        }),
        ["programme", path] => bleed::programme(&PathBuf::from(path)),
        ["bleed", programme, take] => {
            bleed::measure(&PathBuf::from(programme), &PathBuf::from(take)).map(|b| {
                println!("the programme is heard {:.0} ms into the take", b.heard_at_ms);
                println!("the speakers reach the phone at {:.1} dBFS", b.bleed_dbfs);
                match b.voice_dbfs {
                    Some(voice) => println!(
                        "the voice reaches it at {voice:.1} dBFS: {:.1} dB over the bleed",
                        voice - b.bleed_dbfs
                    ),
                    None => println!("no voice over the second half: sing from 16 s"),
                }
                if b.found {
                    println!(
                        "offline, the canceller takes {:.1} dB out of the bleed",
                        b.removed_db
                    );
                } else {
                    println!("offline, the canceller found no echo to take out");
                }
            })
        }
        ["cpu"] => {
            let cpu = cpu::run();
            println!(
                "canceller {:.2} % of a core per slot, {:.1} us a block",
                cpu.canceller, cpu.block_micros
            );
            Ok(())
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
