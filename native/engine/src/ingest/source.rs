//! The source of an `rtp` or `l16` slot: at the top of each block it drains the slot's packet
//! ring into the jitter buffer, then pulls the block that is due. Its first pull discards what
//! the ring already holds: a mixer is armed, and its ports handed to the phones, before it
//! starts, and a stream anchored on that backlog would play behind real time for good. A reset
//! does the same, since what waits in the ring belongs to the stream it forgets, and after a move
//! to another output it is the whole gap.

use kernels::SourceDelay;

use super::jitter::JitterBuffer;
use super::packet::PacketConsumer;
use crate::slot::source::Source;

pub struct PacketSource {
    packets: PacketConsumer,
    jitter: JitterBuffer,
    pulled: bool,
}

impl PacketSource {
    pub fn new(packets: PacketConsumer, jitter: JitterBuffer) -> Self {
        Self { packets, jitter, pulled: false }
    }

    pub fn jitter(&self) -> &JitterBuffer {
        &self.jitter
    }
}

impl Source for PacketSource {
    fn pull(&mut self, out: &mut [f32]) {
        let Self { packets, jitter, pulled } = self;
        if !*pulled {
            *pulled = true;
            while packets.pop_with(|_| {}) {}
        }
        while packets.pop_with(|packet| jitter.push(packet)) {}
        jitter.pull(out);
    }

    fn reset(&mut self) {
        self.jitter.reset();
        self.pulled = false;
    }

    fn latency_frames(&self) -> u32 {
        self.jitter.latency_frames()
    }

