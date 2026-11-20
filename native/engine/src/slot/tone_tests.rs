//! The tone stage through a whole graph on the offline driver: off is no stage, a click leaves on
//! the sample it entered, the readings never change, a change glides, and every delay decision
//! 0029 computed from the filters' formulas, measured with tones. Every block is pumped under the
//! allocation guard.

use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use assert_no_alloc::assert_no_alloc;
use kernels::fixtures::{Lcg, at_db, talker};

use super::*;
use crate::backing::{Backing, Planar, Stems, Transport};
use crate::command::event_ring;
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::params::{Params, ToneBand, ToneBands, ToneParams};
use crate::slot::effects::tone::{Biquad, MOST_LATE_MS, Section};
use crate::slot::source::fixtures::Clip;
use crate::slot::tone_report::ToneFindings;

const SECOND: usize = RATE as usize;
const MS: usize = SECOND / 1000;

struct Run {
    left: Vec<f32>,
    events: Vec<Event>,
    /// What stats would have read after each block.
    findings: Vec<Option<ToneFindings>>,
}

/// `input` through a one-slot graph hard left, so the left channel is the slot's mono output, with
/// the tone stage at `tone(block)` for each block and every other effect off. The backing plays
/// silence from frame 0, as a song's would.
fn on_the_offline_driver(
    input: &[f32],
    mut tone: impl FnMut(usize) -> ToneParams + Send + 'static,
) -> Run {
    let slot = Slot::new(Box::new(Clip::new(input.to_vec())));
    let report = slot.tone_report();
    let mut graph = Graph::new(vec![slot]);
    let (mut events, mut drain) = event_ring();
    let mut backing = Planar::default();
    backing.pad_to(2 * input.len());
    graph.set_backing(Backing::new(Stems::new(backing, None), 0));
    graph.steer(1, Transport::Start, &mut events);
    let mut params = Params::default();
    params.slots[0].pan = -1.0;
    let mut block = 0;
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        params.slots[0].effects.tone = tone(block);
        block += 1;
        graph.render(&params, out, &mut events);
    }));

    let mut run =
        Run { left: Vec::with_capacity(input.len()), events: Vec::new(), findings: Vec::new() };
    let mut interleaved = [0.0; 2 * BLOCK];
    while run.left.len() < input.len() {
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        run.left.extend(interleaved.iter().step_by(2));
        run.findings.push(report.findings());
        run.events.extend(std::iter::from_fn(|| drain.pop()));
    }
    run.left.truncate(input.len());
    run
}

fn given(bands: &[ToneBand]) -> ToneBands {
    ToneBands::given(bands).expect("at most eight bands")
}

/// A profile at the late cap: a low shelf the phone needs, a presence lift, a softer top and a
/// notch on a port resonance, with air. `check` takes it, and its worst frequency is within
/// 0.01 ms of the cap.
fn at_the_cap() -> ToneParams {
    ToneParams {
        enabled: true,
        bands: given(&[
            ToneBand::LowShelf { hz: 200.0, gain_db: 2.48 },
            ToneBand::Peak { hz: 3000.0, gain_db: 4.0, q: 1.0 },
            ToneBand::HighShelf { hz: 8000.0, gain_db: -3.0 },
            ToneBand::Peak { hz: 9000.0, gain_db: -6.0, q: 4.0 },
        ]),
        air_db: 3.0,
        ..ToneParams::default()
    }
}

/// Every part on: the bands at the cap, the guard and the de-esser at their deepest.
fn everything() -> ToneParams {
    ToneParams { plosive_db: 12.0, de_ess_db: 12.0, ..at_the_cap() }
}

fn loudest_from(signal: &[f32], from: usize, frames: usize) -> usize {
    (from..from + frames).max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs())).unwrap()
}

// The done-when: off with bands and amounts set, or on with no bands and every amount at 0, the
// slot is bit for bit what it is with no stage, a click included
#[test]
fn an_off_stage_is_no_stage_bit_for_bit() {
    let mut input = at_db(talker(SECOND, 110.0, 3), -20.0);
    input[SECOND / 2] = 0.7;
    let none = on_the_offline_driver(&input, |_| ToneParams::default()).left;
    let off = [
        ToneParams { enabled: false, ..everything() },
        ToneParams { enabled: true, ..ToneParams::default() },
    ];
    for tone in off {
        let run = on_the_offline_driver(&input, move |_| tone);
        for (frame, (out, alone)) in run.left.iter().zip(&none).enumerate() {
            assert_eq!(out.to_bits(), alone.to_bits(), "{tone:?} changed frame {frame}");
        }
    }
}

