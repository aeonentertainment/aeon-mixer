//! The jitter buffer of docs/design.md 8.4. The first packet of a stream anchors its RTP timestamp
//! to the output clock plus the playout delay; from then on every packet has a due time, and it is
//! decoded in the block that time falls in. A packet missing when due is concealed, from the next
//! packet's in-band FEC when there is one. Until a later packet comes, a missing packet and a
//! sender's pause look the same, so the buffer conceals the time and keeps waiting for that
//! sequence number; RFC 3551 4.1 has a pause hold the sequence number while the timestamp runs on.
//! A packet arriving after its time is counted late and dropped, and the stream plays on. Clock
//! skew is corrected one frame at a time, by moving the anchor a frame and dropping or concealing
//! the frame that makes. A new SSRC, a gap over a second on either clock, or `reset` starts a new
//! stream.

use std::sync::Arc;

use super::codec::Codec;
use super::fifo::Fifo;
use super::opus::MAX_FRAME;
use super::packet::Packet;
use super::stats::{SharedStats, SlotStats};
use crate::block::RATE;

/// Packets held ahead of the decode cursor: 160 ms of 2.5 ms frames, 1.28 s of 20 ms ones.
pub const STORE: usize = 64;

/// A gap this long, on either clock, is a new stream.
pub const REANCHOR_GAP: i64 = RATE as i64;

/// The skew window: once a second the arrivals of the last second decide whether the sender's
/// clock has drifted a frame from ours.
const SKEW_WINDOW: i64 = RATE as i64;

/// Arrivals a window needs before it may move the anchor.
const SKEW_MIN_PACKETS: u32 = 8;

/// Decodes or concealments one pull may do. Two 120-frame packets cover a 128-frame block;
/// the rest is room for a gap fill, a timestamp jump, a pass over lost packets whose time is
/// already concealed, and the one overlap a skew drop discards.
const MAX_STEPS: usize = 8;

/// Decoded samples waiting to play: the longest packet plus a block, with room to spare.
const FIFO: usize = 8192;

/// The frame assumed before a stream's first decode: mediasoup's 20 ms.
const DEFAULT_FRAME: usize = 960;

pub struct JitterBuffer {
    codec: Codec,
    playout_delay: i64,
    store: Vec<Option<Packet>>,
    fifo: Fifo,
    scratch: Vec<f32>,
    stream: Option<Stream>,
    /// Whether a stream has ever been anchored, so the next anchor counts as a resync.
    anchored: bool,
    /// Output frames pulled since the buffer was made: the clock every due time is on.
    now: i64,
    stats: SlotStats,
    shared: Arc<SharedStats>,
}

struct Stream {
    ssrc: u32,
    anchor_ts: u32,
    /// The output frame `anchor_ts` plays at. It moves only by a skew correction, one frame.
    anchor_out: i64,
    next_seq: u16,
    next_ts: u32,
    /// The output frame after the last sample in the fifo.
    fifo_end: i64,
    /// Frames in the last packet decoded: the unit a concealment or a skew correction takes.
    frame: usize,
    last_arrival: i64,
    skew: SkewWindow,
    /// RFC 3550 section 6.4.1: the last packet's transit time and the smoothed jitter.
    transit: Option<i64>,
    jitter: f32,
}

/// How early packets arrived over the last second: the slack between arrival and due time, at
/// its least and its most.
struct SkewWindow {
    ends_at: i64,
    packets: u32,
    min_slack: i64,
    max_slack: i64,
}

impl JitterBuffer {
    /// Built on the control thread, with the slot's decoder. Everything a block needs is
    /// allocated here.
    pub fn new(playout_delay_frames: u32, codec: Codec) -> Self {
        Self {
            codec,
            playout_delay: i64::from(playout_delay_frames),
            store: vec![None; STORE],
            fifo: Fifo::new(FIFO),
            scratch: vec![0.0; MAX_FRAME],
            stream: None,
            anchored: false,
            now: 0,
            stats: SlotStats::default(),
            shared: Arc::new(SharedStats::default()),
        }
    }

    /// The control thread's view of the counters.
    pub fn shared_stats(&self) -> Arc<SharedStats> {
        Arc::clone(&self.shared)
    }

