//! The tables a person pastes into a PR. Each says what its figures include, because a latency
//! number that does not is no use to the next person who quotes it.

use std::fmt::Write;

use crate::cli::DeviceArgs;
use crate::machine;
use crate::offline::{Row, ms};
use crate::opening::Opening;
use crate::programme::Programme;
use crate::reading::median;

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

/// The device tables: one row per opening, then every path with the output's figure added on.
pub fn device(
    args: &DeviceArgs,
    programme: &Programme,
    openings: &[Opening],
    rows: &[Row],
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### measure-latency device, commit {}", machine::commit());
    let _ = writeln!(out, "\n{}\n", machine::describe(args.machine.as_deref()));
    for opened in distinct(openings.iter().map(|opening| &opening.output_opened)) {
        let _ = writeln!(out, "- output: {opened}");
    }
    for opened in distinct(openings.iter().map(|opening| &opening.input_opened)) {
        let _ = writeln!(out, "- input: {opened}");
    }
    let _ = writeln!(out, "- rig: {}", args.physical.describe());
    let _ = writeln!(out, "- signal: {}", signal(programme, args.level_db));
    let air = args.physical.air_ms();
    let _ = writeln!(
        out,
        "\n| period asked, got | input period | OS says output | output at most | output if the OS's \
         input figure is true | delivered | moved | render spread | SNR dB | input peak dBFS | \
         underruns | realtime |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | \
         ---: | ---: | --- |"
    );
    for opening in openings {
        let _ = writeln!(out, "{}", opening_row(opening, air));
    }

    let _ = writeln!(
        out,
        "\n| path | corrector | offline total | output at most | mic path at most |\n\
         | --- | --- | ---: | ---: | ---: |"
    );
    for period in distinct_periods(openings) {
        let readings: Vec<f64> = openings
            .iter()
            .filter(|opening| opening.output_period == period)
            .filter_map(|opening| opening.reading.as_ref().ok())
            .map(|reading| reading.median() - air)
            .collect();
        if readings.is_empty() {
            continue;
        }
        let output = median(&readings);
        // A longer callback than the block drains packets less often, which the offline run, whose
        // callback is one block, cannot see
        let extra = f64::from(period.saturating_sub(engine::block::BLOCK as u32));
        for row in rows {
            let (least, most) = (ms(row.lag.0), ms(row.lag.1 + extra));
            let _ = writeln!(
                out,
                "| {}, period {period} | {} | {:.2} to {:.2} | {output:.2} | {:.2} to {:.2} |",
                row.ingest,
                if row.corrected { "on" } else { "off" },
                least,
                most,
                least + output,
                most + output,
            );
        }
    }
    let _ = writeln!(out, "\n{}", DEVICE_NOTES);
    out
}

const DEVICE_NOTES: &str = "All in ms. Every figure runs from a sample's mean render time. **Output \
at most**: to the moment the sample could first have been woken for at the input, median of the \
chirps. It includes the output's buffers, the DAC and the speaker or cable. It excludes the air \
named above and the input's period, which is dated back out. It still includes what the input's \
hardware and driver hold before the OS can wake anyone, which no software can see, so it is an \
upper bound. A difference between two rows with one input has that share cancelled and is exact. \
**Output if the OS's input figure is true**: the bound less the input latency the OS reports, \
blank when it reports none. **OS says output**: `StreamInfo`, the figure decision 0003 quotes. \
**Delivered**: to the wake that handed the sample over, the input's period included, which is the \
house-mic path's own cost. **Moved**: most to least across the chirps of that opening; over 0.75 means no number. **Render spread**: how far block \
renders stand from their mean, most to least, so a long callback's bursts show. **Mic path \
at most**: the offline total plus the output: a sample entering the sender to its leaving the \
speaker. It excludes the mouth to the phone's encoder, the network, and the air to the singer.";

fn opening_row(opening: &Opening, air: f64) -> String {
    let asked = format!("{}, {}", opening.asked, opening.output_period);
    let os_output = ms(f64::from(opening.output_latency_frames));
    let input_period = opening.input_period;
    let realtime = if opening.realtime { "yes" } else { "no" };
    match &opening.reading {
        Ok(reading) => {
            let bound = reading.median() - air;
            let trusted = opening
                .input_latency_frames
                .map_or(String::new(), |frames| format!("{:.2}", bound - ms(f64::from(frames))));
            format!(
                "| {asked} | {input_period} | {os_output:.2} | {bound:.2} | {trusted} | {:.2} | \
                 {:.2} | {:.2} | {:.0} | {:.0} | {} | {realtime} |",
                reading.delivered - air,
                reading.moved(),
                reading.render_spread,
                reading.snr_db,
                reading.peak_db,
                opening.underruns,
            )
        }
        Err(refused) => format!(
            "| {asked} | {input_period} | {os_output:.2} | no number: {refused} | | | | | | | {} | \
             {realtime} |",
            opening.underruns
        ),
    }
}

fn distinct<'a>(lines: impl Iterator<Item = &'a String>) -> Vec<&'a String> {
    let mut seen: Vec<&String> = Vec::new();
    for line in lines {
        if !seen.contains(&line) {
            seen.push(line);
        }
    }
    seen
}

fn distinct_periods(openings: &[Opening]) -> Vec<u32> {
    let mut periods: Vec<u32> = openings.iter().map(|opening| opening.output_period).collect();
    periods.sort_unstable();
    periods.dedup();
    periods
}

/// What the device run played, in words.
pub fn signal(programme: &Programme, level_db: f32) -> String {
    format!(
        "{} chirps an opening, {:.1} to {:.1} kHz over {} ms, {level_db:.0} dBFS, left channel, \
         through a real l16 slot",
        programme.repeats,
        programme.chirp.low_hz / 1000.0,
        programme.chirp.high_hz / 1000.0,
        ms(programme.chirp.frames as f64),
    )
}