    /// A skew correction drops or conceals a frame, moving the echo with it, and the latency
    /// holds, so the jitter buffer reports it as a splice. A new stream starts at the opening
    /// delay, wherever the last had drifted to.
    fn delay(&self) -> SourceDelay {
        SourceDelay {
            latency_frames: self.jitter.latency_frames(),
            spliced_frames: self.jitter.spliced_frames(),
            streams: self.jitter.streams(),
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::super::codec::Codec;
    use super::super::fixtures::*;
    use super::super::jitter::JitterBuffer;
    use super::super::opus::{MAX_FRAME, OpusDecoder};
    use super::super::packet::{Packet, RING_CAPACITY, packet_ring};
    use super::PacketSource;
    use crate::block::{BLOCK, RATE};
    use crate::slot::meter::{FLOOR_DB, WINDOW_FRAMES};
    use crate::slot::source::Source;

    const SECOND: i64 = RATE as i64;

    /// Packets that arrived while the mixer was armed never reach the jitter buffer; the first
    /// packet after the slot starts pulling is the one that anchors the stream.
    #[test]
    fn the_first_pull_discards_what_arrived_before_the_slot_went_live() {
        let (mut producer, consumer) = packet_ring(RING_CAPACITY);
        let mut source = PacketSource::new(consumer, JitterBuffer::new(DELAY, Codec::L16));
        let stats = source.jitter().shared_stats();
        let payload = [0u8; 4 * 240];
        for seq in 0..RING_CAPACITY as u16 + 8 {
            producer.push(&Packet::new(seq, u32::from(seq) * 240, 7, false, &payload).unwrap());
        }
        let mut out = [0.0; BLOCK];
        assert_no_alloc(|| source.pull(&mut out));
        assert_eq!(stats.read().pushed, 0, "the backlog is dropped unseen");
        producer.push(&Packet::new(100, 100 * 240, 7, false, &payload).unwrap());
        assert_no_alloc(|| source.pull(&mut out));
        assert_eq!(stats.read().pushed, 1);
        assert_eq!(source.latency_frames(), DELAY + 960, "anchored on the fresh packet");
    }

    /// A move to another output stops the clock while packets keep coming. The reset that follows
    /// drops what waited, so the stream anchors on the next packet and plays at its delay.
    #[test]
    fn a_reset_drops_what_waited_in_the_ring() {
        let (mut producer, consumer) = packet_ring(RING_CAPACITY);
        let mut source = PacketSource::new(consumer, JitterBuffer::new(DELAY, Codec::L16));
        let stats = source.jitter().shared_stats();
        let payload = [0u8; 4 * 240];
        let packet = |seq: u16| Packet::new(seq, u32::from(seq) * 240, 7, false, &payload).unwrap();
        let mut out = [0.0; BLOCK];
        assert_no_alloc(|| source.pull(&mut out));
        producer.push(&packet(0));
        assert_no_alloc(|| source.pull(&mut out));
        assert_eq!(stats.read().pushed, 1, "the stream is playing");

        for seq in 1..40 {
            producer.push(&packet(seq));
        }
        assert_no_alloc(|| source.reset());
        assert_no_alloc(|| source.pull(&mut out));
        assert_eq!(stats.read().pushed, 1, "the gap's packets are dropped unseen");
        producer.push(&packet(40));
        assert_no_alloc(|| source.pull(&mut out));
        assert_eq!(stats.read().pushed, 2);
        assert_eq!(source.latency_frames(), DELAY + 960, "anchored on the packet after the gap");
    }

    /// 8.4's done-when: reordering, 30 ms spikes and 5 percent loss on every frame size. The
    /// concealment's expected error is the clean codec's error plus the signal's own energy over
    /// the frames concealed or dropped late: concealment never does worse than a frame's worth of
    /// signal. The stream is never reset by any of it, and plays at the same lag as a clean one.
    #[test]
    fn impaired_rtp_matches_the_source_within_the_concealments_expected_error() {
        let signal = source(3 * RATE as usize);
        for frame in [120usize, 240, 480, 960] {
            let fit_of = |impairment| {
                let mut sender = OpusSender::new(frame, 0xABCD);
                let sent = sender.send_all(&signal, 0);
                let arrivals = impair(&sent, impairment, 7);
                let mut rig = Rig::new(Codec::opus().unwrap());
                rig.run(&arrivals, anchor_of(&arrivals) + signal.len() as i64);
                let expected = anchor_of(&arrivals) + sender.lookahead as i64;
                let fit = compare(&signal, frame, &rig.out, expected, BLOCK as i64);
                assert!(
                    (fit.lag - expected).abs() <= 10,
                    "{frame}: lag {} for {expected}",
                    fit.lag
                );
                assert!(fit.snr_db() > 7.0, "{frame}: {fit:?}");
                (fit, rig.stats(), sent.len() as f32)
            };
            let (clean, clean_stats, _) = fit_of(Impairment::NONE);
            let (wifi, stats, sent) = fit_of(Impairment::WIFI);
            assert_eq!(clean_stats.lost, 0, "{frame}: a clean stream loses nothing");
            assert!(
                (wifi.lag - clean.lag).abs() <= 2,
                "{frame}: impairment does not move the stream"
            );
            assert_eq!(stats.resyncs, 0, "{frame}: nothing reset it");
            assert!(stats.late > 0 && stats.lost > 0, "{frame}: {stats:?}");
            let impaired = (stats.concealed + stats.late) as f32 / sent;
            assert!(
                wifi.error <= clean.error + impaired,
                "{frame}: error {:.4} over the clean {:.4} plus {:.3} impaired",
                wifi.error,
                clean.error,
                impaired
            );
        }
    }

    /// A packet 40 ms late on 20 ms frames: its frame is concealed, the decoder settles over
    /// the two after it, and nothing else changes. Raw PCM shows it exactly: one frame of
    /// silence, every other sample identical.
    #[test]
    fn a_late_packet_costs_at_most_the_frame_it_missed() {
        let signal = source(2 * RATE as usize);
        let frame = 960;
        let mut sender = OpusSender::new(frame, 5);
        let sent = sender.send_all(&signal, 0);
        let clean = impair(&sent, Impairment::NONE, 1);
        let mut late = clean.clone();
        late[50].at += 1920;
        late.sort_by_key(|a| a.at);
        let until = anchor_of(&clean) + signal.len() as i64;
        let mut reference = Rig::new(Codec::opus().unwrap());
        reference.run(&clean, until);
        let mut rig = Rig::new(Codec::opus().unwrap());
        rig.run(&late, until);
        let stats = rig.stats();
        assert_eq!((stats.late, stats.lost, stats.resyncs), (1, 1, 0), "{stats:?}");
        let errors = rig.frame_errors(&reference.out, anchor_of(&clean), frame, 90);
        for (k, error) in errors.iter().enumerate() {
            let touched = (50..53).contains(&k);
            assert!(touched || *error < 0.001, "frame {k} differs by {error:.4}");
        }
        assert!(errors[50] > 0.5, "the missed frame is concealed, not the real thing");

        let mut sender = L16Sender::new(240, 6);
        let sent = sender.send_all(&signal, 0, 1.0);
        let clean = impair(&sent, Impairment::NONE, 1);
        let mut late = clean.clone();
        late[100].at += 960;
        late.sort_by_key(|a| a.at);
        let mut reference = Rig::new(Codec::L16);
        reference.run(&clean, until);
        let mut rig = Rig::new(Codec::L16);
        rig.run(&late, until);
        let due = (anchor_of(&clean) + 100 * 240) as usize;
        assert!(rig.out[due..due + 240].iter().all(|s| *s == 0.0), "the missed frame is silence");
        assert_eq!(rig.out[..due], reference.out[..due]);
        assert_eq!(rig.out[due + 240..], reference.out[due + 240..]);
    }

    /// 10 ms frames with 15 ms of delay: the packet after a lost one is in hand when the lost
    /// one falls due, so its in-band FEC fills the gap. Over ten lost frames across the voice
    /// bursts it beats the decoder's concealment on most and by a wide margin in all; a single
    /// frame can go either way, since concealment of a steady voiced sound is good too, and
    /// libopus's numbers shift with the CPU's SIMD path. With 20 ms frames the next packet is
    /// 5 ms too late for any of this, and concealment is all there is. At 32 kb/s the encoder
    /// is in SILK or hybrid mode, the modes that carry FEC.
    #[test]
    fn in_band_fec_from_the_next_packet_beats_concealment() {
        let signal = voice(2 * RATE as usize);
        let frame = 480;
        let mut sender = OpusSender::at_bitrate(frame, 9, 32_000);
        let sent = sender.send_all(&signal, 0);
        let clean = impair(&sent, Impairment::NONE, 3);
        let until = anchor_of(&clean) + signal.len() as i64;
        let mut reference = Rig::new(Codec::opus().unwrap());
        reference.run(&clean, until);
        let error_at = |arrivals: &[Arrival], k: usize| {
            let mut rig = Rig::new(Codec::opus().unwrap());
            rig.run(arrivals, until);
            assert_eq!(rig.stats().lost, 1);
            rig.frame_errors(&reference.out, anchor_of(&clean), frame, 199)[k]
        };
        let (mut fec_total, mut plc_total, mut fec_wins) = (0.0, 0.0, 0);
        for k in [15usize, 25, 50, 55, 70, 100, 110, 150, 160, 190] {
            let mut lost = clean.clone();
            lost.remove(k);
            let with_fec = error_at(&lost, k);
            // The next packet lands just after the lost frame falls due: no FEC in hand
            let mut next_late = lost.clone();
            next_late[k].at = anchor_of(&clean) + (k * frame) as i64 + 1;
            next_late.sort_by_key(|a| a.at);
            let without = error_at(&next_late, k);
            fec_total += with_fec;
            plc_total += without;
            fec_wins += u32::from(with_fec < without);
        }
        assert!(fec_wins >= 7, "FEC won {fec_wins} of 10");
        assert!(
            fec_total < 0.7 * plc_total,
            "FEC {fec_total:.4} against concealment {plc_total:.4}"
        );
    }

    /// The sender's clock runs on through a two-second pause while its sequence does not. The
    /// gap re-anchors, so what follows plays at the playout delay again, not two seconds late.
    #[test]
    fn a_stream_that_pauses_for_two_seconds_resumes_without_a_growing_delay() {
        let signal = source(2 * RATE as usize);
        let (first, second) = signal.split_at(RATE as usize);
        let mut sender = OpusSender::new(960, 3);
        let mut arrivals = sender.send_all(first, 0);
        sender.pause(2 * RATE);
        arrivals.extend(sender.send_all(second, 3 * SECOND));
        let arrivals = impair(&arrivals, Impairment::NONE, 2);
        let resumed = &arrivals[50..];
        let mut rig = Rig::new(Codec::opus().unwrap());
        rig.run(&arrivals, anchor_of(resumed) + second.len() as i64);
        let lookahead = sender.lookahead as i64;
        let before = compare(first, 960, &rig.out, anchor_of(&arrivals) + lookahead, 10);
        let after = compare(second, 960, &rig.out, anchor_of(resumed) + lookahead, 10);
        assert!(before.snr_db() > 10.0 && after.snr_db() > 10.0, "{before:?} {after:?}");
        let delay_after = after.lag - anchor_of(resumed);
        let delay_before = before.lag - anchor_of(&arrivals);
        assert!((delay_after - delay_before).abs() <= 10, "{delay_before} then {delay_after}");
        let stats = rig.stats();
        assert_eq!(stats.resyncs, 1, "{stats:?}");
        assert_eq!(stats.late, 0);
    }

    /// Opus DTX on the wire: through a pause the sender's timestamp runs on and its sequence
    /// number holds, so the first packet after it carries the very next one. Pauses of 360 ms,
    /// libopus's gap between its DTX refreshes, and 900 ms, just under a new stream, on every
    /// frame size: each part after a pause plays at the first part's delay, nothing is late,
    /// lost or re-anchored, and the pause is concealment, not loss.
    #[test]
    fn a_dtx_pause_under_a_second_loses_nothing_and_plays_on_time() {
        let signal = source(3 * RATE as usize);
        let (parts, _) = signal.as_chunks::<{ RATE as usize }>();
        let pauses = [0, 360 * RATE / 1000, 900 * RATE / 1000];
        for frame in [120usize, 240, 480, 960] {
            let mut sender = OpusSender::new(frame, 0xD7);
            let mut sent = Vec::new();
            let mut starts = Vec::new();
            let mut start = 0i64;
            for (part, pause) in parts.iter().zip(pauses) {
                sender.pause(pause);
                start += i64::from(pause);
                starts.push(start);
                sent.extend(sender.send_all(part, start));
                start += part.len() as i64;
            }
            let arrivals = impair(&sent, Impairment::NONE, 12);
            let mut rig = Rig::new(Codec::opus().unwrap());
            rig.run(&arrivals, anchor_of(&arrivals) + start);
            let expected = anchor_of(&arrivals) + sender.lookahead as i64;
            let delays: Vec<i64> = parts
                .iter()
                .zip(&starts)
                .map(|(part, start)| {
                    let fit = compare(part, 960, &rig.out, expected + start, BLOCK as i64);
                    assert!(fit.snr_db() > 9.0, "{frame}, from {start}: {fit:?}");
                    fit.lag - start
                })
                .collect();
            assert!((delays[0] - expected).abs() <= 10, "{frame}: {delays:?} for {expected}");
            assert!(delays.iter().all(|d| (d - delays[0]).abs() <= 2), "{frame}: {delays:?}");
            let stats = rig.stats();
            assert_eq!((stats.late, stats.lost, stats.resyncs), (0, 0, 0), "{frame}: {stats:?}");
            assert!(stats.concealed > 0, "{frame}: the pauses are concealed");
        }
    }

    /// #100: a phone singing with DTX on, sent as libwebrtc sends it, reads on the slot's meter
    /// at the level libopus decodes the same packets to. The reference puts each packet's decode
    /// at its timestamp on the slot's clock and leaves the DTX gaps silent, as GStreamer's
    /// `opusdec` does. Where the voice sounds the two agree within 0.1 dB; in a gap the slot
    /// plays libopus's concealment, comfort noise near the floor, so it never reads quieter.
    #[test]
    fn a_phones_dtx_stream_meters_at_the_level_libopus_decodes_it_to() {
        const TOLERANCE_DB: f32 = 0.1;
        const VOICED_DB: f32 = -30.0;
        let signal = phrases(12 * RATE as usize);
        let mut sender = OpusSender::at_bitrate(960, 0x1F, 32_000).with_dtx();
        let sent = sender.send_all(&signal, 0);
        assert!(sent.len() < signal.len() / 960 * 9 / 10, "the breaths go out as DTX");
        let arrivals = impair(&sent, Impairment::NONE, 3);
        let anchor = anchor_of(&arrivals) as usize;

        let mut reference = vec![0.0f32; anchor + signal.len() + MAX_FRAME];
        let mut decoder = OpusDecoder::new().unwrap();
        let mut scratch = [0.0f32; MAX_FRAME];
        let first = sent[0].packet.timestamp;
        for arrival in &sent {
            let frames = decoder.decode(arrival.packet.payload(), &mut scratch);
            let at = anchor + arrival.packet.timestamp.wrapping_sub(first) as usize;
            reference[at..at + frames].copy_from_slice(&scratch[..frames]);
        }

        let mut rig = Rig::new(Codec::opus().unwrap());
        rig.run(&arrivals, (anchor + signal.len()) as i64);
        let stats = rig.stats();
        assert_eq!((stats.lost, stats.late), (0, 0), "{stats:?}");

        const WINDOW: usize = WINDOW_FRAMES as usize;
        let windows = reference.as_chunks::<WINDOW>().0.iter().take(signal.len() / WINDOW);
        let mut voiced = 0;
        for (k, (window, &slot)) in windows.zip(&rig.levels).enumerate() {
            let power =
                window.iter().map(|s| f64::from(s * s)).sum::<f64>() / f64::from(WINDOW_FRAMES);
            let libopus = ((10.0 * power.log10()) as f32).max(FLOOR_DB);
            if libopus > VOICED_DB {
                voiced += 1;
                assert!((slot - libopus).abs() <= TOLERANCE_DB, "{k}: {slot} for {libopus}");
            }
            assert!(slot >= libopus - TOLERANCE_DB, "{k}: {slot} under libopus's {libopus}");
        }
        assert!(voiced > 60, "{voiced} voiced windows");
    }

    /// A pause and a loss together: the first packet after a pause never comes. The one after
    /// it shows the loss, which is counted once, and the stream plays on at its delay.
    #[test]
    fn a_packet_lost_as_a_pause_ends_is_counted_lost_once() {
        let signal = source(2 * RATE as usize);
        let (first, second) = signal.split_at(RATE as usize);
        let mut sender = OpusSender::new(960, 0xD8);
        let mut sent = sender.send_all(first, 0);
        let pause = 500 * RATE / 1000;
        sender.pause(pause);
        let resumed = SECOND + i64::from(pause);
        sent.extend(sender.send_all(second, resumed));
        sent.remove(50);
        let arrivals = impair(&sent, Impairment::NONE, 13);
        let mut rig = Rig::new(Codec::opus().unwrap());
        rig.run(&arrivals, anchor_of(&arrivals) + resumed + second.len() as i64);
        let expected = anchor_of(&arrivals) + sender.lookahead as i64 + resumed;
        let fit = compare(second, 2 * 960, &rig.out, expected, BLOCK as i64);
        assert!((fit.lag - expected).abs() <= 10 && fit.snr_db() > 9.0, "{expected}: {fit:?}");
        let stats = rig.stats();
        assert_eq!((stats.late, stats.lost, stats.resyncs), (0, 1, 0), "{stats:?}");
    }

    /// A new SSRC or `ResetSlot` is a new singer: the stream re-anchors at the next packet, and
    /// what the old stream still had in hand, under a playout delay's worth, goes with it.
    #[test]
    fn a_new_ssrc_or_reset_slot_starts_a_new_stream() {
        let signal = quantized(&source(3 * RATE as usize));
        let (parts, _) = signal.as_chunks::<{ RATE as usize }>();
        let mut a = L16Sender::new(240, 11);
        let mut b = L16Sender::new(240, 12);
        let mut arrivals = a.send_all(&parts[0], 0, 1.0);
        arrivals.extend(b.send_all(&parts[1], SECOND, 1.0));
        arrivals.extend(b.send_all(&parts[2], 2 * SECOND, 1.0));
        let arrivals = impair(&arrivals, Impairment::NONE, 4);
        let mut rig = Rig::new(Codec::L16);
        let reset_at = 2 * SECOND;
        rig.run(&arrivals, reset_at);
        rig.reset_slot();
        // The first packet to land after the reset anchors the new stream; part two follows it
        let first = arrivals.iter().position(|a| a.at >= reset_at).unwrap();
        let anchors = [
            anchor_of(&arrivals),
            anchor_of(&arrivals[200..]),
            anchor_of(&arrivals[first..]) + (400 - first as i64) * 240,
        ];
        rig.run(&arrivals, anchors[2] + SECOND);
        for (k, part) in parts.iter().enumerate() {
            // The last 30 ms of a part are in hand when the next stream or the reset drops it
            let kept = if k < 2 { part.len() - 1440 } else { part.len() };
            let fit = compare(&part[..kept], 240, &rig.out, anchors[k], 0);
            assert!(fit.error < 1e-6, "part {k} plays exactly at its own anchor: {fit:?}");
        }
        let stats = rig.stats();
        assert_eq!((stats.resyncs, stats.late, stats.lost), (2, 0, 0), "{stats:?}");
    }
}
