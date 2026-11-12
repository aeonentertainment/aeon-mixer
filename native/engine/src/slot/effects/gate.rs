use crate::block::RATE;
use crate::params::CleanupParams;
use crate::slot::effects::level::{amplitude, fade_to_unity, one_pole, power_db};

/// How far above the room the voice must rise to open the gate, and how far it may fall before
/// the gate starts to close. The gap between them is the hysteresis that stops chatter.
const OPEN_DB: f32 = 7.0;
const CLOSE_DB: f32 = 4.0;
/// How long the gate stays open after the voice falls under `CLOSE_DB`, so a soft tail or a
/// consonant between two vowels is not cut.
const HOLD_MS: f32 = 150.0;
/// The most the gate takes off: a downward expander's range, not a mute, so a breath or a tail
/// it misjudges is quieter rather than gone.
const RANGE_DB: f32 = -18.0;
/// Opening is fast because there is no lookahead to hide it in; closing is a slow fade.
const OPEN_MS: f32 = 0.5;
const CLOSE_MS: f32 = 100.0;
/// The level the decision reads: quick to rise so an onset opens at once, slower to fall. On
/// steady room noise it rides 1.7 dB over its own median at the 99.9th percentile, which is what
/// `CLOSE_DB` clears.
const LEVEL_ATTACK_MS: f32 = 1.0;
const LEVEL_RELEASE_MS: f32 = 30.0;
/// The floor tracker reads the same level smoothed further, so the room and the voice are
/// measured the same way and a noise's crest does not read as singing.
const ROOM_MS: f32 = 50.0;
/// The floor falls at once to anything quieter and creeps up otherwise: quickly while the gate is
/// closed, so a room 10 dB louder than the last is learned in under 2 s, and slowly while it is
/// open, so a long soft note does not become the floor and close on itself. Ten seconds of
/// singing raise it 2.5 dB. A voice that swells by more than 6 dB a second opens the gate before
/// the floor can follow it.
const RISE_CLOSED_DB_PER_S: f32 = 6.0;
const RISE_OPEN_DB_PER_S: f32 = 0.25;
/// Where the floor starts for a new singer, and the span it is held in. Below -70 dBFS nothing is
/// worth gating, which keeps a codec's silence from dragging the floor under the room; above -30
/// the floor would be singing.
const FLOOR_START_DB: f32 = -50.0;
const FLOOR_LOWEST_DB: f32 = -70.0;
const FLOOR_HIGHEST_DB: f32 = -30.0;

/// The slot's gate: it turns the room down between lines and leaves the singing alone. It follows
/// the room's own level, so it needs no threshold per phone or per room. It reads only samples it
/// has already passed, so it adds none: an onset opens it within a millisecond, and that first
/// millisecond is what having no lookahead costs. Until it has heard the room for a second or two
/// it may stay open, never closed, on a room louder than where its floor starts.
pub struct Gate {
    level: f32,
    room: f32,
    floor_db: f32,
    open: bool,
    hold_left: u32,
    gain_db: f32,
    /// The linear gain the last sample got, which turning the gate off fades from.
    applied: f32,
    coefficients: Coefficients,
}

struct Coefficients {
    level_attack: f32,
    level_release: f32,
    room: f32,
    open: f32,
    close: f32,
    rise_closed: f32,
    rise_open: f32,
    hold: u32,
}

impl Gate {
    pub fn new() -> Self {
        let rate = RATE as f32;
        Self {
            level: 0.0,
            room: 10f32.powf(FLOOR_START_DB / 10.0),
            floor_db: FLOOR_START_DB,
            open: false,
            hold_left: 0,
            gain_db: 0.0,
            applied: 1.0,
            coefficients: Coefficients {
                level_attack: one_pole(LEVEL_ATTACK_MS),
                level_release: one_pole(LEVEL_RELEASE_MS),
                room: one_pole(ROOM_MS),
                open: one_pole(OPEN_MS),
                close: one_pole(CLOSE_MS),
                rise_closed: RISE_CLOSED_DB_PER_S / rate,
                rise_open: RISE_OPEN_DB_PER_S / rate,
                hold: (HOLD_MS * rate / 1000.0) as u32,
            },
        }
    }