/// Noise under 40 Hz, up in 1 ms and down over 15: a pop far enough under the voice to duck.
fn pop(frames: usize, peak: f32) -> Vec<f32> {
    let mut noise = Lcg::new(9);
    let mut low = Section::default();
    let mut lower = Section::default();
    let pass = Biquad::low_pass(40.0);
    let raw: Vec<f32> = (0..frames)
        .map(|_| lower.tick(&pass, low.tick(&pass, f64::from(noise.sample()))) as f32)
        .collect();
    let most = raw.iter().fold(0.0f32, |most, s| most.max(s.abs()));
    raw.iter()
        .enumerate()
        .map(|(i, s)| {
            let ms = i as f32 / MS as f32;
            peak * ms.min(1.0) * (-(ms - 1.0).max(0.0) / 15.0).exp() * s / most
        })
        .collect()
}

// The done-when: a click leaves on the sample it entered with the bands at the cap, with the guard
// ducking a pop and with the de-esser cutting an s
#[test]
fn a_click_leaves_on_the_sample_it_entered() {
    let mut input = vec![0.0; 2 * SECOND];
    let alone = SECOND / 5;
    input[alone] = 0.7;
    let pop_at = SECOND / 2;
    for (i, sample) in pop(60 * MS, 0.15).into_iter().enumerate() {
        input[pop_at + i] += sample;
    }
    let ducking = pop_at + 20 * MS;
    input[ducking] += 0.7;
    let s_at = SECOND;
    let mut noise = Lcg::new(4);
    let mut previous = 0.0;
    for sample in &mut input[s_at..s_at + 80 * MS] {
        let white = noise.sample();
        *sample += 0.1 * (white - previous);
        previous = white;
    }
    let cutting = s_at + 50 * MS;
    input[cutting] += 0.7;

    let run = on_the_offline_driver(&input, |_| everything());
    for (name, click) in [("alone", alone), ("in a pop", ducking), ("in an s", cutting)] {
        assert_eq!(loudest_from(&run.left, click - 5 * MS, 10 * MS), click, "a click {name} moved");
    }
    let at = |frame: usize| run.findings[frame / BLOCK].unwrap_or_default();
    assert_eq!(
        (at(ducking - BLOCK).plosives, at(alone).plosives),
        (1, 0),
        "the guard was not ducking"
    );
    assert!(at(cutting - BLOCK).de_ess_db > 3.0, "the de-esser was not cutting: {:?}", at(cutting));
}

// The done-when: every pitch reading and level word is the same with the stage on as off, since it
// runs after the corrector and the meter
#[test]
fn every_reading_is_the_same_with_the_stage_on() {
    let input = at_db(talker(2 * SECOND, 110.0, 8), -18.0);
    let readings = |tone: ToneParams| -> Vec<Event> {
        on_the_offline_driver(&input, move |_| tone)
            .events
            .into_iter()
            .filter(|event| matches!(event, Event::Pitch { .. } | Event::Level { .. }))
            .collect()
    };
    let off = readings(ToneParams::default());
    assert!(off.iter().filter(|e| matches!(e, Event::Level { .. })).count() >= 19, "{off:?}");
    assert!(off.iter().filter(|e| matches!(e, Event::Pitch { .. })).count() >= 190, "{off:?}");
    assert_eq!(readings(everything()), off);
}

