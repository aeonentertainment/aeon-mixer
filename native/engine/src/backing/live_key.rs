use kernels::{Preset, Stretch};

use crate::backing::Ramp;
use crate::block::{BLOCK, RATE};

/// 50 ms: the live key crossfade.
pub const CROSSFADE_FRAMES: u32 = RATE / 20;

/// The live key change on the summed stems. Two stretchers, built on the control thread; the
/// stems pass dry until the first change. A change resets the idle stretcher at the new key and
/// feeds it for its latency, so it has warmed past its silent start, then crossfades to it. A
/// change asked for mid-crossfade starts once that crossfade ends.
pub struct LiveKey {
    stretchers: [Stretch; 2],
    latency: usize,
    /// The stretcher the room hears and its semitones; `None` while dry.
    heard: Option<usize>,
    heard_semitones: i8,
    wanted: i8,
    incoming: Option<Incoming>,
    fade: Ramp,
    dry: [[f32; BLOCK]; 2],
    next: [[f32; BLOCK]; 2],
}

struct Incoming {
    stretcher: usize,
    semitones: i8,
    fed: usize,
}

impl LiveKey {
    pub fn new() -> Self {
        let stretchers =
            [Stretch::new(2, RATE, Preset::Cheaper), Stretch::new(2, RATE, Preset::Cheaper)];
        Self {
            latency: stretchers[0].latency_frames(),
            stretchers,
            heard: None,
            heard_semitones: 0,
            wanted: 0,
            incoming: None,
            fade: Ramp::new(CROSSFADE_FRAMES),
            dry: [[0.0; BLOCK]; 2],
            next: [[0.0; BLOCK]; 2],
        }
    }

    /// The live semitones the room hears, which change when a crossfade completes.
    pub fn semitones(&self) -> i8 {
        self.heard_semitones
    }

    /// How far the audio heard trails its input; 0 while dry.
    pub fn latency_frames(&self) -> usize {
        if self.heard.is_some() { self.latency } else { 0 }
    }

    pub fn set(&mut self, semitones: i8) {
        self.wanted = semitones;
        if let Some(incoming) = &mut self.incoming
            && incoming.fed <= self.latency
        {
            incoming.semitones = semitones;
            self.stretchers[incoming.stretcher].set_transpose_semitones(f32::from(semitones));
        }
    }

    /// One block of the summed stems, in place. Allocation-free.
    pub fn process(&mut self, left: &mut [f32; BLOCK], right: &mut [f32; BLOCK]) {
        if self.incoming.is_none() && self.wanted != self.heard_semitones {
            let stretcher = self.heard.map_or(0, |heard| 1 - heard);
            self.stretchers[stretcher].reset();
            self.stretchers[stretcher].set_transpose_semitones(f32::from(self.wanted));
            self.incoming = Some(Incoming { stretcher, semitones: self.wanted, fed: 0 });
            self.fade.jump(0.0);
        }
        self.dry = [*left, *right];
        let [dry_left, dry_right] = self.dry.each_ref().map(|c| c.as_slice());
        if let Some(heard) = self.heard {
            self.stretchers[heard]
                .process(&[dry_left, dry_right], &mut [left.as_mut_slice(), right.as_mut_slice()]);
        }

        let Some(incoming) = &mut self.incoming else { return };
        let [next_left, next_right] = self.next.each_mut().map(|c| c.as_mut_slice());
        self.stretchers[incoming.stretcher]
            .process(&[dry_left, dry_right], &mut [next_left, next_right]);
        incoming.fed += BLOCK;
        if incoming.fed <= self.latency {
            return;
        }
        self.fade.aim(1.0);
        for i in 0..BLOCK {
            let fade = self.fade.next();
            left[i] += (self.next[0][i] - left[i]) * fade;
            right[i] += (self.next[1][i] - right[i]) * fade;
        }
        if self.fade.rests_at(1.0) {
            self.heard = Some(incoming.stretcher);
            self.heard_semitones = incoming.semitones;
            self.incoming = None;
        }
    }
}

impl Default for LiveKey {
    fn default() -> Self {
        Self::new()
    }
}
