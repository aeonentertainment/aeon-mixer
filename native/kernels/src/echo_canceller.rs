mod between;
mod delay_finder;
mod drift;
mod fft;
mod history;
mod lowpass;
mod mdf;
#[cfg(test)]
mod tests;

use between::Between;
use delay_finder::DelayFinder;
use drift::Drift;
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
/// mdf.c's pre-emphasis of the reference.
const PREEMPHASIS: f32 = 0.9;
/// How far ahead of the found echo the filter starts, for the finder's 16-frame resolution and
/// the direct path's rise.
const LEAD: usize = 64;
/// Frames a source's latency must hold still before the canceller follows it: a jitter buffer
/// that steps its delay holds the new one from its first frame, and a push ring's fill, which
/// falls a frame at every pull and climbs a chunk at every push, never holds for two.
const SETTLE: u32 = 2;
/// A lag the finder names this close to the one in use is the same path.
const SAME_PATH: i64 = 2 * LEAD as i64;
/// A lag the finder names is judged only once the source's recent splices, halving every three
/// seconds, come to less than a lead. The finder takes time to follow an echo the source has
/// moved: after a capture slot's 128-frame correction it named lags up to 259 frames from where
/// the splices put the path for up to two seconds, and 1.6 s of waiting was not enough. Taken for
/// a new path, the filter started over and came out up to 26.5 dB under the same mic on the
/// output's clock, more than 3 dB under for eight seconds. So such a correction holds the finder
/// about three seconds, and a lag named meanwhile is judged then. A 480-frame mic's 32-frame
/// corrections, or a finer step's few frames a second, never move the finder that far, and never
/// hold it.
const SPLICES_SETTLE: f32 = LEAD as f32;
/// Three seconds of blocks halve the recent splices.
const SPLICES_FADE: f32 = 0.999_384;
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

/// Where a slot's source has its delay, block by block: `latency_frames`, how long ago the singer
/// sang what it hands over, which the canceller follows once it holds for two blocks;
/// `spliced_frames`, the frames it has spliced into what it hands over since it began, less those
/// it took out, which the canceller follows the block they happen; and `streams`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceDelay {
    pub latency_frames: u32,
    pub spliced_frames: i64,
    /// Streams the source has started. A new one starts its delay over at the latency, so the
    /// echo is back where the latency says, however far the last one had drifted.
    pub streams: u32,
}

