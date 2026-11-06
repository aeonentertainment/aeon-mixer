//! The real chain on the offline driver: a source, one engaged `Slot`, the `Graph` around it with
//! a backing playhead for the guide clock, the master, and `OfflineDriver` as the clock. It
//! renders `Graph` directly rather than through `AudioThread` because the delay row reads the
//! slot's state between blocks; what `AudioThread` adds is the command drain and the swap fade.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use assert_no_alloc::assert_no_alloc;
use cpu_time::ThreadTime;
use engine::backing::{Backing, Planar, Stems, Transport};
use engine::block::{BLOCK, RATE, Stereo};
use engine::command::{Event, event_ring};
use engine::driver::OfflineDriver;
use engine::graph::Graph;
use engine::ingest::{
    Codec, JitterBuffer, PacketProducer, PacketSource, SharedStats, SlotStats, packet_ring,
};
use engine::params::{CorrectorParams, Params, ReverbParams, SlotParams};
use engine::slot::Slot;
use engine::slot::guide::Guide;
use engine::slot::source::Source;

use crate::contour::Point;
use crate::rtp::{Arrival, PLAYOUT_DELAY};

/// Below the master clipper's 0.75 knee for any clip that peaks at full scale, so the clipper
/// stays out of a measurement.
const MEASURE_GAIN: f32 = 0.5;

/// A clip the slot plays once, then silence. It says it has no latency: the guide clock reads the
/// clip's own time.
struct ClipSource {
    samples: Vec<f32>,
    at: usize,
}

impl Source for ClipSource {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
            self.at += 1;
        }
    }
}

pub enum Input {
    Samples(Vec<f32>),
    /// Opus or L16 packets and the output frame each reaches the socket at, through the real
    /// packet ring and jitter buffer.
    Packets(Vec<Arrival>),
}

/// What runs through the chain.
pub struct Setup {
    pub input: Input,
    pub frames: usize,
    pub corrector: CorrectorParams,
    pub reverb: ReverbParams,
    pub guide: Option<Guide>,
    pub guide_offset_ms: f32,
    /// Slots beside the first, all the same voice: the CPU row's four singers.
    pub extra_voices: Vec<Vec<f32>>,
    /// Centre-panned at `mix_gain` over this backing, for a listening clip. `None` is the
    /// measuring setup: hard left over silence, so the left channel is the voice.
    pub mix: Option<Mix>,
}

pub struct Mix {
    pub backing: Option<Planar>,
    pub voice_gain: f32,
}

impl Setup {
    pub fn new(input: Input, frames: usize, corrector: CorrectorParams) -> Self {
        Self {
            input,
            frames,
            corrector,
            reverb: ReverbParams::default(),
            guide: None,
            guide_offset_ms: 0.0,
            extra_voices: Vec::new(),
            mix: None,
        }
    }
}

/// What came out.
pub struct Take {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// The slot's own pitch readings, on the guide's clock.
    pub readings: Vec<Point>,
    /// How far the corrector made the voice trail, in frames, once per block.
    pub delays: Vec<u32>,
    pub cpu_seconds: f64,
    pub stats: Option<SlotStats>,
}

impl Take {
    /// The voice, when the setup was the measuring one, at the level it went in.
    pub fn voice(&self) -> Vec<f32> {
        self.left.iter().map(|s| s / MEASURE_GAIN).collect()
    }
}

struct Ingest {
    producer: PacketProducer,
    arrivals: Vec<Arrival>,
    next: usize,
    stats: Arc<SharedStats>,
}

impl Ingest {
    /// Hands the ring every packet that reached the socket before this block, as the network
    /// thread would have.
    fn deliver(&mut self, pumped: i64) {
        while let Some(arrival) = self.arrivals.get(self.next).filter(|a| a.at < pumped) {
            assert!(self.producer.push(&arrival.packet), "the ring never fills here");
            self.next += 1;
        }
    }
}

