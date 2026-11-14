use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

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
    /// How long a rate put back may take to land: the second a set is given to.
    landing: Duration,
}

struct Held {
    device: u32,
    /// The rate before ours, or none when the device was at 48 kHz already.
    found: Option<f64>,
    users: u32,
    /// When the last stream out asked for `found` again, which may not have landed yet.
    put_back: Option<Instant>,
}

impl<R: NominalRate> RateHolds<R> {
    pub const fn new(hal: R) -> Self {
        Self { hal, held: Mutex::new(Vec::new()), landing: Duration::from_secs(1) }
    }

    /// Puts `device` at 48 kHz for one stream until the hold drops. Blocks while the rate lands,
    /// on the thread that starts the stream, with no lock held: a release on Node's event loop
    /// never waits behind it. The first stream on a device takes the rate it reads as the one to
    /// put back, so a rate a person chose between songs is the one kept.
    pub fn hold(&self, device: u32) -> Result<RateHold<'_, R>, DriverError> {
        let rate = f64::from(RATE);
        let asked = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            let at = match held.iter().position(|held| held.device == device) {
                Some(at) => at,
                None => {
                    held.push(Held { device, found: None, users: 0, put_back: None });
                    held.len() - 1
                }
            };
            let entry = &mut held[at];
            entry.users += 1;
            let first = entry.users == 1;
            let put_back = entry.put_back.take();
            let landing = put_back.is_some_and(|at| at.elapsed() < self.landing);
            // Asked for under the lock, so a put back and the next ask reach the HAL in order
            match self.hal.rate(device) {
                // A put back may still be on its way while the device reads 48 kHz, however long
                // ago it was asked, so this asks again, to land last. Within a second the read is
                // taken as ours still standing and the rate found is kept; later it is the rate
                // the device was left at
                Ok(now) if now == rate => {
                    if first && !landing {
                        entry.found = None;
                    }
                    match put_back {
                        Some(_) => self.hal.put_rate(device, rate).map(|()| true),
                        None => Ok(false),
                    }
                }
                Ok(now) => {
                    if first {
                        entry.found = Some(now);
                    } else {
                        entry.found.get_or_insert(now);
                    }
                    self.hal.put_rate(device, rate).map(|()| true)
                }
                Err(error) => Err(error),
            }
        };
        // On a failure the hold drops here, and with no other user puts the rate back
        let hold = RateHold { holds: self, device };
        match asked {
            Ok(true) => self.hal.wait_for(device, rate).map(|()| hold),
            Ok(false) => Ok(hold),
            Err(error) => Err(error),
        }
    }

    /// The last stream out asks for the found rate and returns without waiting for it, since a
    /// close drops its stream on Node's event loop. It asks whatever the device reads, so a 48 kHz
    /// that lands after its wait gave up is undone too.
    pub(crate) fn release(&self, device: u32) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(held) = held.iter_mut().find(|held| held.device == device) else { return };
        held.users -= 1;
        if held.users == 0
            && let Some(found) = held.found
        {
            // Best effort: a device that has gone takes its rate with it
            let _ = self.hal.put_rate(device, found);
            held.put_back = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// Devices by id with their rates, and every rate asked for in order. The first ask lands at
    /// once, never with `First::Never`, or only once its wait has given up with `First::Late`.
    /// With `puts_wait`, every later ask is still landing.
    #[derive(Default)]
    struct Fake {
        rates: Mutex<Vec<(u32, f64)>>,
        first: First,
        puts_wait: bool,
        asked: Mutex<Vec<(u32, f64)>>,
        pending: Mutex<Option<(u32, f64)>>,
        /// A wait for this device's rate says so, then blocks until the gate's sender drops.
        slow: Option<Slow>,
    }

    struct Slow {
        device: u32,
        waiting: Sender<()>,
        gate: Mutex<Receiver<()>>,
    }

    #[derive(Default, Clone, Copy, PartialEq)]
    enum First {
        #[default]
        Lands,
        Never,
        Late,
    }

    impl Fake {
        fn new(rates: &[(u32, f64)]) -> Self {
            Self { rates: Mutex::new(rates.to_vec()), ..Self::default() }
        }

        fn asked(&self) -> Vec<(u32, f64)> {
            self.asked.lock().unwrap().clone()
        }

        fn rate_of(&self, device: u32) -> f64 {
            self.rate(device).unwrap()
        }

        fn land(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            let mut rates = self.rates.lock().unwrap();
            let entry =
                rates.iter_mut().find(|(id, _)| *id == device).ok_or(DriverError::NoDevice)?;
            entry.1 = rate;
            Ok(())
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

        fn put_rate(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            let mut asked = self.asked.lock().unwrap();
            asked.push((device, rate));
            let first = asked.len() == 1;
            drop(asked);
            match (first, self.first, self.puts_wait) {
                (true, First::Lands, _) | (false, _, false) => self.land(device, rate),
                (true, ..) => {
                    *self.pending.lock().unwrap() = Some((device, rate));
                    Ok(())
                }
                (false, _, true) => Ok(()),
            }
        }

        fn wait_for(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            if let Some(slow) = &self.slow
                && slow.device == device
            {
                let _ = slow.waiting.send(());
                let _ = slow.gate.lock().unwrap().recv();
            }
            if self.rate(device)? == rate {
                return Ok(());
            }
            if self.first == First::Late
                && let Some((device, rate)) = self.pending.lock().unwrap().take()
            {
                self.land(device, rate)?;
            }
            Err(DriverError::Backend("the rate did not land in time".into()))
        }
    }

    const MONITOR: u32 = 7;
    const SPEAKERS: u32 = 9;

    #[test]
    fn a_device_already_at_48_khz_is_never_set() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 48_000.0)]));
        drop(holds.hold(MONITOR).unwrap());
        assert_eq!(holds.hal.asked(), []);
    }

    /// A close, a move away and a stream that fails after its rate moved all drop the hold.
    #[test]
    fn the_rate_found_is_put_back_when_the_stream_lets_go() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let hold = holds.hold(MONITOR).unwrap();
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0);
        drop(hold);
        assert_eq!(holds.hal.asked(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    #[test]
    fn a_rate_that_never_lands_fails_the_hold_and_the_found_rate_is_asked_for_again() {
        let fake = Fake { first: First::Never, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        assert!(holds.hold(MONITOR).is_err());
        assert_eq!(holds.hal.asked(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    #[test]
    fn a_rate_that_lands_after_the_hold_failed_is_put_back() {
        let fake = Fake { first: First::Late, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        assert!(holds.hold(MONITOR).is_err());
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    /// A reopen after a stall drops the old stream and holds the device again at once, before the
    /// put back has landed and while it still reads 48 kHz.
    #[test]
    fn a_device_held_again_before_its_put_back_lands_is_asked_for_48_khz() {
        let fake = Fake { puts_wait: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        drop(holds.hold(MONITOR).unwrap());
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0, "the put back has not landed");
        let again = holds.hold(MONITOR).unwrap();
        let asked = [(MONITOR, 48_000.0), (MONITOR, 44_100.0), (MONITOR, 48_000.0)];
        assert_eq!(holds.hal.asked(), asked, "48 kHz asked after the put back, so it lands last");
        drop(again);
        assert_eq!(holds.hal.asked().last(), Some(&(MONITOR, 44_100.0)), "and the rate found kept");
    }

    /// Between songs a person sets the interface to 96 kHz, or to 48 kHz, which the engine would
    /// never have touched. The next song's stream puts back what they chose.
    #[test]
    fn a_rate_chosen_between_songs_is_the_one_put_back() {
        let holds = RateHolds {
            landing: Duration::ZERO,
            ..RateHolds::new(Fake::new(&[(MONITOR, 44_100.0)]))
        };
        drop(holds.hold(MONITOR).unwrap());
        holds.hal.land(MONITOR, 96_000.0).unwrap();
        drop(holds.hold(MONITOR).unwrap());
        assert_eq!(holds.hal.rate_of(MONITOR), 96_000.0);

        holds.hal.land(MONITOR, 48_000.0).unwrap();
        drop(holds.hold(MONITOR).unwrap());
        assert_eq!(holds.hal.asked().last(), Some(&(MONITOR, 48_000.0)), "nothing to put back");
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0);
    }

    /// A HAL slow to move the rate can be slow to put it back: the put back is still on its way
    /// when the next stream holds the device, long after, and that stream's ask must land last.
    #[test]
    fn a_put_back_still_on_its_way_long_after_is_asked_past() {
        let fake = Fake { puts_wait: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = RateHolds { landing: Duration::ZERO, ..RateHolds::new(fake) };
        drop(holds.hold(MONITOR).unwrap());
        drop(holds.hold(MONITOR).unwrap());
        let asked = &holds.hal.asked()[..3];
        assert_eq!(asked, [(MONITOR, 48_000.0), (MONITOR, 44_100.0), (MONITOR, 48_000.0)]);
    }

    #[test]
    fn a_hold_that_cannot_read_the_device_holds_nothing() {
        let holds = RateHolds::new(Fake::new(&[]));
        assert!(holds.hold(MONITOR).is_err());
        assert!(holds.held.lock().unwrap().iter().all(|held| held.users == 0));
    }

    /// A close drops a house mic's hold on Node's event loop while the follower thread waits for
    /// the room output's rate to land. The release returns while that wait still runs.
    #[test]
    fn a_release_never_waits_behind_another_devices_rate_landing() {
        let (open, gate) = channel();
        let (waiting, waits) = channel();
        let slow = Slow { device: SPEAKERS, waiting, gate: Mutex::new(gate) };
        let fake =
            Fake { slow: Some(slow), ..Fake::new(&[(MONITOR, 44_100.0), (SPEAKERS, 44_100.0)]) };
        let holds = RateHolds::new(fake);
        let mic = holds.hold(MONITOR).unwrap();
        thread::scope(|scope| {
            let output = scope.spawn(|| holds.hold(SPEAKERS).map(drop));
            waits.recv().unwrap();
            let (released, release) = channel();
            scope.spawn(move || {
                drop(mic);
                let _ = released.send(());
            });
            let done = release.recv_timeout(Duration::from_secs(5));
            drop(open);
            assert!(done.is_ok(), "the release waited behind the output's rate");
            assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
            output.join().unwrap().unwrap();
        });
    }

    #[test]
    fn a_device_two_streams_use_keeps_48_khz_until_both_let_go() {
        let holds = RateHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let output = holds.hold(MONITOR).unwrap();
        let input = holds.hold(MONITOR).unwrap();
        drop(output);
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0, "the input still runs at 48 kHz");
        drop(input);
        assert_eq!(holds.hal.asked(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
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
