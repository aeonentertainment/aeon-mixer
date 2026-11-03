//! The source of an `rtp` or `l16` slot: at the top of each block it drains the slot's packet
//! ring into the jitter buffer, then pulls the block that is due.

use super::jitter::JitterBuffer;
use super::packet::PacketConsumer;
use crate::slot::source::Source;

pub struct PacketSource {
    packets: PacketConsumer,
    jitter: JitterBuffer,
}

impl PacketSource {
    pub fn new(packets: PacketConsumer, jitter: JitterBuffer) -> Self {
        Self { packets, jitter }
    }

    pub fn jitter(&self) -> &JitterBuffer {
        &self.jitter
    }
}

impl Source for PacketSource {
    fn pull(&mut self, out: &mut [f32]) {
        let Self { packets, jitter } = self;
        while packets.pop_with(|packet| jitter.push(packet)) {}
        jitter.pull(out);
    }

    fn reset(&mut self) {
        self.jitter.reset();
    }

    fn latency_frames(&self) -> u32 {
        self.jitter.latency_frames()
    }
}

#[cfg(test)]
mod tests {
    use super::super::codec::Codec;
    use super::super::fixtures::*;
    use crate::block::{BLOCK, RATE};

    const SECOND: i64 = RATE as i64;

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