// A change glides across a block and never steps: a 1 kHz tone through a 500 Hz shelf that takes
// 11 dB off it, then a 2 kHz one, then none, moves at most 1.25 times its own slope between two
// samples, where switching the same filters at a block's edge steps it 5 times. Once the last
// shelf has rung out, within a few blocks of off, the voice is bit for bit the source's again.
#[test]
fn a_change_glides_and_never_steps() {
    const AMPLITUDE: f64 = 0.25;
    let input: Vec<f32> = (0..SECOND)
        .map(|i| (AMPLITUDE * (TAU * 1000.0 * i as f64 / f64::from(RATE)).sin()) as f32)
        .collect();
    let shelf = |hz| ToneParams {
        enabled: true,
        bands: given(&[ToneBand::HighShelf { hz, gain_db: -12.0 }]),
        ..ToneParams::default()
    };
    let run = on_the_offline_driver(&input, move |block| match block {
        100..150 => shelf(500.0),
        150..200 => shelf(2000.0),
        _ => ToneParams::default(),
    });
    let slope = AMPLITUDE * TAU * 1000.0 / f64::from(RATE);
    let steepest =
        run.left.windows(2).map(|pair| f64::from((pair[1] - pair[0]).abs())).fold(0.0, f64::max);
    assert!(steepest < 1.25 * slope, "moved {steepest:.4} between samples, its slope {slope:.4}");
    let during = &run.left[120 * BLOCK..148 * BLOCK];
    let most = during.iter().fold(0.0f32, |most, s| most.max(s.abs()));
    let lost = 20.0 * (AMPLITUDE / f64::from(most)).log10();
    assert!((10.6..=11.6).contains(&lost), "the shelf took {lost:.2} dB off");
    for frame in 205 * BLOCK..run.left.len() {
        assert_eq!(run.left[frame].to_bits(), input[frame].to_bits(), "frame {frame} after off");
    }
}

// A narrow, slow band the caps take, leaving: its poles take longer than a block to settle, so
// the voice it leaves rings out through the band's flat until it has, and never steps. A 110 Hz
// tone under a -0.5 dB notch of Q 8 moves at most 1.05 times its own slope between two samples.
#[test]
fn a_slow_band_leaving_rings_out_without_a_step() {
    const AMPLITUDE: f64 = 0.25;
    let input: Vec<f32> = (0..SECOND)
        .map(|i| (AMPLITUDE * (TAU * 110.0 * i as f64 / f64::from(RATE)).sin()) as f32)
        .collect();
    let notch = ToneParams {
        enabled: true,
        bands: given(&[ToneBand::Peak { hz: 110.0, gain_db: -0.5, q: 8.0 }]),
        ..ToneParams::default()
    };
    assert_eq!(notch.check(), Ok(()));
    let run =
        on_the_offline_driver(
            &input,
            move |block| if block < 150 { notch } else { ToneParams::default() },
        );
    let slope = AMPLITUDE * TAU * 110.0 / f64::from(RATE);
    let (at, steepest) = run
        .left
        .windows(2)
        .enumerate()
        .skip(100 * BLOCK)
        .map(|(i, pair)| (i, f64::from((pair[1] - pair[0]).abs())))
        .fold((0, 0.0), |most, point| if point.1 > most.1 { point } else { most });
    assert!(steepest < 1.05 * slope, "moved {steepest:.5} at frame {at}, its slope {slope:.5}");
}

/// The analysis window, a second, so each tone sits on a 1 Hz bin and a pair of neighbours reads
/// the phase's slope; and what precedes it, long enough for the narrowest band to ring in.
const WINDOW: usize = SECOND;
const SETTLE: usize = SECOND / 4;

/// The lower bin of each pair measured: every other hertz from 80 to 400, where the bands are
/// narrowest in hertz, then 1.5 % apart to 16 kHz.
fn pairs() -> &'static [usize] {
    static PAIRS: OnceLock<Vec<usize>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut pairs: Vec<usize> = (80..400).step_by(2).collect();
        let mut hz = 400.0f64;
        while hz < 16_000.0 {
            let bin = hz.round() as usize;
            if bin >= pairs[pairs.len() - 1] + 2 {
                pairs.push(bin);
            }
            hz *= 1.015;
        }
        pairs
    })
}