/// Takes the room's echo of the master out of one mic, with the master as its reference. The
/// slot's source says how long ago the singer sang what it hands over; the canceller finds the
/// rest of the path, the output, the air and whatever the sender adds, and follows the source's
/// own moves at once instead of finding the echo again. A mic on its own clock slides its echo a
/// few samples a second against the master, and the canceller follows that too. Built on the
/// control thread; `process` allocates nothing.
pub struct EchoCanceller {
    finder: DelayFinder,
    mdf: Mdf,
    /// The lag the finder last named.
    found: Option<usize>,
    /// The part of the path the engine cannot know: the found lag less the source's latency.
    unknown: Option<i64>,
    known: Known,
    /// The lag the filter's reference is read at now, less the drift's offset.
    alignment: Option<usize>,
    drift: Drift,
    /// The source's splices and streams as of the block before.
    spliced: Option<i64>,
    streams: Option<u32>,
    /// The source's recent splices, in frames, fading.
    recent_splices: f32,
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
    echo: [f32; FRAME],
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
            drift: Drift::default(),
            spliced: None,
            streams: None,
            recent_splices: 0.0,
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
            echo: [0.0; FRAME],
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
        self.drift.reset();
        self.spliced = None;
        self.streams = None;
        self.recent_splices = 0.0;
        self.adapted = false;
        self.meters = Meters::default();
        self.talk = (0.0, 0.0);
    }

    /// None until the echo is found and the filter has adapted to it.
    pub fn findings(&self) -> Option<EchoFindings> {
        let alignment = self.alignment.filter(|_| self.adapted)?;
        let db = |num: f32, den: f32| 10.0 * ((num + 1e-12) / (den + 1e-12)).log10();
        Some(EchoFindings {
            path_frames: ((alignment + LEAD) as f64 + self.drift.offset()).round().max(0.0) as u32,
            bleed_db: db(self.meters.echo, self.meters.reference),
            removed_db: db(self.meters.mic, self.meters.out),
        })
    }

    /// One frame of the mic, in place. `history` holds the master up to the frame before this
    /// one, and `own` the part of it that was this mic's own voice; `source` is where the slot's
    /// source had its delay for this frame. Until the echo is found the frame passes untouched.
    pub fn process(
        &mut self,
        history: &EchoHistory,
        own: &EchoHistory,
        source: SourceDelay,
        block: &mut [f32],
    ) {
        let Ok(block) = <&mut [f32; FRAME]>::try_from(block) else { return };
        history.read(0, &mut self.newest);
        let found = self.finder.process(&self.newest, block);
        let known = self.follow(source.latency_frames);
        let spliced = source.spliced_frames - self.spliced.unwrap_or(source.spliced_frames);
        self.spliced = Some(source.spliced_frames);
        self.recent_splices = self.recent_splices * SPLICES_FADE + spliced.unsigned_abs() as f32;
        let restarted = self.streams.is_some_and(|was| was != source.streams);
        self.streams = Some(source.streams);
        if restarted {
            // A new stream: the echo is back where the latency says, and the clocks are the same
            self.drift.restart();
            self.alignment = None;
        }
        let read_at = self.drift.offset();
        self.drift.shift(spliced as f64);

        // Until the source has settled, a lag the finder names is not taken, nor kept as the one
        // it named, so it is judged again once the source has
        let settled = self.unknown.is_none() || self.recent_splices < SPLICES_SETTLE;
        if let Some(lag) = found.filter(|_| settled && found != self.found) {
            self.found = found;
            let unknown = lag as i64 - i64::from(known);
            let followed = unknown - self.drift.offset().round() as i64;
            if self.unknown.is_none_or(|was| (was - followed).abs() > SAME_PATH) {
                // A new path: what the filter learned was about another
                self.unknown = Some(unknown);
                self.drift.restart();
                self.mdf.reset();
                self.alignment = None;
                self.adapted = false;
            }
        }
        let Some(unknown) = self.unknown else { return };
        let most = EchoHistory::reach() - PARTITIONS * FRAME - WINDOW - 1;
        let lag = (unknown + i64::from(known) - LEAD as i64).clamp(FRAME as i64, most as i64);
        let alignment = lag as usize;
        if self.alignment == Some(alignment) && spliced.abs() == 1 {
            // A splice moves the echo a sample, and every window the filter holds slides by one.
            // `edge(history, more)` is the pre-emphasised sample `more` older than the one after
            // the newest of window 0, read where the windows were read
            let after = Between::new(alignment as f64 + read_at - 1.0);
            let edge = |history: &EchoHistory, more: usize| {
                let mut pair = [0.0; 2];
                after.read(history, more, &mut pair);
                (pair[1] - PREEMPHASIS * pair[0]) * SCALE
            };
            let older = spliced > 0;
            self.mdf.slide(older, |j| {
                let newest = j * FRAME + 1;
                // Older, the frame before the window enters and its newest leaves; newer, the
                // frame after it enters and its oldest leaves
                let (enters, leaves) = if older {
                    (newest + WINDOW, newest)
                } else {
                    (newest - 1, newest + WINDOW - 1)
                };
                let whole = edge(history, enters) - edge(history, leaves);
                let others = whole - (edge(own, enters) - edge(own, leaves));
                (whole, others)
            });
        } else if self.alignment != Some(alignment) || spliced != 0 {
            // A jump, of the path the source names or by more than a sample: read every window
            self.alignment = Some(alignment);
            let Self { mdf, raw, own_raw, drift, .. } = self;
            mdf.refill(|j, whole, others| {
                let lag = (alignment + j * FRAME) as f64 + drift.offset();
                emphasised(history, lag, raw, whole);
                emphasised(own, lag, own_raw, others);
                others.iter_mut().zip(whole.iter()).for_each(|(o, w)| *o = w - *o);
            });
        }

        // This frame's window ends `alignment` frames before the mic's first sample, a frame on,
        // and as far again as the path has drifted
        let lag = (alignment - FRAME) as f64 + self.drift.offset();
        emphasised(history, lag, &mut self.raw, &mut self.window);
        emphasised(own, lag, &mut self.own_raw, &mut self.others);
        self.others.iter_mut().zip(&self.window).for_each(|(o, w)| *o = w - *o);
        for (scaled, sample) in self.mic.iter_mut().zip(block.iter()) {
            *scaled = sample * SCALE;
        }
        let talking = self.talk.0 > DOUBLE_TALK * self.talk.1;
        if self.adapted && talking {
            self.mdf.hold(1);
        }

        let frame = self.mdf.process(&self.mic, &self.window, &self.others, &mut self.out);
        self.adapted = frame.adapted;
        for ((echo, mic), out) in self.echo.iter_mut().zip(&self.mic).zip(&self.out) {
            *echo = mic - out;
        }
        self.drift.follow(&self.echo, &self.out, self.adapted && !talking);

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
    lag: f64,
    raw: &mut [f32; WINDOW + 1],
    out: &mut [f32; WINDOW],
) {
    Between::new(lag).read(history, 0, raw);
    for (i, sample) in out.iter_mut().enumerate() {
        *sample = (raw[i + 1] - PREEMPHASIS * raw[i]) * SCALE;
    }
}
