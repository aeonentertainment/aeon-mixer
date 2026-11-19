use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use engine::block::RATE;
use engine::driver::DriverError;

use crate::device_hold::DeviceHold;
use crate::device_properties::DeviceProperties;

/// The devices this process's streams hold at 48 kHz and at their period, each with the rate and
/// the buffer size it was found at and how many streams use it. A device is shared: an output and
/// a house mic can be one USB interface, so the first stream moves each property and the last one
/// out puts it back, and no stream has either changed under it. Decision 0023 has the reasons.
/// Never touched from an audio thread.
pub struct DeviceHolds<P> {
    hal: P,
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
    /// The IO buffer while a stream holds the device. A set lands before it returns, so unlike
    /// the rate it has nothing to wait for and nothing to outlive the last stream.
    buffer: Option<Buffer>,
}

struct Buffer {
    found: u32,
    ours: u32,
}

impl<P: DeviceProperties> DeviceHolds<P> {
    pub const fn new(hal: P) -> Self {
        Self { hal, held: Mutex::new(Vec::new()), landing: Duration::from_secs(1) }
    }

    /// Puts `device` at 48 kHz and its IO buffer near `frames` for one stream until the hold
    /// drops. Blocks while the rate lands, on the thread that starts the stream, with no lock held:
    /// a release on Node's event loop never waits behind it. The first stream on a device takes
    /// the rate and the size it reads as the ones to put back, so what a person chose between
    /// songs is kept. `rate_landed` runs once the device is at 48 kHz and before its buffer is set,
    /// with no lock held, so a start that fails in between can say the rate landed.
    pub fn hold(
        &self,
        device: u32,
        frames: u32,
        rate_landed: impl FnOnce(),
    ) -> Result<DeviceHold<'_, P>, DriverError> {
        let rate = f64::from(RATE);
        let (at, asked) = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            let at = match held.iter().position(|held| held.device == device) {
                Some(at) => at,
                None => {
                    held.push(Held { device, found: None, users: 0, put_back: None, buffer: None });
                    held.len() - 1
                }
            };
            let entry = &mut held[at];
            entry.users += 1;
            let first = entry.users == 1;
            let put_back = entry.put_back.take();
            let landing = put_back.is_some_and(|at| at.elapsed() < self.landing);
            // Asked for under the lock, so a put back and the next ask reach the HAL in order
            let asked = match self.hal.rate(device) {
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
            };
            (at, asked)
        };
        // On a failure the hold drops here, and with no other user puts back what moved
        let mut hold = DeviceHold { holds: self, device, frames };
        match asked {
            Ok(true) => self.hal.wait_for(device, rate)?,
            Ok(false) => {}
            Err(error) => return Err(error),
        }
        rate_landed();
        hold.frames = self.set_buffer(at, device, frames)?;
        Ok(hold)
    }

    // The buffer is set once the rate has landed, as a device's size range can follow its rate.
    // `at` is the device's entry, which stays where it is: an entry is never removed.
    fn set_buffer(&self, at: usize, device: u32, frames: u32) -> Result<u32, DriverError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = &mut held[at];
        let found = match &entry.buffer {
            Some(buffer) => buffer.found,
            None => self.hal.frames(device)?,
        };
        self.hal.put_frames(device, frames)?;
        let ours = self.hal.frames(device)?;
        entry.buffer = Some(Buffer { found, ours });
        Ok(ours)
    }

    /// The last stream out puts back the buffer size, then asks for the found rate and returns
    /// without waiting for it, since a close drops its stream on Node's event loop. It asks
    /// whatever rate the device reads, so a 48 kHz that lands after its wait gave up is undone
    /// too. The size goes back only while the device still has ours: one set since is theirs.
    pub(crate) fn release(&self, device: u32) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(held) = held.iter_mut().find(|held| held.device == device) else { return };
        held.users -= 1;
        if held.users > 0 {
            return;
        }
        // Best effort, both: a device that has gone takes its settings with it
        if let Some(Buffer { found, ours }) = held.buffer.take()
            && found != ours
            && self.hal.frames(device).is_ok_and(|now| now == ours)
        {
            let _ = self.hal.put_frames(device, found);
        }
        if let Some(found) = held.found {
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
    use crate::steps::Steps;

    /// Devices by id with their rates and buffer sizes, and every property asked for in order. The
    /// first rate asked lands at once, never with `First::Never`, or only once its wait has given
    /// up with `First::Late`. With `puts_wait`, every later rate is still landing. A device in
    /// `refuses` fails every buffer size.
    #[derive(Default)]
    struct Fake {
        devices: Mutex<Vec<(u32, f64, u32)>>,
        first: First,
        puts_wait: bool,
        refuses: Vec<u32>,
        asked: Mutex<Vec<(u32, Ask)>>,
        pending: Mutex<Option<(u32, f64)>>,
        /// A wait for this device's rate says so, then blocks until the gate's sender drops.
        slow: Option<Slow>,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Ask {
        Rate(f64),
        Frames(u32),
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
        /// Each device at `rate`, with a 512-frame buffer.
        fn new(rates: &[(u32, f64)]) -> Self {
            let devices = rates.iter().map(|&(id, rate)| (id, rate, 512)).collect();
            Self { devices: Mutex::new(devices), ..Self::default() }
        }

        fn asked(&self) -> Vec<(u32, Ask)> {
            self.asked.lock().unwrap().clone()
        }

        fn rates_asked(&self) -> Vec<(u32, f64)> {
            let asked = self.asked();
            asked.into_iter().filter_map(|(id, ask)| Some((id, ask.rate()?))).collect()
        }

        fn rate_of(&self, device: u32) -> f64 {
            self.rate(device).unwrap()
        }

        fn size_of(&self, device: u32) -> u32 {
            self.frames(device).unwrap()
        }

        fn land(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            self.with(device, |entry| entry.1 = rate)
        }

        /// Another app, or a person in Audio MIDI Setup, sets the size.
        fn set_elsewhere(&self, device: u32, frames: u32) {
            self.with(device, |entry| entry.2 = frames).unwrap();
        }

        fn with(
            &self,
            device: u32,
            change: impl FnOnce(&mut (u32, f64, u32)),
        ) -> Result<(), DriverError> {
            let mut devices = self.devices.lock().unwrap();
            let entry = devices.iter_mut().find(|(id, ..)| *id == device);
            entry.map(change).ok_or(DriverError::NoDevice)
        }

        fn read<T>(
            &self,
            device: u32,
            field: impl FnOnce(&(u32, f64, u32)) -> T,
        ) -> Result<T, DriverError> {
            let devices = self.devices.lock().unwrap();
            devices.iter().find(|(id, ..)| *id == device).map(field).ok_or(DriverError::NoDevice)
        }
    }

    impl Ask {
        fn rate(self) -> Option<f64> {
            match self {
                Ask::Rate(rate) => Some(rate),
                Ask::Frames(_) => None,
            }
        }
    }

    impl DeviceProperties for Fake {
        fn rate(&self, device: u32) -> Result<f64, DriverError> {
            self.read(device, |entry| entry.1)
        }

        fn put_rate(&self, device: u32, rate: f64) -> Result<(), DriverError> {
            let mut asked = self.asked.lock().unwrap();
            let first = asked.iter().all(|(_, ask)| ask.rate().is_none());
            asked.push((device, Ask::Rate(rate)));
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

        fn frames(&self, device: u32) -> Result<u32, DriverError> {
            self.read(device, |entry| entry.2)
        }

        fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError> {
            self.asked.lock().unwrap().push((device, Ask::Frames(frames)));
            if self.refuses.contains(&device) {
                return Err(DriverError::Backend("the device refused the size".into()));
            }
            self.with(device, |entry| entry.2 = frames)
        }
    }

    const MONITOR: u32 = 7;
    const SPEAKERS: u32 = 9;
    const PERIOD: u32 = 128;

    #[test]
    fn a_device_already_at_48_khz_is_never_set() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 48_000.0)]));
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        assert_eq!(holds.hal.rates_asked(), []);
    }

    /// A close, a move away and a stream that fails after its rate moved all drop the hold.
    #[test]
    fn the_rate_found_is_put_back_when_the_stream_lets_go() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let hold = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0);
        drop(hold);
        assert_eq!(holds.hal.rates_asked(), [(MONITOR, 48_000.0), (MONITOR, 44_100.0)]);
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    /// The buffer is the period on the rate's device, so it goes back first, and the rate the
    /// stream ran at is still there while it does.
    #[test]
    fn the_buffer_is_put_back_before_the_rate() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        let asked = holds.hal.asked().into_iter().map(|(_, ask)| ask).collect::<Vec<_>>();
        let order =
            [Ask::Rate(48_000.0), Ask::Frames(PERIOD), Ask::Frames(512), Ask::Rate(44_100.0)];
        assert_eq!(asked, order);
    }

    #[test]
    fn a_rate_that_never_lands_fails_the_hold_and_the_found_rate_is_asked_for_again() {
        let fake = Fake { first: First::Never, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        assert!(holds.hold(MONITOR, PERIOD, || {}).is_err());
        assert_eq!(
            holds.hal.asked(),
            [(MONITOR, Ask::Rate(48_000.0)), (MONITOR, Ask::Rate(44_100.0))]
        );
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    #[test]
    fn a_rate_that_lands_after_the_hold_failed_is_put_back() {
        let fake = Fake { first: First::Late, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        assert!(holds.hold(MONITOR, PERIOD, || {}).is_err());
        assert_eq!(holds.hal.rate_of(MONITOR), 44_100.0);
    }

    /// A reopen after a stall drops the old stream and holds the device again at once, before the
    /// put back has landed and while it still reads 48 kHz.
    #[test]
    fn a_device_held_again_before_its_put_back_lands_is_asked_for_48_khz() {
        let fake = Fake { puts_wait: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0, "the put back has not landed");
        let again = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        let asked = [(MONITOR, 48_000.0), (MONITOR, 44_100.0), (MONITOR, 48_000.0)];
        assert_eq!(
            holds.hal.rates_asked(),
            asked,
            "48 kHz asked after the put back, so it lands last"
        );
        drop(again);
        assert_eq!(
            holds.hal.rates_asked().last(),
            Some(&(MONITOR, 44_100.0)),
            "and the rate found kept"
        );
    }

    /// Between songs a person sets the interface to 96 kHz, or to 48 kHz, which the engine would
    /// never have touched. The next song's stream puts back what they chose.
    #[test]
    fn a_rate_chosen_between_songs_is_the_one_put_back() {
        let holds = DeviceHolds {
            landing: Duration::ZERO,
            ..DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0)]))
        };
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        holds.hal.land(MONITOR, 96_000.0).unwrap();
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        assert_eq!(holds.hal.rate_of(MONITOR), 96_000.0);

        holds.hal.land(MONITOR, 48_000.0).unwrap();
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        assert_eq!(
            holds.hal.rates_asked().last(),
            Some(&(MONITOR, 48_000.0)),
            "nothing to put back"
        );
        assert_eq!(holds.hal.rate_of(MONITOR), 48_000.0);
    }

    /// A HAL slow to move the rate can be slow to put it back: the put back is still on its way
    /// when the next stream holds the device, long after, and that stream's ask must land last.
    #[test]
    fn a_put_back_still_on_its_way_long_after_is_asked_past() {
        let fake = Fake { puts_wait: true, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds { landing: Duration::ZERO, ..DeviceHolds::new(fake) };
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        let asked = &holds.hal.rates_asked()[..3];
        assert_eq!(asked, [(MONITOR, 48_000.0), (MONITOR, 44_100.0), (MONITOR, 48_000.0)]);
    }

    #[test]
    fn a_hold_that_cannot_read_the_device_holds_nothing() {
        let holds = DeviceHolds::new(Fake::new(&[]));
        assert!(holds.hold(MONITOR, PERIOD, || {}).is_err());
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
        let holds = DeviceHolds::new(fake);
        let mic = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        thread::scope(|scope| {
            let output = scope.spawn(|| holds.hold(SPEAKERS, PERIOD, || {}).map(drop));
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
    fn a_device_two_streams_use_keeps_48_khz_and_the_period_until_both_let_go() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let output = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        let input = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        drop(output);
        let now = (holds.hal.rate_of(MONITOR), holds.hal.size_of(MONITOR));
        assert_eq!(now, (48_000.0, PERIOD), "the input still runs on both");
        drop(input);
        assert_eq!((holds.hal.rate_of(MONITOR), holds.hal.size_of(MONITOR)), (44_100.0, 512));
    }

    #[test]
    fn each_device_is_put_back_to_its_own_rate() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0), (SPEAKERS, 96_000.0)]));
        let monitor = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        let speakers = holds.hold(SPEAKERS, PERIOD, || {}).unwrap();
        drop(monitor);
        drop(speakers);
        assert_eq!((holds.hal.rate_of(MONITOR), holds.hal.rate_of(SPEAKERS)), (44_100.0, 96_000.0));
    }

    #[test]
    fn the_size_found_is_put_back_when_the_stream_closes() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 48_000.0)]));
        let hold = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        assert_eq!((hold.frames, holds.hal.size_of(MONITOR)), (PERIOD, PERIOD));
        drop(hold);
        assert_eq!(holds.hal.size_of(MONITOR), 512);
    }

    /// A move opens the new output's stream and drops the old one's.
    #[test]
    fn a_move_puts_back_the_output_it_leaves() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 48_000.0), (SPEAKERS, 48_000.0)]));
        holds.hal.set_elsewhere(SPEAKERS, 1024);
        let monitor = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        let speakers = holds.hold(SPEAKERS, PERIOD, || {}).unwrap();
        drop(monitor);
        assert_eq!((holds.hal.size_of(MONITOR), holds.hal.size_of(SPEAKERS)), (512, PERIOD));
        drop(speakers);
        assert_eq!(holds.hal.size_of(SPEAKERS), 1024);
    }

    /// The next song's stream finds what was chosen and puts that back in turn.
    #[test]
    fn a_size_set_elsewhere_while_open_is_left_alone() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 48_000.0)]));
        let hold = holds.hold(MONITOR, PERIOD, || {}).unwrap();
        holds.hal.set_elsewhere(MONITOR, 256);
        drop(hold);
        assert_eq!(holds.hal.size_of(MONITOR), 256);

        drop(holds.hold(MONITOR, PERIOD, || {}).unwrap());
        assert_eq!(holds.hal.size_of(MONITOR), 256);
    }

    #[test]
    fn a_size_the_device_refuses_fails_the_hold_and_puts_back_only_the_rate() {
        let fake = Fake { refuses: vec![MONITOR], ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        assert!(holds.hold(MONITOR, PERIOD, || {}).is_err());
        let asked = [Ask::Rate(48_000.0), Ask::Frames(PERIOD), Ask::Rate(44_100.0)];
        assert_eq!(holds.hal.asked(), asked.map(|ask| (MONITOR, ask)));
        assert!(
            holds.held.lock().unwrap().iter().all(|held| held.users == 0 && held.buffer.is_none())
        );
    }

    /// `open` marks the rate landing through the hold and the buffer after it, so a start that
    /// fails later, here on the stream's format, says where its time went.
    #[test]
    fn a_start_that_fails_after_the_hold_names_the_rate_then_the_buffer() {
        let holds = DeviceHolds::new(Fake::new(&[(MONITOR, 44_100.0)]));
        let mut steps = Steps::new();
        let hold = holds.hold(MONITOR, PERIOD, || steps.done("48 kHz held")).unwrap();
        steps.done("IO buffer set");
        let failed = steps.failed(DriverError::Backend("not f32 PCM".into()));
        drop(hold);
        let reason = failed.to_string();
        let rate = reason.find(", after 48 kHz held at ").expect(&reason);
        let buffer = reason.find(", IO buffer set at ").expect(&reason);
        assert!(rate < buffer, "{reason}");
    }

    #[test]
    fn a_start_that_fails_setting_the_buffer_says_the_rate_had_landed() {
        let fake = Fake { refuses: vec![MONITOR], ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        let mut steps = Steps::new();
        let asked_when_landed = Mutex::new(Vec::new());
        let error = holds
            .hold(MONITOR, PERIOD, || {
                *asked_when_landed.lock().unwrap() = holds.hal.asked();
                steps.done("48 kHz held");
            })
            .map(drop)
            .unwrap_err();
        let landed = [(MONITOR, Ask::Rate(48_000.0))];
        assert_eq!(
            *asked_when_landed.lock().unwrap(),
            landed,
            "marked before the buffer was asked"
        );
        let reason = steps.failed(error).to_string();
        assert!(reason.contains("the device refused the size"), "{reason}");
        assert!(reason.contains(", after 48 kHz held at "), "{reason}");
        assert!(!reason.contains("IO buffer set"), "{reason}");
    }

    #[test]
    fn a_rate_that_never_lands_marks_nothing() {
        let fake = Fake { first: First::Never, ..Fake::new(&[(MONITOR, 44_100.0)]) };
        let holds = DeviceHolds::new(fake);
        let mut landed = false;
        assert!(holds.hold(MONITOR, PERIOD, || landed = true).is_err());
        assert!(!landed);
    }
}
