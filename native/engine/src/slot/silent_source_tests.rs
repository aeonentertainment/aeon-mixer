//! A slot's source goes silent while the backing plays on, through each kind of source and a
//! whole graph on the offline driver, closed loop: the room hears the master as the device played
//! it, and the mic hears the room until its source stops handing it over.

use assert_no_alloc::{assert_no_alloc, permit_alloc};
use kernels::fixtures::{at_db, db, pink_noise, rms, room, talker};
use rtrb::RingBuffer;

use super::source::fixtures::Tapped;
use super::*;
use crate::backing::{Backing, Planar, Stems, Transport};
use crate::command::event_ring;
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::ingest::capture_voice;
use crate::ingest::fixtures::{DELAY, OpusSender};
use crate::ingest::packet::{PacketProducer, RING_CAPACITY, packet_ring};
use crate::ingest::push::{DEFAULT_CAPACITY_FRAMES, PushProducer, push_voice};
use crate::ingest::{CaptureProducer, Codec, JitterBuffer, PacketSource};
use crate::params::{EchoCancelParams, params_channel};

const SECOND: usize = RATE as usize;
/// The phone's packets: 2.5 ms, as the phone's native module sends them.
const PACKET: usize = 120;
/// The source is silent over `[SILENT_FROM, SILENT_TO)` and the run ends at `END`.
const SILENT_FROM: usize = 6 * SECOND;
const SILENT_TO: usize = 9 * SECOND;
const END: usize = 12 * SECOND;

/// How the source goes silent.
#[derive(Clone, Copy, Debug)]
enum Silence {
    /// A phone stops sending, so its jitter buffer conceals for a second and then plays nothing.
    RtpStops,
    /// A house mic's input is lost, so its capture ring runs dry.
    CaptureLost,
    /// The host stops pushing, so its push ring runs dry.
    PushStops,
    /// A phone mutes its mic and sends on: Opus packets of silence.
    MutedAtMic,
}

/// The far end of the slot's source: the phone, the input or the host, handed what the mic hears
/// a block at a time.
enum Feed {
    Rtp { sender: OpusSender, pending: Vec<f32>, packets: PacketProducer },
    Capture(CaptureProducer),
    Push(PushProducer),
}

impl Feed {
    /// The source for `silence` and the feed that fills it.
    fn for_silence(silence: Silence) -> (Self, Box<dyn Source>) {
        match silence {
            Silence::RtpStops | Silence::MutedAtMic => {
                let (packets, consumer) = packet_ring(RING_CAPACITY);
                let jitter = JitterBuffer::new(DELAY, Codec::opus().expect("a decoder"));
                let sender = OpusSender::new(PACKET, 0x5107);
                let feed = Self::Rtp { sender, pending: Vec::with_capacity(SECOND), packets };
                (feed, Box::new(PacketSource::new(consumer, jitter)))
            }
            Silence::CaptureLost => {
                let (producer, voice) = capture_voice();
                let voice = voice.for_input(BLOCK as u32, 0, BLOCK as u32).expect("it takes these");
                (Self::Capture(producer), Box::new(voice))
            }
            Silence::PushStops => {
                let (producer, voice) = push_voice(DEFAULT_CAPACITY_FRAMES);
                (Self::Push(producer), Box::new(voice))
            }
        }
    }

    /// One block of what the mic heard; `silent` while the source has gone silent.
    fn hand(&mut self, heard: &[f32], silence: Silence, silent: bool) {
        match self {
            Self::Rtp { sender, pending, packets } => {
                let muted = silent && matches!(silence, Silence::MutedAtMic);
                pending.extend(heard.iter().map(|s| if muted { 0.0 } else { *s }));
                while pending.len() >= PACKET {
                    let pcm: Vec<f32> = pending.drain(..PACKET).collect();
                    if silent && !muted {
                        sender.pause(PACKET as u32);
                    } else if let Some(packet) = sender.packet(&pcm) {
                        packets.push(&packet);
                    }
                }
            }
            Self::Capture(producer) if !silent => {
                producer.push(heard);
            }
            Self::Push(producer) if !silent => {
                let stereo: Vec<f32> = heard.iter().flat_map(|s| [*s, *s]).collect();
                producer.push(&stereo);
            }
            Self::Capture(_) | Self::Push(_) => {}
        }
    }
}

/// What the source handed the slot, and what the slot played, with the canceller on.
struct Run {
    handed: Vec<f32>,
    out: Vec<f32>,
}