    /// One run of the slot's voice, in place. Turned off, it fades to unity across one block and
    /// then costs nothing; turned on again, it learns the room afresh.
    pub fn process(&mut self, params: &CleanupParams, block: &mut [f32]) {
        if !params.enabled {
            if self.applied != 1.0 {
                fade_to_unity(self.applied, block);
                *self = Self::new();
            }
            return;
        }
        let c = &self.coefficients;
        for sample in block {
            let power = *sample * *sample;
            let toward = if power > self.level { c.level_attack } else { c.level_release };
            self.level += toward * (power - self.level);
            self.room += c.room * (self.level - self.room);

            let room_db = power_db(self.room);
            self.floor_db = if room_db < self.floor_db {
                room_db
            } else {
                self.floor_db + if self.open { c.rise_open } else { c.rise_closed }
            }
            .clamp(FLOOR_LOWEST_DB, FLOOR_HIGHEST_DB);

            let level_db = power_db(self.level);
            if level_db > self.floor_db + OPEN_DB {
                self.open = true;
            }
            if self.open && level_db >= self.floor_db + CLOSE_DB {
                self.hold_left = c.hold;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }

            let target = if self.open { 0.0 } else { RANGE_DB };
            let toward = if target > self.gain_db { c.open } else { c.close };
            self.gain_db += toward * (target - self.gain_db);
            self.applied = amplitude(self.gain_db);
            *sample *= self.applied;
        }
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::BLOCK;

    const ON: CleanupParams = CleanupParams { enabled: true };
    const MS: usize = RATE as usize / 1000;

    /// A room: one-pole low-passed noise, falling with frequency, at `db` dBFS RMS.
    fn room(frames: usize, db: f32) -> Vec<f32> {
        let mut seed = 0x5EED_u32;
        let mut low = 0.0f32;
        let noise: Vec<f32> = (0..frames)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                low += 0.2 * (white - low);
                low
            })
            .collect();
        let rms = (noise.iter().map(|s| s * s).sum::<f32>() / frames as f32).sqrt();
        noise.iter().map(|s| s * amplitude(db) / rms).collect()
    }

    /// Twelve harmonics of 220 Hz, a sung vowel, at `db` dBFS RMS from `start` to `end` ms.
    fn sing(voice: &mut [f32], start: usize, end: usize, db: f32) {
        let norm: f64 = (1..=12).map(|h| 0.5 / f64::from(h * h)).sum::<f64>().sqrt();
        let scale = f64::from(amplitude(db)) / norm;
        for (i, sample) in voice[start * MS..end * MS].iter_mut().enumerate() {
            let t = i as f64 / f64::from(RATE);
            let vowel: f64 =
                (1..=12).map(|h| (TAU * 220.0 * f64::from(h) * t).sin() / f64::from(h)).sum();
            *sample += (scale * vowel) as f32;
        }
    }

    fn rms_db(samples: &[f32]) -> f32 {
        power_db(samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32)
    }

    /// The gain the gate applied over each ms span: it only scales, so output over input is it.
    fn gain_db(input: &[f32], output: &[f32], from_ms: usize, to_ms: usize) -> f32 {
        let span = from_ms * MS..to_ms * MS;
        rms_db(&output[span.clone()]) - rms_db(&input[span])
    }

    // The done-when: on a room 20 dB under the singing, the gate takes at least 15 dB off the room
    // between lines and leaves every note within 1 dB from 7 ms after it starts to where it ends.
    // One note is 10 dB over the room and one lasts 10 s, the two ways a gate chops a soft note.
    // Those 7 ms are the cost of no lookahead: a soft note that starts as a step opens the gate
    // 4 ms in, and its first 4 ms come out up to 18 dB down.
    #[test]
    fn the_room_goes_between_lines_and_soft_notes_stay_whole() {
        // A stream hears the room before its first line
        let mut input = room(17_500 * MS, -50.0);
        let notes = [(2_000, 3_000, -30.0), (4_000, 4_400, -40.0), (5_500, 15_500, -40.0)];
        for (start, end, db) in notes {
            sing(&mut input, start, end, db);
        }
        let mut output = input.clone();
        let mut gate = Gate::new();
        assert_no_alloc(|| output.chunks_mut(BLOCK).for_each(|block| gate.process(&ON, block)));

        for (start, end, _) in notes {
            let worst = (start + 7..end)
                .step_by(5)
                .map(|ms| gain_db(&input, &output, ms, ms + 5))
                .fold(0.0f32, |worst, gain| if gain.abs() > worst.abs() { gain } else { worst });
            assert!(worst.abs() < 1.0, "note at {start} ms: gain reached {worst:.2} dB");
        }
        // Each gap from past the hold and the fade to the next note
        for (from, to) in [(1_500, 2_000), (3_500, 4_000), (4_900, 5_500), (16_000, 17_500)] {
            let gain = gain_db(&input, &output, from, to);
            assert!(gain < -15.0, "gap at {from} ms: gain {gain:.1} dB");
        }
    }

    #[test]
    fn off_is_the_voice_itself() {
        let input = room(500 * MS, -50.0);
        let mut output = input.clone();
        let mut gate = Gate::new();
        let off = CleanupParams { enabled: false };
        assert_no_alloc(|| output.chunks_mut(BLOCK).for_each(|block| gate.process(&off, block)));
        assert_eq!(output, input);
    }
}
