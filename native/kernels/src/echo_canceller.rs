mod delay_finder;
mod fft;
mod history;
mod lowpass;
mod mdf;
#[cfg(test)]
mod tests;

use delay_finder::DelayFinder;
pub use history::EchoHistory;
use mdf::Mdf;

/// The engine's block, and the canceller's frame: it works in the slot's own blocks, so it holds
/// nothing back and adds no delay.
pub const FRAME: usize = 128;
const WINDOW: usize = 2 * FRAME;
const BINS: usize = FRAME + 1;
/// The filter's tail: 64 frames, 171 ms at 48 kHz, past which a living room's echo has fallen
/// 30 dB or more. The finder places the start of it, so it holds the room and not the path's
/// bulk delay.
const PARTITIONS: usize = 64;
/// mdf.c's arithmetic is tuned to 16-bit samples.
const SCALE: f32 = 32768.0;
/// How far ahead of the found echo the filter starts, for the finder's 16-frame resolution and
/// the direct path's rise.
const LEAD: usize = 64;
/// Frames a source's latency must hold still before the canceller follows it: a jitter buffer
/// that steps its delay holds the new one from its first frame, and a push ring's fill, which
/// falls a frame at every pull and climbs a chunk at every push, never holds for two.
const SETTLE: u32 = 2;
/// A lag the finder names this close to the one in use is the same path.
const SAME_PATH: i64 = 2 * LEAD as i64;
/// Once adapted, the filter stops learning while the mic is this many times the energy of the
/// echo it estimates: a singer over the echo, whose voice the filter would otherwise partly fit
/// through the backing's own notes. 3 dB, a Geigel detector on energies: 1.5 dB slows the first
/// convergence, and 6 dB takes 5 dB less of the echo out while a singer sings over it.
const DOUBLE_TALK: f32 = 2.0;
/// The double-talk detector's smoothing per frame, about 20 ms.
const TALK_SMOOTHING: f32 = 0.125;
/// One second of blocks at 48 kHz, the meters' smoothing.
const METER_BLOCKS: f32 = 375.0;

/// What the canceller has found, once its filter has adapted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EchoFindings {
    /// From a master frame being rendered to its echo arriving in the slot.
    pub path_frames: u32,
    /// The echo's level under the master's, over the last second.
    pub bleed_db: f32,
    /// How much quieter the slot came out than it went in, over the last second.
    pub removed_db: f32,
}

/// Takes the room's echo of the master out of one mic, with the master as its reference. The
/// slot's source says how long ago the singer sang what it hands over; the canceller finds the
/// rest of the path, the output, the air and whatever the sender adds, and follows the source's
/// own moves at once instead of finding the echo again. Built on the control thread; `process`
/// allocates nothing.
pub struct EchoCanceller {
    finder: DelayFinder,
    mdf: Mdf,
    /// The lag the finder last named.
    found: Option<usize>,
    /// The part of the path the engine cannot know: the found lag less the source's latency.
    unknown: Option<i64>,
    known: Known,
    /// The lag the filter's reference is read at now.
    alignment: Option<usize>,
    adapted: bool,
    meters: Meters,
    /// The mic's energy and the echo estimate's, smoothed over about 20 ms.
    talk: (f32, f32),
    newest: [f32; FRAME],
    raw: [f32; WINDOW + 1],
    own_raw: [f32; WINDOW + 1],
    window: [f32; WINDOW],
    others: [f32; WINDOW],
    mic: [f32; FRAME],
    out: [f32; FRAME],
}

/// A source's latency as the canceller follows it.
#[derive(Clone, Copy, Default)]
struct Known {
    followed: Option<u32>,
    candidate: u32,
    held: u32,
}

#[derive(Clone, Copy, Default)]
struct Meters {
    mic: f32,
    out: f32,
    echo: f32,
    reference: f32,
}

impl EchoCanceller {
    pub fn new(rate: u32) -> Self {
        Self {
            finder: DelayFinder::new(rate),
            mdf: Mdf::new(rate, PARTITIONS),
            found: None,
            unknown: None,
            known: Known::default(),
            alignment: None,
            adapted: false,
            meters: Meters::default(),
            talk: (0.0, 0.0),
            newest: [0.0; FRAME],
            raw: [0.0; WINDOW + 1],
            own_raw: [0.0; WINDOW + 1],
            window: [0.0; WINDOW],
            others: [0.0; WINDOW],
            mic: [0.0; FRAME],
            out: [0.0; FRAME],
        }
    }

    /// A new sender on the slot: a new path to find.
    pub fn reset(&mut self) {
        self.finder.reset();
        self.mdf.reset();
        self.found = None;
        self.unknown = None;
        self.known = Known::default();
        self.alignment = None;
        self.adapted = false;
        self.meters = Meters::default();
        self.talk = (0.0, 0.0);
    }