    pub fn stats(&self) -> SlotStats {
        self.stats
    }

    /// How long ago the singer sang what `pull` hands over, as far as this end can know: the
    /// sender gathers a frame before sending it, and the anchor plays it a playout delay after it
    /// arrives. The Wi-Fi and the phone's own capture are the host's `guideOffsetMs`.
    pub fn latency_frames(&self) -> u32 {
        let frame = self.stream.as_ref().map_or(DEFAULT_FRAME, |stream| stream.frame);
        u32::try_from(self.playout_delay + frame as i64).unwrap_or(u32::MAX)
    }

    /// `ResetSlot`: forget the stream. The next packet is a new singer and re-anchors.
    pub fn reset(&mut self) {
        self.stream = None;
        self.fifo.clear();
    }

    /// Takes one packet from the ring, on the audio thread.
    pub fn push(&mut self, packet: &Packet) {
        self.stats.pushed += 1;
        let Some(stream) = &mut self.stream else { return self.anchor(packet) };
        if packet.ssrc != stream.ssrc || self.now - stream.last_arrival > REANCHOR_GAP {
            return self.anchor(packet);
        }
        let ahead = packet.seq.wrapping_sub(stream.next_seq) as i16;
        let due = stream.due(packet.timestamp);
        if ahead >= 0 && (due - stream.fifo_end).abs() > REANCHOR_GAP {
            return self.anchor(packet);
        }
        // Behind the cursor in sequence, or in time: its frame was concealed while it was awaited
        if ahead < 0 || (packet.timestamp.wrapping_sub(stream.next_ts) as i32) < 0 {
            self.stats.late += 1;
            return;
        }
        if ahead as usize >= STORE {
            // Too far ahead to hold; it is counted lost as the cursor passes it
            return;
        }
        stream.observe(self.now, due);
        self.store[usize::from(packet.seq) % STORE] = Some(*packet);
    }

    /// One block, on the audio thread, after the ring is drained.
    pub fn pull(&mut self, out: &mut [f32]) {
        let now = self.now;
        let end = now + out.len() as i64;
        if let Some(stream) = &self.stream
            && now - stream.last_arrival > REANCHOR_GAP
        {
            // Nothing for a second: stop concealing and wait for a new stream
            self.stream = None;
            self.fifo.clear();
        }
        if let Some(stream) = &mut self.stream {
            stream.correct_skew(now, self.playout_delay, &mut self.stats);
            fill(
                stream,
                &mut self.codec,
                &mut self.store,
                &mut self.fifo,
                &mut self.scratch,
                &mut self.stats,
                end,
            );
        }

        let head = match &self.stream {
            Some(stream) => stream.fifo_end - self.fifo.len() as i64,
            None => i64::MAX,
        };
        if head < now {
            // Left over from an underflow: those samples' time has passed
            self.fifo.discard((now - head) as usize);
        }
        let first_due = head.max(now);
        for (i, sample) in out.iter_mut().enumerate() {
            *sample =
                if now + i as i64 >= first_due { self.fifo.pop().unwrap_or(0.0) } else { 0.0 };
        }

        self.now = end;
        self.stats.jitter_ms =
            self.stream.as_ref().map_or(0.0, |stream| stream.jitter * 1000.0 / RATE as f32);
        self.shared.publish(&self.stats);
    }

    fn anchor(&mut self, packet: &Packet) {
        if self.anchored {
            self.stats.resyncs += 1;
        }
        self.anchored = true;
        self.fifo.clear();
        let plays_at = self.now + self.playout_delay;
        let mut stream = Stream {
            ssrc: packet.ssrc,
            anchor_ts: packet.timestamp,
            anchor_out: plays_at,
            next_seq: packet.seq,
            next_ts: packet.timestamp,
            fifo_end: plays_at,
            frame: DEFAULT_FRAME,
            last_arrival: self.now,
            skew: SkewWindow::new(self.now + SKEW_WINDOW),
            transit: None,
            jitter: 0.0,
        };
        stream.observe(self.now, plays_at);
        self.store[usize::from(packet.seq) % STORE] = Some(*packet);
        self.stream = Some(stream);
    }
}