/// Both tones of every pair at once, 0.02 RMS in all, Schroeder's phases so they never pile into
/// a peak the master's clipper would bend.
fn tones() -> &'static [f32] {
    static TONES: OnceLock<Vec<f32>> = OnceLock::new();
    TONES.get_or_init(|| {
        let bins: Vec<usize> = pairs().iter().flat_map(|&k| [k, k + 1]).collect();
        let amplitude = 0.02 * (2.0 / bins.len() as f64).sqrt();
        let mut signal = vec![0.0f64; SETTLE + WINDOW];
        for (i, &bin) in bins.iter().enumerate() {
            let phase = PI * (i * i) as f64 / bins.len() as f64;
            let step = TAU * bin as f64 / f64::from(RATE);
            let (mut re, mut im) = (phase.cos(), phase.sin());
            let (step_re, step_im) = (step.cos(), step.sin());
            for sample in &mut signal {
                *sample += amplitude * im;
                (re, im) = (re * step_re - im * step_im, re * step_im + im * step_re);
            }
        }
        signal.into_iter().map(|s| s as f32).collect()
    })
}

/// `signal`'s complex content at `bin` hertz over the window, by Goertzel, up to a phase that every
/// signal measured at that bin shares.
fn at_bin(signal: &[f32], bin: usize) -> (f64, f64) {
    let w = TAU * bin as f64 / WINDOW as f64;
    let coefficient = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for sample in &signal[SETTLE..SETTLE + WINDOW] {
        let s0 = f64::from(*sample) + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 - w.cos() * s2, w.sin() * s2)
}

/// What the tones say the stage does at each pair: the gain at its lower bin and the group delay
/// between its two, at the midpoint, from how far the phase turned over that hertz.
struct Measured {
    gain_db: Vec<(f64, f64)>,
    delay_ms: Vec<(f64, f64)>,
}

impl Measured {
    fn latest(&self) -> (f64, f64) {
        self.delay_ms
            .iter()
            .copied()
            .fold((0.0, f64::MIN), |worst, point| if point.1 > worst.1 { point } else { worst })
    }
}

/// The tones, plus a `holder` tone that keeps the guard or the de-esser at its deepest, through the
/// engine at `tone`.
fn measure(tone: ToneParams, holder: Option<(f64, f64)>) -> Measured {
    let mut input = tones().to_vec();
    if let Some((hz, amplitude)) = holder {
        for (i, sample) in input.iter_mut().enumerate() {
            *sample += (amplitude * (TAU * hz * i as f64 / f64::from(RATE)).sin()) as f32;
        }
    }
    let output = on_the_offline_driver(&input, move |_| tone).left;
    let response = |bin: usize| {
        let (x_re, x_im) = at_bin(&input, bin);
        let (y_re, y_im) = at_bin(&output, bin);
        let power = x_re * x_re + x_im * x_im;
        ((y_re * x_re + y_im * x_im) / power, (y_im * x_re - y_re * x_im) / power)
    };
    let mut measured = Measured { gain_db: Vec::new(), delay_ms: Vec::new() };
    for &bin in pairs() {
        let (lower, upper) = (response(bin), response(bin + 1));
        measured.gain_db.push((bin as f64, 10.0 * (lower.0 * lower.0 + lower.1 * lower.1).log10()));
        let turned =
            (upper.1 * lower.0 - upper.0 * lower.1).atan2(upper.0 * lower.0 + upper.1 * lower.1);
        measured.delay_ms.push((bin as f64 + 0.5, -turned / TAU * 1000.0));
    }
    measured
}

/// The cookbook's analog prototypes at the frequency the bilinear transform maps `hz` to: the
/// curve a host's fitting tool draws, derived apart from the engine's coefficients.
fn cookbook_db(band: &ToneBand, hz: f64) -> f64 {
    let warp = |f: f64| (PI * f / f64::from(RATE)).tan();
    let gain = |band_hz: f32, gain_db: f32| {
        (warp(hz) / warp(f64::from(band_hz)), 10f64.powf(f64::from(gain_db) / 40.0))
    };
    let ratio = |n: (f64, f64), d: (f64, f64)| {
        10.0 * ((n.0 * n.0 + n.1 * n.1) / (d.0 * d.0 + d.1 * d.1)).log10()
    };
    match *band {
        ToneBand::Peak { hz: band_hz, gain_db, q } => {
            let (w, a) = gain(band_hz, gain_db);
            let q = f64::from(q);
            ratio((1.0 - w * w, w * a / q), (1.0 - w * w, w / (a * q)))
        }
        ToneBand::LowShelf { hz: band_hz, gain_db } => {
            let (w, a) = gain(band_hz, gain_db);
            let side = a.sqrt() * 2f64.sqrt() * w;
            20.0 * a.log10() + ratio((a - w * w, side), (1.0 - a * w * w, side))
        }
        ToneBand::HighShelf { hz: band_hz, gain_db } => {
            let (w, a) = gain(band_hz, gain_db);
            let side = a.sqrt() * 2f64.sqrt() * w;
            20.0 * a.log10() + ratio((1.0 - a * w * w, side), (a - w * w, side))
        }
    }
}

