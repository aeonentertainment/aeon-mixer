use std::sync::{Mutex, PoisonError};

use engine::driver::DriverError;

use crate::buffer_hold::BufferHold;
use crate::io_buffer::IoBuffer;

/// The devices whose IO buffer this process's streams set, each with the size it was found at,
/// the size the engine left on it, and how many streams use it. An output and a house mic can be
/// one USB interface, so the last stream out puts the size back, and only while the device still
/// has the engine's: a size something else set since is theirs to keep. Never touched from an
/// audio thread.
pub struct BufferHolds<B> {
    hal: B,
    held: Mutex<Vec<Held>>,
}

struct Held {
    device: u32,
    found: u32,
    ours: u32,
    users: u32,
}

impl<B: IoBuffer> BufferHolds<B> {
    pub const fn new(hal: B) -> Self {
        Self { hal, held: Mutex::new(Vec::new()) }
    }

    /// Sets `device`'s IO buffer to `frames` for one stream until the hold drops. The first stream
    /// on a device takes the size it reads as the one to put back, so a size chosen between songs
    /// is the one kept. A set that fails holds nothing.
    pub fn hold(&self, device: u32, frames: u32) -> Result<BufferHold<'_, B>, DriverError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let took = match held.iter_mut().find(|held| held.device == device) {
            Some(entry) => {
                self.hal.put_frames(device, frames)?;
                entry.ours = self.hal.frames(device)?;
                entry.users += 1;
                entry.ours
            }
            None => {
                let found = self.hal.frames(device)?;
                self.hal.put_frames(device, frames)?;
                let ours = self.hal.frames(device)?;
                held.push(Held { device, found, ours, users: 1 });
                ours
            }
        };
        Ok(BufferHold { holds: self, device, frames: took })
    }

    pub(crate) fn release(&self, device: u32) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(at) = held.iter().position(|held| held.device == device) else { return };
        held[at].users -= 1;
        if held[at].users > 0 {
            return;
        }
        let Held { found, ours, .. } = held.swap_remove(at);
        if found != ours && self.hal.frames(device).is_ok_and(|now| now == ours) {
            // Best effort: a device that has gone takes its size with it
            let _ = self.hal.put_frames(device, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Devices by id with their IO buffer sizes, and every size asked for in order. A device in
    /// `refuses` fails every set.
    #[derive(Default)]
    struct Fake {
        sizes: Mutex<Vec<(u32, u32)>>,
        asked: Mutex<Vec<(u32, u32)>>,
        refuses: Vec<u32>,
    }

    impl Fake {
        fn new(sizes: &[(u32, u32)]) -> Self {
            Self { sizes: Mutex::new(sizes.to_vec()), ..Self::default() }
        }

        fn size_of(&self, device: u32) -> u32 {
            self.frames(device).unwrap()
        }

        fn asked(&self) -> Vec<(u32, u32)> {
            self.asked.lock().unwrap().clone()
        }

        /// Another app, or a person in Audio MIDI Setup, sets the size.
        fn set_elsewhere(&self, device: u32, frames: u32) {
            let mut sizes = self.sizes.lock().unwrap();
            sizes.iter_mut().find(|(id, _)| *id == device).unwrap().1 = frames;
        }
    }

    impl IoBuffer for Fake {
        fn frames(&self, device: u32) -> Result<u32, DriverError> {
            let sizes = self.sizes.lock().unwrap();
            sizes
                .iter()
                .find(|(id, _)| *id == device)
                .map(|(_, frames)| *frames)
                .ok_or(DriverError::NoDevice)
        }

        fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError> {
            self.asked.lock().unwrap().push((device, frames));
            if self.refuses.contains(&device) {
                return Err(DriverError::Backend("the device refused the size".into()));
            }
            self.set_elsewhere(device, frames);
            Ok(())
        }
    }

    const MONITOR: u32 = 7;
    const SPEAKERS: u32 = 9;

    #[test]
    fn the_size_found_is_put_back_when_the_stream_closes() {
        let holds = BufferHolds::new(Fake::new(&[(MONITOR, 512)]));
        let hold = holds.hold(MONITOR, 128).unwrap();
        assert_eq!((hold.frames, holds.hal.size_of(MONITOR)), (128, 128));
        drop(hold);
        assert_eq!(holds.hal.size_of(MONITOR), 512);
    }

    /// A move opens the new output's stream and drops the old one's.
    #[test]
    fn a_move_puts_back_the_output_it_leaves() {
        let holds = BufferHolds::new(Fake::new(&[(MONITOR, 512), (SPEAKERS, 1024)]));
        let monitor = holds.hold(MONITOR, 128).unwrap();
        let speakers = holds.hold(SPEAKERS, 128).unwrap();
        drop(monitor);
        assert_eq!((holds.hal.size_of(MONITOR), holds.hal.size_of(SPEAKERS)), (512, 128));
        drop(speakers);
        assert_eq!(holds.hal.size_of(SPEAKERS), 1024);
    }

    /// The next song's stream finds what was chosen and puts that back in turn.
    #[test]
    fn a_size_set_elsewhere_while_open_is_left_alone() {
        let holds = BufferHolds::new(Fake::new(&[(MONITOR, 512)]));
        let hold = holds.hold(MONITOR, 128).unwrap();
        holds.hal.set_elsewhere(MONITOR, 256);
        drop(hold);
        assert_eq!(holds.hal.size_of(MONITOR), 256);

        drop(holds.hold(MONITOR, 128).unwrap());
        assert_eq!(holds.hal.size_of(MONITOR), 256);
    }

    #[test]
    fn a_device_two_streams_use_keeps_the_period_until_both_let_go() {
        let holds = BufferHolds::new(Fake::new(&[(MONITOR, 512)]));
        let output = holds.hold(MONITOR, 128).unwrap();
        let input = holds.hold(MONITOR, 128).unwrap();
        drop(output);
        assert_eq!(holds.hal.size_of(MONITOR), 128, "the input still runs on it");
        drop(input);
        assert_eq!(holds.hal.size_of(MONITOR), 512);
    }

    #[test]
    fn a_size_the_device_refuses_holds_nothing_and_puts_nothing_back() {
        let fake = Fake { refuses: vec![MONITOR], ..Fake::new(&[(MONITOR, 512)]) };
        let holds = BufferHolds::new(fake);
        assert!(holds.hold(MONITOR, 128).is_err());
        assert_eq!(holds.hal.asked(), [(MONITOR, 128)]);
        assert!(holds.held.lock().unwrap().is_empty());
    }
}
