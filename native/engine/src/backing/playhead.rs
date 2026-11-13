use crate::backing::Ramp;
use crate::block::{BLOCK, RATE};
use crate::command::{Event, EventProducer};

/// 20 ms: the fade out before a seek jumps or a pause holds, and the fade in after.
pub const SEEK_FADE_FRAMES: u32 = RATE / 50;

/// The backing path's clock: a frame counter that advances a block per render while playing.
/// It acks each command at the block the command takes effect, a seek or a pause once its fade
/// out is done, and raises `BackingEnded` once when the stems run out or `stopAt` falls silent.
/// Positions it reports trail the frame it reads by the live key's latency, so they name the
/// audio the room hears.
pub struct Playhead {
    frame: u64,
    end: u64,
    latency: u64,
    motion: Motion,
    level: Ramp,
    stop: Option<Stop>,
    since_position: u32,
}

#[derive(Clone, Copy)]
enum Motion {
    Idle,
    Playing,
    /// Fading out before a seek jumps, a pause holds, or both.
    Leaving {
        seek: Option<Seek>,
        pause: Option<u32>,
    },
    Paused,
    Ended,
}

#[derive(Clone, Copy)]
struct Seek {
    id: u32,
    to: u64,
}

/// The stems fall from full level at `from` to silence at `to`.
#[derive(Clone, Copy)]
struct Stop {
    from: u64,
    to: u64,
}

impl Playhead {
    /// A playhead over stems `end` frames long, waiting for `start`.
    pub fn new(end: u64) -> Self {
        Self {
            frame: 0,
            end,
            latency: 0,
            motion: Motion::Idle,
            level: Ramp::new(SEEK_FADE_FRAMES),
            stop: None,
            since_position: 0,
        }
    }

    /// The frame the room hears now.
    pub fn position(&self) -> u64 {
        self.frame.saturating_sub(self.latency)
    }

    /// How far the audio heard trails the frame read; the live key sets it.
    pub fn set_latency(&mut self, frames: u64) {
        self.latency = frames;
    }

    /// Whether the next block moves the playhead on: playing, or fading out to a seek or a pause.
    pub fn is_advancing(&self) -> bool {
        matches!(self.motion, Motion::Playing | Motion::Leaving { .. })
    }

    /// Whether the playhead holds still on pause, where the live key still drains its tail.
    pub fn is_paused(&self) -> bool {
        matches!(self.motion, Motion::Paused)
    }

    pub fn ack(&self, id: u32, events: &mut EventProducer) {
        events.push(Event::Ack { id, position_frames: self.position() });
    }

    pub fn start(&mut self, id: u32, events: &mut EventProducer) {
        if let Motion::Idle = self.motion {
            self.motion = Motion::Playing;
            self.level.jump(1.0);
        }
        self.ack(id, events);
    }

    /// While playing, fades out and jumps once silent. A seek past the end lands on it. A seek
    /// that a later one replaces before it lands is acked with the position at that moment.
    pub fn seek(&mut self, id: u32, to: u64, events: &mut EventProducer) {
        let to = to.min(self.end);
        let next = Seek { id, to };
        match &mut self.motion {
            Motion::Playing => self.leave(Some(next), None),
            Motion::Leaving { seek, .. } => {
                if let Some(replaced) = seek.replace(next) {
                    self.ack(replaced.id, events);
                }
            }
            Motion::Idle | Motion::Paused => {
                self.frame = to;
                self.ack(id, events);
            }
            Motion::Ended => self.ack(id, events),
        }
    }

    pub fn pause(&mut self, id: u32, events: &mut EventProducer) {
        match &mut self.motion {
            Motion::Playing => self.leave(None, Some(id)),
            Motion::Leaving { pause, .. } => {
                if let Some(replaced) = pause.replace(id) {
                    self.ack(replaced, events);
                }
            }
            Motion::Idle | Motion::Paused | Motion::Ended => self.ack(id, events),
        }
    }

    /// Fades back in from a pause, or calls off one still fading out.
    pub fn resume(&mut self, id: u32, events: &mut EventProducer) {
        match self.motion {
            Motion::Paused => self.play(),
            Motion::Leaving { seek, pause: Some(called_off) } => {
                self.ack(called_off, events);
                match seek {
                    Some(_) => self.motion = Motion::Leaving { seek, pause: None },
                    None => self.play(),
                }
            }
            Motion::Idle | Motion::Playing | Motion::Leaving { .. } | Motion::Ended => {}
        }
        self.ack(id, events);
    }

    /// Silent at `at`, after `fade` frames of fade. When `at` is too close for the whole fade it
    /// fades from now; when `at` has passed, it fades for `fade` from now.
    pub fn stop_at(&mut self, id: u32, at: u64, fade: u32, events: &mut EventProducer) {
        let now = self.frame;
        self.stop = Some(if at > now {
            Stop { from: at.saturating_sub(u64::from(fade)).max(now), to: at }
        } else {
            Stop { from: now, to: now + u64::from(fade) }
        });
        self.ack(id, events);
    }

    /// Advances one block. Returns the first frame to read, with each sample's fade written to
    /// `fades`, or `None` when nothing is read. Allocation-free.
    pub fn next_block(
        &mut self,
        fades: &mut [f32; BLOCK],
        events: &mut EventProducer,
    ) -> Option<u64> {
        let (Motion::Playing | Motion::Leaving { .. }) = self.motion else { return None };
        let start = self.frame;
        for (frame, fade) in (start..).zip(fades.iter_mut()) {
            *fade = self.level.next() * self.stop_level(frame);
        }
        self.frame += BLOCK as u64;

        if let Motion::Leaving { seek, pause } = self.motion
            && self.level.rests_at(0.0)
        {
            if let Some(seek) = seek {
                self.frame = seek.to;
            }
            match pause {
                Some(_) => self.motion = Motion::Paused,
                None => self.play(),
            }
            for id in seek.map(|s| s.id).into_iter().chain(pause) {
                self.ack(id, events);
            }
        }

        self.since_position += BLOCK as u32;
        if self.since_position >= RATE {
            self.since_position -= RATE;
            events.push(Event::Position { frames: self.position() });
        }

        let end = self.stop.map_or(self.end, |stop| stop.to.min(self.end));
        let landing = matches!(self.motion, Motion::Leaving { seek: Some(_), .. });
        if self.position() >= end && !landing {
            if let Motion::Leaving { pause: Some(id), .. } = self.motion {
                self.ack(id, events);
            }
            self.motion = Motion::Ended;
            events.push(Event::BackingEnded);
        }
        Some(start)
    }

    fn leave(&mut self, seek: Option<Seek>, pause: Option<u32>) {
        self.motion = Motion::Leaving { seek, pause };
        self.level.aim(0.0);
    }

    fn play(&mut self) {
        self.motion = Motion::Playing;
        self.level.aim(1.0);
    }

    fn stop_level(&self, frame: u64) -> f32 {
        match self.stop {
            Some(Stop { to, .. }) if frame >= to => 0.0,
            Some(Stop { from, to }) if frame > from => (to - frame) as f32 / (to - from) as f32,
            _ => 1.0,
        }
    }
}