/// A house mic's living room, 10 ms and 10 dB down with a 300 ms tail, hearing a karaoke room's
/// backing until the source goes silent by `silence` for three seconds. The backing plays hard
/// right and the slot hard left, so the master's left is the slot alone.
fn run(silence: Silence) -> Run {
    let far: Vec<f32> = at_db(talker(END, 110.0, 3), -18.0)
        .iter()
        .zip(&at_db(pink_noise(END, 7), -26.0))
        .map(|(v, b)| v + b)
        .collect();
    let path = room(480, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(SECOND, 99), -60.0);
    let (mut feed, source) = Feed::for_silence(silence);
    let (handed, mut handed_out) = RingBuffer::new(END + BLOCK);
    let mut graph = Graph::new(vec![Slot::new(Box::new(Tapped { inner: source, handed }))]);
    let (mut events, _drain) = event_ring();
    let silent = vec![0.0; far.len()];
    graph.set_backing(Backing::new(Stems::new(Planar { left: silent, right: far }, None), 0));
    graph.steer(1, Transport::Start, &mut events);
    let (mut writer, mut reader) = params_channel();
    writer.update(|p| {
        p.slots[0].pan = -1.0;
        p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true };
    });
    let mut played: Vec<f32> = Vec::with_capacity(END + BLOCK);
    let mut heard = [0.0; BLOCK];
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        // The phone, the input and the host are other threads, which may allocate
        permit_alloc(|| {
            let at = played.len();
            for (k, sample) in heard.iter_mut().enumerate() {
                let t = at + k;
                let echo: f32 = path
                    .iter()
                    .filter_map(|&(lag, gain)| Some(gain * played.get(t.checked_sub(lag)?)?))
                    .sum();
                *sample = echo + floor[t % floor.len()];
            }
            feed.hand(&heard, silence, (SILENT_FROM..SILENT_TO).contains(&at));
        });
        let Stereo { left, right } = out;
        graph.render(reader.latest(), Stereo { left: &mut *left, right: &mut *right }, &mut events);
        permit_alloc(|| played.extend(left.iter().zip(right.iter()).map(|(l, r)| 0.5 * (l + r))));
    }));
    let mut out = Vec::with_capacity(END);
    let mut interleaved = [0.0; 2 * BLOCK];
    while out.len() < END {
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        out.extend(interleaved.iter().step_by(2));
    }
    let handed = std::iter::from_fn(|| handed_out.pop().ok()).collect();
    Run { handed, out }
}

fn level(signal: &[f32], from: f64, to: f64) -> f32 {
    db(rms(&signal[(from * SECOND as f64) as usize..(to * SECOND as f64) as usize]))
}

/// The echo taken out over `[from, to)` seconds.
fn removed(run: &Run, from: f64, to: f64) -> f32 {
    level(&run.handed, from, to) - level(&run.out, from, to)
}

// A slot with the canceller on learns the room for six seconds, then its source goes silent for
// three while the master plays on, then comes back. While it is silent the slot plays no louder
// than its source hands over, as with the canceller off, within 0.5 dB; and from a second after
// it comes back the canceller takes out within 3 dB of what it took out in the second before.
// Subtracting its estimate from the silence, it played the echo inverted, up to 12 dB louder than
// its source, then took out 0.6 dB at most once the source was back.
#[test]
fn a_source_gone_silent_plays_silence_and_has_its_echo_cancelled_when_it_comes_back() {
    let silences =
        [Silence::RtpStops, Silence::CaptureLost, Silence::PushStops, Silence::MutedAtMic];
    let rows: Vec<_> = silences
        .into_iter()
        .map(|silence| {
            let run = run(silence);
            let before = removed(&run, 5.0, 6.0);
            let (handed, out) = (level(&run.handed, 6.0, 9.0), level(&run.out, 6.0, 9.0));
            let back = removed(&run, 10.0, 11.0);
            println!(
                "{silence:?}: {before:.1} dB out before; silent, the source {handed:.1} dBFS and \
                 the slot {out:.1}; {back:.1} dB out from a second after it came back"
            );
            (silence, before, handed, out, back)
        })
        .collect();
    for (silence, before, handed, out, back) in rows {
        assert!(out <= handed + 0.5, "{silence:?}: {out:.1} dBFS out of {handed:.1} handed");
        assert!(back >= before - 3.0, "{silence:?}: {back:.1} dB out after, {before:.1} before");
    }
}
