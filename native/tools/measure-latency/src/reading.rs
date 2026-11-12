//! What one opening of a real device says: the signal as rendered and as captured, each stream's
//! wakes on the host clock, and from those how long a rendered sample took to get to the input.
//! Pure, so a test can run it on a device of its own making with a known answer.

use engine::block::RATE;

use crate::clock_line::{ClockLine, Wake};
use crate::matched_filter::{MatchedFilter, first_arrival, signal_to_noise_db};
use crate::programme::Programme;

/// The least a first arrival may stand over the input's noise. The onset rule takes the earliest
/// peak within 20 dB of the strongest, so a noisy input could hand it a noise peak; that peak
/// stands only about 10 dB over the noise's own level, and is refused here rather than read.
pub const MIN_SNR_DB: f64 = 30.0;

/// The most the figure may move across one opening's chirps, in ms. An output that drops audio and
/// carries on stands later against its own frame count from then on, by at least the 1.33 ms of a
/// 64-frame period, and the chirps either side of it disagree by nearly that. It is read from the
/// sound and not from the backend's underrun count, which misses what the OS does not report and,
/// in WASAPI exclusive, counts every late wake of a driver that signals in bursts (decision 0003).
pub const MAX_MOVED_MS: f64 = 0.75;

/// Under this peak the input is silent, which on macOS is what a denied microphone sounds like.
const SILENT_DB: f64 = -70.0;

/// What one opening recorded. `render_wakes` hold the first frame of the block rendered at that
/// moment; `capture_wakes` hold how many frames had been delivered at that moment.
pub struct Take {
    pub rendered: Vec<f32>,
    pub render_wakes: Vec<Wake>,
    pub captured: Vec<f32>,
    pub capture_wakes: Vec<Wake>,
}

/// One opening's figures, in milliseconds. Each runs from a sample's mean render time, so bursty
/// callbacks count at their average.
#[derive(Clone, Debug)]
pub struct Reading {
    /// To the sample's arrival at the input's wake line: output, speaker and air or cable, and
    /// what the capture hardware and driver hold before the OS can wake anyone. Each chirp's, in
    /// order.
    pub to_input: Vec<f64>,
    /// As `to_input` but to the wake that delivered the sample: the input's period too. A native
    /// capture slot (#52) can do no better than this.
    pub delivered: f64,
    /// The worst chirp's arrival over the input's noise floor.
    pub snr_db: f64,
    pub peak_db: f64,
    /// How far the render wakes stand from their line, most to least: bursts and scheduling.
    pub render_spread: f64,
}

impl Reading {
    /// The median chirp's figure.
    pub fn median(&self) -> f64 {
        median(&self.to_input)
    }

    /// The most the figure moved between chirps of one opening.
    pub fn moved(&self) -> f64 {
        let (low, high) =
            self.to_input.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), ms| {
                (low.min(*ms), high.max(*ms))
            });
        high - low
    }
}

