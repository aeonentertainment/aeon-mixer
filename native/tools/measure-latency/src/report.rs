//! The tables a person pastes into a PR. Each says what its figures include, because a latency
//! number that does not is no use to the next person who quotes it.

use std::fmt::Write;

use crate::machine;
use crate::offline::{Row, ms};

/// The offline table: the engine's share of each path.
pub fn offline(rows: &[Row]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### measure-latency offline, commit {}", machine::commit());
    let _ = writeln!(out, "\n{}\n", machine::describe(None));
    let _ = writeln!(
        out,
        "| path | corrector | sender gathers | sender encoder | engine holds | total |\n\
         | --- | --- | ---: | ---: | ---: | ---: |"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {:.2} | {:.2} | {} | {} |",
            row.ingest,
            if row.corrected { "on" } else { "off" },
            ms(row.ingest.gather_frames() as f64),
            ms(row.lookahead as f64),
            range(row.held()),
            range(row.lag),
        );
    }
    let playout =
        rows.iter().find(|row| row.ingest.has_playout_delay()).map(|row| row.playout_frames);
    let _ = writeln!(
        out,
        "\nAll in ms, a range where it depends on where in a block the stream began. No device, no \
         socket, no network, on the offline driver. **Sender gathers**: the frame or chunk a \
         sender fills before any of it can leave. **Sender encoder**: Opus's lookahead, which the \
         phone pays. **Engine holds**: from the packet's arrival to the master's output: the wait \
         for the next block, {} ms of playout delay on the jitter-buffered paths, decode, slot and \
         master. **Total**: a sample entering the sender to its leaving the master; it excludes \
         the mouth to the phone's encoder, the network, and everything after the master.",
        playout.map_or("no".into(), |frames| format!("{:.2}", ms(f64::from(frames)))),
    );
    out
}

/// A range of frames, in milliseconds, or one figure when it is all one.
pub fn range((least, most): (f64, f64)) -> String {
    let (least, most) = (ms(least), ms(most));
    if most - least < 0.01 { format!("{least:.2}") } else { format!("{least:.2} to {most:.2}") }
}