/// Decodes or conceals, in sequence order, until the fifo reaches `end` or the step bound.
fn fill(
    stream: &mut Stream,
    codec: &mut Codec,
    store: &mut [Option<Packet>],
    fifo: &mut Fifo,
    scratch: &mut [f32],
    stats: &mut SlotStats,
    end: i64,
) {
    for _ in 0..MAX_STEPS {
        if stream.fifo_end >= end {
            return;
        }
        let due = stream.due(stream.next_ts);
        if due > stream.fifo_end {
            let gap = (due - stream.fifo_end) as usize;
            if gap <= stream.frame {
                // A skew correction delayed the stream by a frame: conceal it
                codec.conceal(None, &mut scratch[..gap]);
                stream.fifo_end += fifo.push(&scratch[..gap]) as i64;
                stats.concealed += 1;
            } else {
                // The sender skipped ahead, a pause under a second: silence until it is due
                let needed = (end - stream.fifo_end) as usize;
                stream.fifo_end += fifo.push_silence(gap.min(needed)) as i64;
            }
            continue;
        }

        // The store is keyed by sequence number alone, so a packet is only the stream's when its
        // SSRC says so: an old stream's leftovers never decode into a new one
        let at = usize::from(stream.next_seq) % STORE;
        let present = store[at].as_ref().is_some_and(|packet| stream.owns(packet, stream.next_seq));
        let (frames, consumed) = if present {
            let packet = store[at].as_ref().expect("checked present");
            if packet.timestamp != stream.next_ts {
                stream.next_ts = packet.timestamp;
                continue;
            }
            let decoded = codec.decode(packet.payload(), scratch);
            store[at] = None;
            if decoded == 0 {
                codec.conceal(None, &mut scratch[..stream.frame]);
                stats.concealed += 1;
                (stream.frame, true)
            } else {
                stream.frame = decoded;
                (decoded, true)
            }
        } else if let Some((distance, next)) = stream.next_in_hand(store) {
            // A later packet came first, so the ones before it are lost. Those whose time was
            // concealed while they were awaited are passed over together, in one step
            let waited = i64::from(next.timestamp.wrapping_sub(stream.next_ts) as i32);
            let frame = stream.frame as i64;
            let needed = if waited > 0 { (waited + frame - 1) / frame } else { 0 };
            let covered = (i64::from(distance) - needed).max(0) as u16;
            if covered > 0 {
                stats.lost += u32::from(covered);
                stream.next_seq = stream.next_seq.wrapping_add(covered);
                continue;
            }
            stats.lost += 1;
            let uncovered = (waited - i64::from(distance - 1) * frame) as usize;
            let fec = distance == 1 && uncovered <= MAX_FRAME;
            let frames = if fec { uncovered } else { uncovered.min(stream.frame) };
            codec.conceal(fec.then(|| next.payload()), &mut scratch[..frames]);
            stats.concealed += 1;
            (frames, true)
        } else {
            // Nothing in hand: a pause, or a loss not yet known as one. The time is concealed and
            // the sequence number still awaited, so a sender resuming after a pause plays on time
            codec.conceal(None, &mut scratch[..stream.frame]);
            stats.concealed += 1;
            (stream.frame, false)
        };

        // Overlap means a skew correction pulled the stream a frame earlier: that frame goes
        let overlap = ((stream.fifo_end - due) as usize).min(frames);
        fifo.push(&scratch[overlap..frames]);
        stream.fifo_end = stream.fifo_end.max(due + frames as i64);
        if consumed {
            stream.next_seq = stream.next_seq.wrapping_add(1);
        }
        stream.next_ts = stream.next_ts.wrapping_add(frames as u32);
    }
}

impl Stream {
    fn owns(&self, packet: &Packet, seq: u16) -> bool {
        packet.ssrc == self.ssrc && packet.seq == seq
    }