pub fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// Reads a take, or says why there is no number in it.
pub fn read(take: &Take, programme: &Programme) -> Result<Reading, String> {
    let interval = programme.interval();
    let filter = MatchedFilter::new(programme.chirp.samples());

    let rendered = filter.envelope(&take.rendered);
    let out: Vec<f64> = programme
        .starts()
        .map(|start| first_arrival(&rendered, start..start + interval).map(|arrival| arrival.frame))
        .collect::<Option<_>>()
        .ok_or("a chirp never reached the render, which is a bug in the engine or this tool")?;

    // Only the wakes the chirps span, so what a stream does while it settles or after the last chirp
    // has no say in the figure
    let (Some(first), Some(last)) = (out.first(), out.last()) else {
        return Err("the programme plays no chirps".into());
    };
    let render_wakes: Vec<Wake> = take
        .render_wakes
        .iter()
        .copied()
        .filter(|wake| (*first..=*last).contains(&(wake.frame as f64)))
        .collect();
    let settled = programme.preroll as u64 / 2;
    let capture_wakes: Vec<Wake> =
        take.capture_wakes.iter().copied().filter(|wake| wake.frame >= settled).collect();
    if render_wakes.len() < 2 || capture_wakes.len() < 2 {
        return Err("a stream delivered too little to read".into());
    }
    let render_line = ClockLine::through(&render_wakes);
    let capture_line = ClockLine::under(&capture_wakes);
    let (low, high) = render_line.spread(&render_wakes);

    let peak = take.captured.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let peak_db = 20.0 * f64::from(peak).max(1e-9).log10();
    if peak_db < SILENT_DB {
        return Err(format!(
            "the input is silent, peak {peak_db:.0} dBFS. On macOS, grant this terminal the \
             microphone in System Settings, Privacy & Security"
        ));
    }

    let captured = filter.envelope(&take.captured);
    let mut to_input = Vec::with_capacity(programme.repeats);
    let mut delivered = Vec::with_capacity(programme.repeats);
    let mut snr_db = f64::INFINITY;
    // The floor is what the input hears before any chirp could reach it
    let quiet_until = capture_line.frame_at(render_line.at(programme.preroll as f64)) as usize;
    for out in out {
        let rendered_ns = render_line.at(out);
        // Causality fixes where to look: not before the chirp was rendered, and in its own interval
        let from = capture_line.frame_at(rendered_ns).max(0.0) as usize;
        let arrival = first_arrival(&captured, from..from + interval).ok_or_else(|| {
            format!("no chirp in the {} ms after one was rendered", interval * 1000 / RATE as usize)
        })?;
        let snr = signal_to_noise_db(&arrival, &captured, 0..quiet_until.max(1));
        if snr < MIN_SNR_DB {
            return Err(format!(
                "signal {snr:.0} dB over the noise, under the {MIN_SNR_DB:.0} dB a reading needs. \
                 Raise the output, bring the mic closer, or quiet the room"
            ));
        }
        snr_db = snr_db.min(snr);
        // A frame exists once the one after it begins
        let last_frame = arrival.frame + 1.0;
        to_input.push((capture_line.at(last_frame) - rendered_ns) / 1e6);
        let wake = take
            .capture_wakes
            .iter()
            .find(|wake| wake.frame as f64 >= last_frame)
            .ok_or("the capture ended before its last chirp came back")?;
        delivered.push((wake.at_ns as f64 - rendered_ns) / 1e6);
    }
    let reading = Reading {
        to_input,
        delivered: median(&delivered),
        snr_db,
        peak_db,
        render_spread: (high - low) / 1e6,
    };
    if reading.moved() > MAX_MOVED_MS {
        return Err(format!(
            "the figure moved {:.2} ms across the chirps, over the {MAX_MOVED_MS} ms a reading \
             allows. The output dropped audio, or the first sound changed between chirps",
            reading.moved()
        ));
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use engine::block::BLOCK;
    use engine::driver::OfflineDriver;

    use super::*;
    use crate::ingest::Ingest;
    use crate::rig::Rig;

    /// A device of the test's own making, with its answer known.
    struct Device {
        /// How long from a frame's place on the output's clock to its reaching the input's
        /// hardware, in ms.
        output_ms: f64,
        /// What the input holds it for before its first possible wake, in ms.
        residual_ms: f64,
        /// A callback's frames at the output and the input.
        output_period: usize,
        input_period: usize,
        /// How fast the input's clock runs against the output's: 1.0001 is 100 ppm.
        input_clock: f64,
        /// Paths after the first as (extra ms, gain), and noise RMS.
        echoes: &'static [(f64, f32)],
        noise: f32,
        dropout: Option<Dropout>,
    }

    /// The output loses `ms` at `frame` of its own count and carries on, its frames still counted
    /// as consecutive: what CoreAudio's skipped cycle and an ALSA recovery both look like.
    #[derive(Clone, Copy)]
    struct Dropout {
        frame: usize,
        ms: f64,
        /// Whether the thread's wakes step with it, or stay on schedule while the sound slips.
        wakes_too: bool,
    }

    const NS_PER_FRAME: f64 = 1e9 / RATE as f64;

    /// The real engine renders on the offline driver, and the rest plays out on a timeline: the
    /// output's callbacks, the path through the air, the input's clock and late wakes.
    fn take(device: &Device, programme: &Programme) -> Take {
        let signal = programme.signal();
        let frames = signal.len() + programme.interval();
        let Rig { render, controls: _controls, .. } =
            Rig::new(Ingest::L16 { frame: 240 }, 240, &signal, 0).unwrap();
        let mut driver = OfflineDriver::new(render);
        let mut rendered = Vec::with_capacity(frames);
        let mut block = [0.0f32; 2 * BLOCK];
        let mut render_wakes = Vec::new();
        let mut state = 99u64;
        let mut random = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as f64 / (1u64 << 31) as f64
        };
        let start_ns = 1e9;
        while rendered.len() < frames {
            let frame = rendered.len();
            // Blocks render at the top of the callback that needs them, up to 200 µs late
            let callback = frame / device.output_period * device.output_period;
            let lost = device
                .dropout
                .filter(|dropout| dropout.wakes_too && frame >= dropout.frame)
                .map_or(0.0, |dropout| dropout.ms * 1e6);
            let at_ns = start_ns + callback as f64 * NS_PER_FRAME + lost + random() * 200_000.0;
            render_wakes.push(Wake { frame: frame as u64, at_ns: at_ns as u64 });
            assert_no_alloc::assert_no_alloc(|| driver.pump_into(&mut block));
            rendered.extend(block.iter().step_by(2));
        }

        let input_ns_per_frame = NS_PER_FRAME / device.input_clock;
        // Starts 300 ms before the output, as the tool starts it
        let input_start = start_ns - 3e8;
        let total = ((frames as f64 + 1000.0) * NS_PER_FRAME + 3e8) / input_ns_per_frame;
        let sound_at = |ns: f64, extra_ms: f64| -> f32 {
            let mut since = ns - start_ns - (device.output_ms + extra_ms) * 1e6;
            if let Some(dropout) = device.dropout {
                let at = dropout.frame as f64 * NS_PER_FRAME;
                let gap = dropout.ms * 1e6;
                if (at..at + gap).contains(&since) {
                    return 0.0;
                }
                if since >= at + gap {
                    since -= gap;
                }
            }
            let frame = since / NS_PER_FRAME;
            if frame < 0.0 || frame + 1.0 >= rendered.len() as f64 {
                return 0.0;
            }
            let (whole, part) = (frame.floor() as usize, frame.fract() as f32);
            rendered[whole] * (1.0 - part) + rendered[whole + 1] * part
        };
        let mut captured = Vec::with_capacity(total as usize);
        let mut capture_wakes = Vec::new();
        for frame in 0..total as usize {
            let ns = input_start + frame as f64 * input_ns_per_frame;
            let direct = sound_at(ns, 0.0);
            let echoes: f32 = device.echoes.iter().map(|(ms, gain)| gain * sound_at(ns, *ms)).sum();
            let noise = device.noise * (random() as f32 * 2.0 - 1.0) * 3.0f32.sqrt();
            captured.push(direct + echoes + noise);
            if (frame + 1).is_multiple_of(device.input_period) {
                // The frames reach the OS `residual_ms` later, and the thread wakes up to 1 ms
                // past that, with one wake in fifty right on time
                let on_time = capture_wakes.len().is_multiple_of(50);
                let late = if on_time { 0.0 } else { random() * 1_000_000.0 };
                let at = ns + input_ns_per_frame + device.residual_ms * 1e6 + late;
                capture_wakes.push(Wake { frame: frame as u64 + 1, at_ns: at as u64 });
            }
        }
        Take { rendered, render_wakes, captured, capture_wakes }
    }

    /// What the figure should come to: the device's own delay and the input's residual, plus how
    /// long on average a block sits rendered before its place on the output's clock comes.
    fn truth(device: &Device, take: &Take) -> f64 {
        let before_any_dropout = device.dropout.map_or(u64::MAX, |dropout| dropout.frame as u64);
        let waits: Vec<f64> = take
            .render_wakes
            .iter()
            .filter(|wake| wake.frame < before_any_dropout)
            .map(|wake| {
                let due = 1e9 + wake.frame as f64 * NS_PER_FRAME;
                (due - wake.at_ns as f64) / 1e6
            })
            .collect();
        let mean = waits.iter().sum::<f64>() / waits.len() as f64;
        device.output_ms + device.residual_ms + mean
    }

    fn reading(device: &Device) -> (Reading, f64) {
        let programme = Programme::device(0.25);
        let take = take(device, &programme);
        (read(&take, &programme).unwrap(), truth(device, &take))
    }

    const CLEAN: Device = Device {
        output_ms: 11.2,
        residual_ms: 1.0,
        output_period: 128,
        input_period: 128,
        input_clock: 1.0,
        echoes: &[],
        noise: 0.0,
        dropout: None,
    };

    /// The figure is the output and the input's residual and nothing of the input's period, to
    /// within 100 µs, on a device whose answer is known.
    #[test]
    fn a_known_delay_comes_back_with_the_inputs_period_taken_out() {
        for input_period in [64, 128, 480, 960] {
            let (reading, truth) = reading(&Device { input_period, ..CLEAN });
            let error = reading.median() - truth;
            assert!(error.abs() < 0.1, "a {input_period}-frame input: off by {error} ms");
            assert!(reading.moved() < 0.1, "moved {} ms", reading.moved());
            let period_ms = input_period as f64 * 1000.0 / f64::from(RATE);
            assert!(
                reading.delivered - reading.median() < period_ms + 1.5,
                "delivered {} ms",
                reading.delivered
            );
        }
    }

    /// Windows' 480-frame shared engine, with a USB mic on its own clock 100 ppm fast, across a
    /// room: reflections louder than the direct sound, noise at -40 dBFS. The figure holds to
    /// within 300 µs, and the bursts show in the spread.
    #[test]
    fn a_bursty_output_a_drifting_input_and_a_real_room_still_give_the_figure() {
        let device = Device {
            output_ms: 20.0,
            output_period: 480,
            input_period: 480,
            input_clock: 1.0001,
            echoes: &[(4.0, 1.5), (9.0, 0.7)],
            noise: 0.01,
            ..CLEAN
        };
        let (reading, truth) = reading(&device);
        let error = reading.median() - truth;
        assert!(error.abs() < 0.3, "off by {error} ms");
        assert!(reading.snr_db > MIN_SNR_DB, "SNR {}", reading.snr_db);
        assert!(reading.render_spread > 5.0, "bursts span {} ms", reading.render_spread);
    }

    /// An output that loses a single 64-frame period between two chirps gives no number, whether
    /// or not its thread's wakes show it, and wherever in the run it falls. A figure across that
    /// step would be neither the delay before it nor the delay after.
    #[test]
    fn a_dropout_between_the_chirps_refuses_the_reading() {
        let programme = Programme::device(0.25);
        let ms = 64.0 * 1000.0 / f64::from(RATE);
        let starts: Vec<usize> = programme.starts().collect();
        for after_chirp in [0, 3, 6] {
            for wakes_too in [true, false] {
                let frame = starts[after_chirp] + programme.interval() / 2;
                let device = Device { dropout: Some(Dropout { frame, ms, wakes_too }), ..CLEAN };
                let error = read(&take(&device, &programme), &programme).unwrap_err();
                assert!(
                    error.contains("moved"),
                    "after chirp {after_chirp}, wakes_too {wakes_too}: {error}"
                );
            }
        }
    }

    /// A dropout once the last chirp has come back touches nothing the figure rests on, however
    /// long it is, so the reading stands. Refusing it would throw away a good number.
    #[test]
    fn a_dropout_after_the_last_chirp_leaves_the_figure_alone() {
        let programme = Programme::device(0.25);
        let last = programme.starts().last().unwrap();
        let frame = last + programme.chirp.frames + programme.gap / 2;
        let dropout = Some(Dropout { frame, ms: 30.0, wakes_too: true });
        let device = Device { dropout, ..CLEAN };
        let take = take(&device, &programme);
        let reading = read(&take, &programme).unwrap();
        let error = reading.median() - truth(&device, &take);
        assert!(error.abs() < 0.1, "off by {error} ms");
    }

    /// A signal lost in the noise gives no number rather than a wrong one, and a silent input says
    /// what silence usually means.
    #[test]
    fn a_reading_it_cannot_stand_behind_is_refused() {
        let programme = Programme::device(0.25);
        let noisy = Device { noise: 0.5, ..CLEAN };
        let error = read(&take(&noisy, &programme), &programme).unwrap_err();
        assert!(error.contains("over the noise"), "{error}");

        let mut silent = take(&CLEAN, &programme);
        silent.captured.fill(0.0);
        let error = read(&silent, &programme).unwrap_err();
        assert!(error.contains("silent"), "{error}");
    }
}
