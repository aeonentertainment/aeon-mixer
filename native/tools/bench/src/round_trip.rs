//! What a loopback run heard, read with `measure-latency`'s own detector and refused where that
//! tool refuses (decision 0006). Both signals count frames of the output's clock, so a chirp's
//! gap is a subtraction: no clock line, and nothing split.

use engine::block::RATE;
use measure_latency::matched_filter::{
    MatchedFilter, ONSET_REACH, first_arrival, signal_to_noise_db,
};
use measure_latency::programme::Programme;
use measure_latency::reading::{MAX_MOVED_MS, MIN_SNR_DB, median};

/// Under this loudest sample the input heard nothing, which on macOS is what a refused microphone
/// sounds like: `measure-latency`'s own bar.
const SILENT_DB: f64 = -70.0;

/// Each chirp's figures, in programme order.
pub struct RoundTrip {
    /// From the chirp leaving the render to its arrival in the capture slot's output, less the
    /// air, in ms.
    pub gaps_ms: Vec<f64>,
    /// The chirp as heard against as played, in dB.
    pub gains_db: Vec<f64>,
    /// The weakest arrival over what the slot played before the first chirp, in dB.
    pub snr_db: f64,
}

impl RoundTrip {
    pub fn median_ms(&self) -> f64 {
        median(&self.gaps_ms)
    }

    /// How far the chirps' gaps disagree, in ms.
    pub fn spread_ms(&self) -> f64 {
        let (low, high) =
            self.gaps_ms.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), ms| {
                (low.min(*ms), high.max(*ms))
            });
        high - low
    }

    pub fn gain_db(&self) -> f64 {
        median(&self.gains_db)
    }
}

/// Reads the run: `played` is what the output was handed and `heard` what the capture slot
/// played, both from the run's first frame. Says why when it holds no number.
pub fn read(
    played: &[f32],
    heard: &[f32],
    programme: &Programme,
    air_ms: f64,
) -> Result<RoundTrip, String> {
    let peak = heard.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let peak_db = 20.0 * f64::from(peak).max(1e-9).log10();
    if peak_db < SILENT_DB {
        return Err(format!(
            "the input is silent, its loudest at {peak_db:.0} dBFS: on macOS let this terminal use \
             the microphone in System Settings, Privacy & Security; elsewhere check --input, \
             --input-channel and the cable"
        ));
    }
    let filter = MatchedFilter::new(programme.chirp.samples());
    let (played, heard) = (filter.envelope(played), filter.envelope(heard));
    let interval = programme.interval();
    // The floor is the second half of the preroll, all of which the slot plays the input through
    let quiet = programme.preroll / 2..programme.preroll;
    let mut round_trip = RoundTrip { gaps_ms: Vec::new(), gains_db: Vec::new(), snr_db: f64::MAX };
    for start in programme.starts() {
        let out = first_arrival(&played, start..start + interval, 0)
            .ok_or("a chirp never reached the output, which is a bug in this tool")?;
        // Causality fixes where to look: not before the chirp left, and in its own interval
        let from = out.frame as usize;
        let back = first_arrival(&heard, from..from + interval, ONSET_REACH).ok_or_else(|| {
            format!(
                "no chirp came back in the {} ms after one was played. Check the rig, the input \
                 channel and the volume",
                interval * 1000 / RATE as usize
            )
        })?;
        let snr = signal_to_noise_db(&back, &heard, quiet.clone());
        if snr < MIN_SNR_DB {
            return Err(format!(
                "a chirp came back {snr:.0} dB over the noise, under the {MIN_SNR_DB:.0} dB a \
                 reading needs. Raise the output, bring the mic closer, or quiet the room"
            ));
        }
        round_trip.snr_db = round_trip.snr_db.min(snr);
        round_trip.gaps_ms.push((back.frame - out.frame) * 1000.0 / f64::from(RATE) - air_ms);
        round_trip.gains_db.push(20.0 * (back.strength / out.strength).log10());
    }
    if round_trip.spread_ms() > MAX_MOVED_MS {
        return Err(format!(
            "the chirps disagree by {:.2} ms, over the {MAX_MOVED_MS} ms a reading allows. The \
             output or the input dropped audio, or the first sound changed between chirps: rerun \
             with nothing else playing or recording",
            round_trip.spread_ms()
        ));
    }
    Ok(round_trip)
}