    /// The nearest packet of this stream held past the cursor, and how many sequence numbers on.
    fn next_in_hand<'a>(&self, store: &'a [Option<Packet>]) -> Option<(u16, &'a Packet)> {
        (1..STORE as u16).find_map(|distance| {
            let seq = self.next_seq.wrapping_add(distance);
            let packet = store[usize::from(seq) % STORE].as_ref()?;
            self.owns(packet, seq).then_some((distance, packet))
        })
    }

    /// The output frame a timestamp plays at, under the anchor.
    fn due(&self, timestamp: u32) -> i64 {
        self.anchor_out + i64::from(timestamp.wrapping_sub(self.anchor_ts) as i32)
    }

    fn observe(&mut self, now: i64, due: i64) {
        self.last_arrival = now;
        let slack = due - now;
        self.skew.packets += 1;
        self.skew.min_slack = self.skew.min_slack.min(slack);
        self.skew.max_slack = self.skew.max_slack.max(slack);
        if let Some(last) = self.transit {
            let difference = (-slack - last).abs() as f32;
            self.jitter += (difference - self.jitter) / 16.0;
        }
        self.transit = Some(-slack);
    }

    /// Once a second: when every packet of the last second came a frame early, the sender's
    /// clock runs fast and the anchor moves a frame earlier, dropping one; when every packet
    /// came a frame late, or half the delay when a frame is longer, it runs slow and the anchor
    /// moves a frame later, concealing one.
    fn correct_skew(&mut self, now: i64, playout_delay: i64, stats: &mut SlotStats) {
        if now < self.skew.ends_at {
            return;
        }
        let window = std::mem::replace(&mut self.skew, SkewWindow::new(now + SKEW_WINDOW));
        if window.packets < SKEW_MIN_PACKETS {
            return;
        }
        let frame = self.frame as i64;
        if window.min_slack >= playout_delay + frame {
            self.anchor_out -= frame;
            stats.resyncs += 1;
        } else if window.max_slack <= playout_delay - frame.min(playout_delay / 2) {
            self.anchor_out += frame;
            stats.resyncs += 1;
        }
    }
}

impl SkewWindow {
    fn new(ends_at: i64) -> Self {
        Self { ends_at, packets: 0, min_slack: i64::MAX, max_slack: i64::MIN }
    }
}

#[cfg(test)]
mod tests {
    use super::super::codec::Codec;
    use super::super::fixtures::*;
    use super::*;
    use crate::block::BLOCK;

    const SECOND: i64 = RATE as i64;

    /// What the guide clock subtracts is the delay a sung sample really has here: the measured
    /// lag from source to output, less the sender's encoder lookahead, which is the phone's, to
    /// within the block a packet waits to be drained.
    #[test]
    fn latency_frames_is_the_delay_a_sung_sample_has() {
        let signal = source(2 * RATE as usize);
        for frame in [120usize, 240, 480, 960] {
            let mut sender = OpusSender::new(frame, 0xBEEF);
            let arrivals = sender.send_all(&signal, 0);
            let mut jitter = JitterBuffer::new(DELAY, Codec::opus().unwrap());
            assert_eq!(jitter.latency_frames(), DELAY + DEFAULT_FRAME as u32, "before a stream");
            let out = pull_direct(&mut jitter, &arrivals, signal.len() as i64 + SECOND / 10);
            let expected = i64::from(DELAY) + frame as i64 + sender.lookahead as i64;
            let fit = compare(&signal, frame, &out, expected, BLOCK as i64);
            let measured = fit.lag - sender.lookahead as i64;
            let reported = i64::from(jitter.latency_frames());
            assert!(
                (0..BLOCK as i64).contains(&(measured - reported)),
                "{frame}-frame packets: measured {measured}, reported {reported}"
            );
        }
    }