    /// None until the echo is found and the filter has adapted to it.
    pub fn findings(&self) -> Option<EchoFindings> {
        let alignment = self.alignment.filter(|_| self.adapted)?;
        let db = |num: f32, den: f32| 10.0 * ((num + 1e-12) / (den + 1e-12)).log10();
        Some(EchoFindings {
            path_frames: (alignment + LEAD) as u32,
            bleed_db: db(self.meters.echo, self.meters.reference),
            removed_db: db(self.meters.mic, self.meters.out),
        })
    }

    /// One frame of the mic, in place. `history` holds the master up to the frame before this
    /// one, and `own` the part of it that was this mic's own voice; `known_frames` is how long
    /// ago the singer sang what the slot's source handed over. Until the echo is found the frame
    /// passes untouched.
    pub fn process(
        &mut self,
        history: &EchoHistory,
        own: &EchoHistory,
        known_frames: u32,
        block: &mut [f32],
    ) {
        let Ok(block) = <&mut [f32; FRAME]>::try_from(block) else { return };
        history.read(0, &mut self.newest);
        let found = self.finder.process(&self.newest, block);
        let known = self.follow(known_frames);

        if found.is_some() && found != self.found {
            self.found = found;
            let unknown = found.map_or(0, |lag| lag as i64) - i64::from(known);
            if self.unknown.is_none_or(|was| (was - unknown).abs() > SAME_PATH) {
                // A new path: what the filter learned was about another
                self.unknown = Some(unknown);
                self.mdf.reset();
                self.alignment = None;
                self.adapted = false;
            }
        }
        let Some(unknown) = self.unknown else { return };
        let most = EchoHistory::reach() - PARTITIONS * FRAME - WINDOW - 1;
        let lag = (unknown + i64::from(known) - LEAD as i64).clamp(FRAME as i64, most as i64);
        let alignment = lag as usize;
        if self.alignment != Some(alignment) {
            self.alignment = Some(alignment);
            let Self { mdf, raw, own_raw, .. } = self;
            mdf.refill(|j, whole, others| {
                let lag = alignment + j * FRAME;
                emphasised(history, lag, raw, whole);
                emphasised(own, lag, own_raw, others);
                others.iter_mut().zip(whole.iter()).for_each(|(o, w)| *o = w - *o);
            });
        }

        // This frame's window ends `alignment` frames before the mic's first sample, a frame on
        emphasised(history, alignment - FRAME, &mut self.raw, &mut self.window);
        emphasised(own, alignment - FRAME, &mut self.own_raw, &mut self.others);
        self.others.iter_mut().zip(&self.window).for_each(|(o, w)| *o = w - *o);
        for (scaled, sample) in self.mic.iter_mut().zip(block.iter()) {
            *scaled = sample * SCALE;
        }
        if self.adapted && self.talk.0 > DOUBLE_TALK * self.talk.1 {
            self.mdf.hold(1);
        }
        let frame = self.mdf.process(&self.mic, &self.window, &self.others, &mut self.out);
        self.adapted = frame.adapted;

        let mut energies = Meters::default();
        for ((sample, out), reference) in
            block.iter_mut().zip(&self.out).zip(&self.raw[FRAME + 1..])
        {
            let out = out / SCALE;
            energies.mic += *sample * *sample;
            energies.out += out * out;
            energies.echo += (*sample - out) * (*sample - out);
            energies.reference += reference * reference;
            *sample = out;
        }
        let smooth = |meter: &mut f32, energy: f32| *meter += (energy - *meter) / METER_BLOCKS;
        smooth(&mut self.meters.mic, energies.mic);
        smooth(&mut self.meters.out, energies.out);
        smooth(&mut self.meters.echo, energies.echo);
        smooth(&mut self.meters.reference, energies.reference);
        self.talk.0 += TALK_SMOOTHING * (energies.mic - self.talk.0);
        self.talk.1 += TALK_SMOOTHING * (energies.echo - self.talk.1);
    }

    fn follow(&mut self, known_frames: u32) -> u32 {
        let known = &mut self.known;
        if known_frames == known.candidate {
            known.held += 1;
        } else {
            known.candidate = known_frames;
            known.held = 1;
        }
        if known.followed.is_none() || known.held >= SETTLE {
            known.followed = Some(known.candidate);
        }
        known.followed.unwrap_or(known_frames)
    }
}

/// The reference window that ends `lag` frames before the next frame rendered, pre-emphasised as
/// mdf.c's input is and on its 16-bit scale. `raw` keeps the window as played, and the frame
/// before it for the first sample's emphasis.
fn emphasised(
    history: &EchoHistory,
    lag: usize,
    raw: &mut [f32; WINDOW + 1],
    out: &mut [f32; WINDOW],
) {
    history.read(lag, raw);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample = (raw[i + 1] - 0.9 * raw[i]) * SCALE;
    }
}