/// One figure decision 0029 computed: what it names, the stage set to it, the tone that holds a
/// moving part at its deepest, and where the record puts the latest frequency and how late.
struct Row {
    name: &'static str,
    tone: ToneParams,
    holder: Option<(f64, f64)>,
    record: (Option<f64>, f64),
}

fn bands_row(name: &'static str, bands: &[ToneBand], record: (Option<f64>, f64)) -> Row {
    Row {
        name,
        tone: ToneParams { enabled: true, bands: given(bands), ..ToneParams::default() },
        holder: None,
        record,
    }
}

fn rows() -> Vec<Row> {
    use ToneBand::{HighShelf, LowShelf, Peak};
    vec![
        bands_row(
            "peak +12 dB, Q 4, 100 Hz",
            &[Peak { hz: 100.0, gain_db: 12.0, q: 4.0 }],
            (Some(100.0), 19.0),
        ),
        bands_row(
            "peak +6 dB, Q 1, 100 Hz",
            &[Peak { hz: 100.0, gain_db: 6.0, q: 1.0 }],
            (Some(96.0), 2.3),
        ),
        bands_row(
            "peak +6 dB, Q 2, 1 kHz",
            &[Peak { hz: 1000.0, gain_db: 6.0, q: 2.0 }],
            (Some(1000.0), 0.45),
        ),
        bands_row(
            "peak +6 dB, Q 1, 3 kHz",
            &[Peak { hz: 3000.0, gain_db: 6.0, q: 1.0 }],
            (Some(2900.0), 0.08),
        ),
        bands_row(
            "peak -12 dB, Q 4, 200 Hz",
            &[Peak { hz: 200.0, gain_db: -12.0, q: 4.0 }],
            (Some(155.0), 1.1),
        ),
        bands_row(
            "peak -9 dB, Q 4, 9 kHz",
            &[Peak { hz: 9000.0, gain_db: -9.0, q: 4.0 }],
            (Some(7400.0), 0.02),
        ),
        bands_row(
            "peak -6 dB, Q 1, 300 Hz",
            &[Peak { hz: 300.0, gain_db: -6.0, q: 1.0 }],
            (None, 0.35),
        ),
        bands_row(
            "peak +3 dB, Q 2, 400 Hz",
            &[Peak { hz: 400.0, gain_db: 3.0, q: 2.0 }],
            (None, 0.56),
        ),
        bands_row(
            "low shelf +6 dB, 200 Hz",
            &[LowShelf { hz: 200.0, gain_db: 6.0 }],
            (Some(95.0), 0.53),
        ),
        bands_row("low shelf +4 dB, 200 Hz", &[LowShelf { hz: 200.0, gain_db: 4.0 }], (None, 0.35)),
        bands_row(
            "low shelf +2 dB, 200 Hz",
            &[LowShelf { hz: 200.0, gain_db: 2.0 }],
            (Some(97.0), 0.18),
        ),
        bands_row(
            "high shelf -12 dB, 5 kHz",
            &[HighShelf { hz: 5000.0, gain_db: -12.0 }],
            (Some(2300.0), 0.04),
        ),
        bands_row(
            "low shelf -12 dB, 150 Hz",
            &[LowShelf { hz: 150.0, gain_db: -12.0 }],
            (Some(227.0), 0.45),
        ),
        bands_row(
            "the match's worst combination",
            &[
                LowShelf { hz: 400.0, gain_db: 3.0 },
                Peak { hz: 3000.0, gain_db: -6.0, q: 0.7 },
                HighShelf { hz: 8000.0, gain_db: -6.0 },
            ],
            (None, 0.19),
        ),
        Row {
            name: "air +6 dB",
            tone: ToneParams { enabled: true, air_db: 6.0, ..ToneParams::default() },
            holder: None,
            record: (None, 0.01),
        },
        Row {
            name: "plosive guard at 12 dB",
            tone: ToneParams { enabled: true, plosive_db: 12.0, ..ToneParams::default() },
            holder: Some((30.0, 0.6)),
            record: (Some(233.0), 0.17),
        },
        Row {
            name: "de-esser at 12 dB",
            tone: ToneParams { enabled: true, de_ess_db: 12.0, ..ToneParams::default() },
            holder: Some((17_000.0, 0.5)),
            record: (Some(2300.0), 0.04),
        },
    ]
}