    /// Thirty seconds from a sender a tenth of a percent fast or slow, which drifts six 5 ms
    /// frames. Each window that sees a whole frame of drift moves the anchor one frame, so the
    /// delay the singer hears stays within a frame of the playout delay, and nothing goes late.
    #[test]
    fn clock_skew_is_corrected_a_frame_at_a_time() {
        let signal = quantized(&source(30 * RATE as usize));
        for (rate, direction) in [(1.001f64, -1i64), (0.999, 1)] {
            let mut sender = L16Sender::new(240, 21);
            let arrivals = impair(&sender.send_all(&signal, 0, rate), Impairment::NONE, 8);
            let mut jitter = JitterBuffer::new(DELAY, Codec::L16);
            let out = pull_direct(&mut jitter, &arrivals, 31 * SECOND);
            let anchor = anchor_of(&arrivals);
            let start = compare(&signal[..RATE as usize], 0, &out, anchor, 0);
            let last = 29 * RATE as usize;
            let end = compare(&signal[last..], 0, &out, anchor + last as i64, 2000);
            assert!(start.error < 1e-6 && end.error < 1e-6, "{rate}: {start:?} {end:?}");
            let drift = (29.0 * RATE as f64 * (1.0 / rate - 1.0)).round() as i64;
            let moved = end.lag - last as i64 - start.lag;
            assert!(
                (moved - drift).abs() <= 240,
                "{rate}: the stream moved {moved} for a drift of {drift}"
            );
            assert_eq!(moved.signum(), direction);
            let stats = jitter.stats();
            assert!((5..=7).contains(&stats.resyncs), "{rate}: {stats:?}");
            // The second of concealment after the stream ends is a pause until a later packet
            // says otherwise, so a clean link loses nothing
            assert_eq!((stats.late, stats.lost), (0, 0), "{rate}: {stats:?}");
        }
    }

    /// Packets stop for over a second: concealment stops too, and the next packet is a new
    /// stream at the playout delay, however far its timestamp jumped.
    #[test]
    fn silence_over_a_second_ends_the_stream_and_the_next_packet_begins_another() {
        let signal = quantized(&source(RATE as usize));
        let mut sender = L16Sender::new(240, 22);
        let mut arrivals = sender.send_all(&signal, 0, 1.0);
        sender.pause(90 * RATE);
        arrivals.extend(sender.send_all(&signal, 3 * SECOND, 1.0));
        let arrivals = impair(&arrivals, Impairment::NONE, 9);
        let mut jitter = JitterBuffer::new(DELAY, Codec::L16);
        let out = pull_direct(&mut jitter, &arrivals, 4 * SECOND + 4800);
        let gap = &out[(2 * SECOND + 4800) as usize..3 * SECOND as usize];
        assert!(gap.iter().all(|s| *s == 0.0), "silent while nothing comes");
        let resumed = anchor_of(&arrivals[200..]);
        assert!(compare(&signal, 0, &out, resumed, 0).error < 1e-6);
        let stats = jitter.stats();
        assert_eq!(stats.resyncs, 1, "{stats:?}");
        assert!(stats.concealed < 400, "concealment stopped after a second: {stats:?}");
    }

    /// The old stream's packets still in the store share sequence numbers with the new one, and
    /// none of them is heard.
    #[test]
    fn a_new_stream_never_decodes_the_old_streams_leftovers() {
        let signal = quantized(&source(RATE as usize));
        let mut old = L16Sender::new(240, 30);
        let mut new = L16Sender::new(240, 31);
        let mut arrivals = old.send_all(&signal, 0, 1.0);
        // The new stream begins where the old one's last 15 ms are still in hand, at the
        // sequence numbers those packets carry
        new.seq = old.seq.wrapping_sub(8);
        arrivals.extend(new.send_all(&signal, SECOND, 1.0));
        let arrivals = impair(&arrivals, Impairment::NONE, 11);
        let mut jitter = JitterBuffer::new(DELAY, Codec::L16);
        let out = pull_direct(&mut jitter, &arrivals, 2 * SECOND + 4800);
        let fit = compare(&signal, 0, &out, anchor_of(&arrivals[200..]), 0);
        assert!(fit.error < 1e-6, "{fit:?}");
        assert_eq!(jitter.stats().resyncs, 1);
    }

    #[test]
    fn jitter_is_measured_and_nothing_is_reported_before_a_stream() {
        let mut jitter = JitterBuffer::new(DELAY, Codec::L16);
        let shared = jitter.shared_stats();
        let out = pull_direct(&mut jitter, &[], 4 * BLOCK as i64);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(shared.read(), SlotStats::default());

        let signal = quantized(&source(RATE as usize));
        let mut sender = L16Sender::new(240, 23);
        let arrivals = impair(&sender.send_all(&signal, 0, 1.0), Impairment::WIFI, 10);
        pull_direct(&mut jitter, &arrivals, SECOND + 4800);
        let stats = shared.read();
        assert!(stats.jitter_ms > 1.0 && stats.jitter_ms < 10.0, "{stats:?}");
        assert_eq!(stats.pushed, arrivals.len() as u32);
    }
}
