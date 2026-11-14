use std::sync::{Mutex, PoisonError};

use engine::block::RATE;
use engine::driver::DriverError;

use crate::nominal_rate::NominalRate;
use crate::rate_hold::RateHold;

/// The devices this process's streams hold at 48 kHz, each with the rate it was found at and how
/// many streams use it. A device is shared: an output and a house mic can be one USB interface, so
/// the first stream moves the rate and the last one out puts it back, and no stream has the rate
/// changed under it. Decision 0023 has the reasons. Never touched from an audio thread.
pub struct RateHolds<R> {
    hal: R,
    held: Mutex<Vec<Held>>,
}

struct Held {
    device: u32,
    /// The rate before ours, or none when the device was at 48 kHz already.
    found: Option<f64>,
    users: u32,
}

impl<R: NominalRate> RateHolds<R> {
    pub const fn new(hal: R) -> Self {
        Self { hal, held: Mutex::new(Vec::new()) }
    }

    /// Puts `device` at 48 kHz for one stream until the hold drops. Blocks while the rate lands. A
    /// rate that never lands is put back before this fails.
    pub fn hold(&self, device: u32) -> Result<RateHold<'_, R>, DriverError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match held.iter().position(|held| held.device == device) {
            Some(at) => at,
            None => {
                held.push(Held { device, found: None, users: 0 });
                held.len() - 1
            }
        };
        let hold = RateHold { holds: self, device };
        held[at].users += 1;
        let rate = f64::from(RATE);
        let moved = match self.hal.rate(device) {
            Ok(found) if found == rate => Ok(()),
            Ok(found) => {
                held[at].found.get_or_insert(found);
                self.hal.set_rate(device, rate)
            }
            Err(error) => Err(error),
        };
        drop(held);
        // On a failure the hold drops here, and with no other user puts the rate back
        moved.map(|()| hold)
    }

    pub(crate) fn release(&self, device: u32) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(at) = held.iter().position(|held| held.device == device) else { return };
        held[at].users -= 1;
        if held[at].users > 0 {
            return;
        }
        let Held { found, .. } = held.swap_remove(at);
        // Best effort: a device that has gone takes its rate with it
        if let Some(found) = found
            && self.hal.rate(device).is_ok_and(|rate| rate != found)
        {
            let _ = self.hal.set_rate(device, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Devices by id with their rates. A set fails when `fails` is set: before the rate moves,
    /// or with `lands_late`, after, as a rate that lands once the wait for it has given up. Every
    /// set is recorded.
    #[derive(Default)]
    struct Fake {
        rates: Mutex<Vec<(u32, f64)>>,
        fails: bool,
        lands_late: bool,
        sets: Mutex<Vec<(u32, f64)>>,
    }

    impl Fake {
        fn new(rates: &[(u32, f64)]) -> Self {
            Self { rates: Mutex::new(rates.to_vec()), ..Self::default() }
        }

        fn sets(&self) -> Vec<(u32, f64)> {
            self.sets.lock().unwrap().clone()
        }

        fn rate_of(&self, device: u32) -> f64 {
            self.rate(device).unwrap()
        }
    }

    impl NominalRate for Fake {
        fn rate(&self, device: u32) -> Result<f64, DriverError> {
            let rates = self.rates.lock().unwrap();
            rates
                .iter()
                .find(|(id, _)| *id == device)
                .map(|(_, rate)| *rate)
                .ok_or(DriverError::NoDevice)
        }

        fn set_rate(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            let first = self.sets.lock().unwrap().is_empty();
            self.sets.lock().unwrap().push((device, rate));
            if self.fails && first && !self.lands_late {
                return Err(DriverError::Backend("the rate never landed".into()));
            }
            let mut rates = self.rates.lock().unwrap();
            let entry =
                rates.iter_mut().find(|(id, _)| *id == device).ok_or(DriverError::NoDevice)?;
            entry.1 = rate;
            match self.fails && first {
                true => Err(DriverError::Backend("the rate landed too late".into())),
                false => Ok(()),
            }
        }
    }

    const MONITOR: u32 = 7;
    const SPEAKERS: u32 = 9;

    #[test]
    fn a_device_already_at_48_khz_is_never_set() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 48_000.0)]));
        drop(holds.hold(MONITOR).unwrap());
        assert_eq!(holds.hal.sets(), []);
    }

    /// A close, a move away and a stream that fails after its rate moved all drop the hold.
    #[test]
    fn the_rate_found_is_put_back_when_the_stream_lets_go() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let hold = holds.hold(MONITOR).unwrap();
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0);
        drop(hold);
        assert_eq!(holds.hal.sets(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
    }

    #[test]
    fn a_rate_that_never_lands_fails_the_hold_and_holds_nothing() {
        let fake = Fake { fails: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        assert!(holds.hold(MONITOR).is_err());
        assert_eq!(
            holds.hal.sets(),
            [(MONITOR, 48_000.0)],
            "still at 44.1 kHz: nothing to put back"
        );
        assert!(holds.held.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rate_that_lands_after_the_hold_failed_is_put_back() {
        let fake = Fake { fails: true, lands_late: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        assert!(holds.hold(MONITOR).is_err());
        assert_eq!(holds.hal.sets(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
    }

    #[test]
    fn a_device_two_streams_use_keeps_48_khz_until_both_let_go() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let output = holds.hold(MONITOR).unwrap();
        let input = holds.hold(MONITOR).unwrap();
        drop(output);
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0, "the input still runs at 48 kHz");
        drop(input);
        assert_eq!(holds.hal.sets(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
    }

    #[test]
    fn each_device_is_put_back_to_its_own_rate() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 44_100.0), (SPEAKERS, 96_000.0)]));
        let monitor = holds.hold(MONITOR).unwrap();
        let speakers = holds.hold(SPEAKERS).unwrap();
        drop(monitor);
        drop(speakers);
        assert_eq!((holds.hal.rate_of(MONITOR), holds.hal.rate_of(SPEAKERS)), (44_100.0, 96_000.0));
    }
}