// The done-when, and every delay decision 0029 computed: through the engine, each band comes out
// within 0.1 dB of the cookbook's curve at every tone and as late as its coefficients say, and
// each row's latest frequency and delay are within 3 % of the record's, or 0.005 ms for the
// smallest. A tone holds the guard or the de-esser at its deepest. `--nocapture` prints the table.
#[test]
fn each_delay_the_record_computed_is_what_the_engine_measures() {
    println!("{:<32} {:>16} {:>16}", "filter, 48 kHz", "computed", "measured");
    for row in rows() {
        let measured = measure(row.tone, row.holder);
        let (hz, ms) = measured.latest();
        let (record_hz, record_ms) = row.record;
        let shown = |hz: Option<f64>, ms: f64| {
            format!("{} {ms:.3} ms", hz.map_or(String::new(), |hz| format!("{hz:.0} Hz")))
        };
        println!(
            "{:<32} {:>16} {:>16}",
            row.name,
            shown(record_hz, record_ms),
            shown(Some(hz), ms)
        );
        assert!(
            (ms - record_ms).abs() <= (0.03 * record_ms).max(0.005),
            "{}: {ms:.4} ms",
            row.name
        );
        if let Some(record_hz) = record_hz {
            assert!((hz / record_hz - 1.0).abs() <= 0.03, "{}: latest at {hz:.0} Hz", row.name);
        }
        if row.holder.is_some() {
            continue;
        }
        let cascade = row.tone.cascade();
        let worst = record_ms.max(0.01);
        for &(at, delay) in &measured.delay_ms {
            let computed: f64 = cascade.iter().map(|section| section.response(at).1).sum::<f64>()
                * 1000.0
                / f64::from(RATE);
            assert!(
                (delay - computed).abs() <= 0.01 * worst + 0.001,
                "{} at {at} Hz: {delay:.4} ms, computed {computed:.4}",
                row.name
            );
        }
        for &(at, gain) in &measured.gain_db {
            let air = (row.tone.air_db != 0.0)
                .then_some(ToneBand::HighShelf { hz: 10_000.0, gain_db: row.tone.air_db });
            let curve: f64 = row
                .tone
                .bands
                .as_slice()
                .iter()
                .chain(air.as_ref())
                .map(|band| cookbook_db(band, at))
                .sum();
            assert!(
                (gain - curve).abs() < 0.1,
                "{} at {at} Hz: {gain:.3} dB, the cookbook's {curve:.3}",
                row.name
            );
        }
    }
}

// The done-when: tones from 80 Hz to 16 kHz through bands and air at the cap come out at most
// 0.2 ms late, and 0.4 with the guard held at its deepest
#[test]
fn tones_through_bands_and_air_at_the_cap_are_within_it() {
    let cap = at_the_cap();
    assert_eq!(cap.check(), Ok(()));
    let (hz, ms) = measure(cap, None).latest();
    println!("bands and air at the cap: {hz:.0} Hz {ms:.3} ms late");
    assert!(ms > MOST_LATE_MS - 0.01 && ms <= MOST_LATE_MS, "{ms:.4} ms at {hz:.0} Hz");
    let ducking = ToneParams { plosive_db: 12.0, ..cap };
    let (hz, ms) = measure(ducking, Some((30.0, 0.6))).latest();
    println!("and the guard at its deepest: {hz:.0} Hz {ms:.3} ms late");
    assert!(ms <= 2.0 * MOST_LATE_MS, "{ms:.4} ms at {hz:.0} Hz");
}