pub fn run(setup: Setup) -> Take {
    let Setup { input, frames, corrector, reverb, guide, guide_offset_ms, extra_voices, mix } =
        setup;
    let (source, mut ingest): (Box<dyn Source>, Option<Ingest>) = match input {
        Input::Samples(samples) => (Box::new(ClipSource { samples, at: 0 }), None),
        Input::Packets(arrivals) => {
            let (producer, consumer) = packet_ring(engine::ingest::packet::RING_CAPACITY);
            let codec = Codec::opus().expect("the decoder could not be made");
            let jitter = JitterBuffer::new(PLAYOUT_DELAY, codec);
            let stats = jitter.shared_stats();
            let source = PacketSource::new(consumer, jitter);
            (Box::new(source), Some(Ingest { producer, arrivals, next: 0, stats }))
        }
    };

    let mut first = Slot::engaged(source);
    first.set_guide(guide.map(Box::new));
    let mut slots = vec![first];
    slots.extend(
        extra_voices
            .into_iter()
            .map(|samples| Slot::engaged(Box::new(ClipSource { samples, at: 0 }))),
    );
    let voices = slots.len();
    let mut graph = Graph::new(slots);

    // The guide clock is the backing's playhead, so the slot needs one even over silence
    let blocks = frames.div_ceil(BLOCK);
    let (mut backing, pan, gain) = match mix {
        Some(Mix { backing, voice_gain }) => (backing.unwrap_or_default(), 0.0, voice_gain),
        None => (Planar::default(), -1.0, MEASURE_GAIN),
    };
    backing.pad_to(blocks * BLOCK);
    let (mut events, mut drain) = event_ring();
    graph.set_backing(Backing::new(Stems::new(backing, None), 0));
    graph.steer(1, Transport::Start, &mut events);

    let slot_params = SlotParams {
        gain,
        pan,
        guide_offset_ms,
        effects: engine::params::EffectParams { corrector, reverb },
        ..SlotParams::default()
    };
    let mut params = Params::default();
    params.slots[..voices].fill(slot_params);

    let delay = Arc::new(AtomicU32::new(0));
    let published = Arc::clone(&delay);
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        graph.render(&params, out, &mut events);
        published.store(graph.slots()[0].corrector_latency_frames() as u32, Ordering::Relaxed);
    }));

    let mut take = Take {
        left: Vec::with_capacity(blocks * BLOCK),
        right: Vec::with_capacity(blocks * BLOCK),
        readings: Vec::with_capacity(frames / 480 + 1),
        delays: Vec::with_capacity(blocks),
        cpu_seconds: 0.0,
        stats: None,
    };
    let mut interleaved = [0.0f32; 2 * BLOCK];
    let started = ThreadTime::now();
    for block in 0..blocks {
        if let Some(ingest) = &mut ingest {
            ingest.deliver((block * BLOCK) as i64);
        }
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        let (stereo, _) = interleaved.as_chunks::<2>();
        take.left.extend(stereo.iter().map(|[left, _]| *left));
        take.right.extend(stereo.iter().map(|[_, right]| *right));
        take.delays.push(delay.load(Ordering::Relaxed));
        while let Some(event) = drain.pop() {
            if let Event::Pitch { slot: 0, position_ms, hz, confidence } = event {
                take.readings.push(Point { ms: position_ms, hz, confidence });
            }
        }
    }
    assert_eq!(drain.dropped(), 0, "the event ring is drained every block");
    // The whole loop, not each pump: reading a thread clock per block would cost more than a
    // tenth of what it measures, and copying the block out costs far less
    take.cpu_seconds = started.elapsed().as_secs_f64();
    take.stats = ingest.map(|ingest| ingest.stats.read());
    take
}

/// Frames in `ms` milliseconds, rounded.
pub fn frames_of_ms(ms: f64) -> u64 {
    (ms * f64::from(RATE) / 1000.0).round() as u64
}
