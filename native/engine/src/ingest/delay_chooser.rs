//! How a slot chooses its own playout delay (docs/design.md 8.4, decision 0007). A stream opens at
//! a delay that suits busy Wi-Fi. Every half second the chooser notes how close to its due time
//! the half second's latest packet came, so a stall counts once however short the frames are. Once
//! ten seconds of that says the stream never needed the slack it has, the delay falls, once, to
//! what those ten seconds used. If packets then keep arriving late it rises, at most back to where
//! it opened. It moves only while the slot is quiet, when nobody can hear it.

use crate::block::{BLOCK, RATE};

/// The delay a stream opens at, and the most a fall can ever give back.
pub const OPENING: u32 = RATE * 15 / 1000;

/// One note per half second: a stall spans at most a few, and neither a frame's length nor how
/// many packets a stall catches changes what it counts for.
const INTERVAL: i64 = RATE as i64 / 2;

/// Ten seconds of notes. The worst of 20 covers 86 percent of half seconds with 95 percent
/// confidence whatever the distribution, and a later half second beats it about once in ten
/// seconds. The margin is what makes that rarer; decision 0007 has what it buys.
const INTERVALS: usize = 20;

/// What a link with no jitter at all still needs on top of the block a packet can wait for.
const MARGIN: i64 = RATE as i64 / 1000;

/// A packet can wait up to a block to be drained, so nothing shorter survives even loopback.
pub const FLOOR: i64 = BLOCK as i64 + MARGIN;

/// A smaller gain is not worth moving for.
const LEAST_FALL: i64 = BLOCK as i64;

/// Late half seconds out of the last ten seconds' worth that mean the link has worsened. One is a
/// stall the margin did not cover, which is expected about every ten seconds.
const RISE_AFTER: usize = 3;

/// A slot is quiet under -30 dBFS: silence, a breath, or the room's own bleed into a mic held at
/// the mouth. A jump in audio that small is lost under the backing track.
pub const QUIET_PEAK: f32 = 0.0316;

/// Blocks of quiet before a move, 21 ms, so a dip inside a sung note does not count.
const QUIET_BLOCKS: u32 = 8;

/// A move of the anchor: frames later when `by` is positive, and the delay it leaves the stream at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub by: i64,
    pub delay: i64,
}

/// The slack of one interval's earliest and latest packets against their due times.
#[derive(Clone, Copy)]
struct Interval {
    worst: i64,
    best: i64,
}

impl Interval {
    const EMPTY: Self = Self { worst: i64::MAX, best: i64::MIN };
}

pub struct DelayChooser {
    current: Interval,
    ends_at: i64,
    intervals: [Interval; INTERVALS],
    /// Where the next note goes, and how many have been taken, up to all of them.
    next: usize,
    filled: usize,
    /// How far the anchor sits ahead of where it opened.
    fallen: i64,
    has_fallen: bool,
    wanted: Option<Move>,
    quiet_blocks: u32,
}

impl DelayChooser {
    pub fn new(now: i64) -> Self {
        Self {
            current: Interval::EMPTY,
            ends_at: now + INTERVAL,
            intervals: [Interval::EMPTY; INTERVALS],
            next: 0,
            filled: 0,
            fallen: 0,
            has_fallen: false,
            wanted: None,
            quiet_blocks: 0,
        }
    }

    /// How far the anchor sits ahead of where it opened, which is how much sooner a packet is late
    /// here than at the opening delay.
    pub fn fallen(&self) -> i64 {
        self.fallen
    }

    /// A packet's slack against its due time, whether it made it or not.
    pub fn observe(&mut self, slack: i64) {
        self.current.worst = self.current.worst.min(slack);
        self.current.best = self.current.best.max(slack);
    }

    /// The block just played, to tell whether the slot is quiet.
    pub fn hear(&mut self, block: &[f32]) {
        let quiet = block.iter().all(|sample| sample.abs() < QUIET_PEAK);
        self.quiet_blocks = if quiet { self.quiet_blocks.saturating_add(1) } else { 0 };
    }

    /// Once a block: closes an interval that has ended, and returns the move wanted, if the slot
    /// has been quiet. The caller tells `made` when it has made it, or asks again next block.
    pub fn tick(&mut self, now: i64) -> Option<Move> {
        if now >= self.ends_at {
            self.ends_at = now + INTERVAL;
            // A sender in DTX or a stall that outlasts an interval leaves nothing to note
            if self.current.best != Interval::EMPTY.best {
                self.intervals[self.next] = self.current;
                self.next = (self.next + 1) % INTERVALS;
                self.filled = (self.filled + 1).min(INTERVALS);
                self.current = Interval::EMPTY;
            }
            self.wanted = self.choose();
        }
        if self.quiet_blocks < QUIET_BLOCKS {
            return None;
        }
        self.wanted
    }

    /// The caller moved the anchor as `tick` asked.
    pub fn made(&mut self, moved: Move) {
        self.fallen -= moved.by;
        self.has_fallen |= moved.by < 0;
        self.shift(moved.by);
    }

    /// The anchor moved for another reason, a skew correction: every slack noted moved with it.
    pub fn shift(&mut self, by: i64) {
        let move_slack = |interval: &mut Interval| {
            if interval.best != Interval::EMPTY.best {
                interval.worst += by;
                interval.best += by;
            }
        };
        self.intervals.iter_mut().for_each(move_slack);
        move_slack(&mut self.current);
        self.wanted = None;
    }

    /// `worst - margin` is how far the anchor can move earlier and still cover what these ten
    /// seconds did. A quarter of the spread goes on the margin, because a link that varies by more
    /// has more left to show than ten seconds of it did; the block a packet can wait is no such
    /// evidence.
    fn choose(&self) -> Option<Move> {
        if self.filled < INTERVALS {
            return None;
        }
        let worst = self.intervals.iter().map(|interval| interval.worst).min()?;
        let best = self.intervals.iter().map(|interval| interval.best).max()?;
        let margin = MARGIN + (best - worst - BLOCK as i64).max(0) / 4;
        let room = (worst - margin).min(best - FLOOR);
        if self.has_fallen {
            let late = self.intervals.iter().filter(|interval| interval.worst < 0).count();
            let by = (-room).min(self.fallen);
            (late >= RISE_AFTER && by > 0).then_some(Move { by, delay: best + by })
        } else {
            (room >= LEAST_FALL).then_some(Move { by: -room, delay: best - room })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::codec::Codec;
    use super::super::fixtures::*;
    use super::super::jitter::JitterBuffer;
    use super::*;

    const SECOND: i64 = RATE as i64;

    /// Silence long enough to settle, then tones to measure the lag with.
    fn settle_then_tones(tones: usize) -> Vec<f32> {
        let mut signal = vec![0.0; 12 * RATE as usize];
        signal.extend(source(tones));
        signal
    }

    fn delay_frames(rig: &Rig) -> i64 {
        (rig.stats().playout_delay_ms * RATE as f32 / 1000.0).round() as i64
    }

    /// A link with no jitter owes nothing but the block a packet can wait, on every frame size:
    /// 20 ms frames arrive in 20 ms lumps, and that is the sender's gathering, never a reason to
    /// hold them longer here. The sound comes out where the delay says, within that block's wait,
    /// which takes 11 ms or more off what the same stream holds at the opening delay.
    #[test]
    fn a_clean_link_falls_to_the_floor_whatever_the_frame() {
        let signal = settle_then_tones(RATE as usize);
        let tones = 12 * RATE as usize;
        for frame in [120usize, 240, 480, 960] {
            let mut sender = OpusSender::new(frame, 40);
            let arrivals = sender.send_all(&signal, 0);
            let (ends, plays_out) = (13 * SECOND, 13 * SECOND + 4800);
            let mut chosen = Rig::choosing(Codec::opus().unwrap());
            chosen.run(&arrivals, ends);
            let stats = chosen.stats();
            assert_eq!(delay_frames(&chosen), FLOOR, "{frame}: {stats:?}");
            assert_eq!((stats.late, stats.lost), (0, 0), "{frame}: {stats:?}");
            chosen.run(&arrivals, plays_out);
            let mut fixed = Rig::new(Codec::opus().unwrap());
            fixed.run(&arrivals, plays_out);
            let lag = |rig: &Rig, opening: i64| {
                let expected = tones as i64 + (sender.lookahead + frame) as i64 + opening;
                compare(&signal[tones..], frame, &rig.out, expected, BLOCK as i64).lag
            };
            let held = i64::from(OPENING);
            let saved = lag(&fixed, held) - lag(&chosen, FLOOR);
            assert!(saved >= held - FLOOR - BLOCK as i64, "{frame}: {saved} frames saved");
        }
    }

    /// Stalls set the delay: it is the spread the link showed, the stall and the jitter and the
    /// block's wait, plus a millisecond and a quarter of what the spread adds over the block.
    /// Within a block, since a stall lands wherever it lands against the block grid. Every eighth
    /// frame has one, so a 20 ms stream, which a 3 ms stall catches one time in seven, still shows
    /// it inside ten seconds. The song then loses at most 0.1 percent more of its packets than the
    /// same song held at the opening delay, and the count the slot keeps of those is the one a held
    /// run really gets.
    #[test]
    fn a_known_stall_sets_the_delay_within_a_block() {
        let signal = voice(30 * RATE as usize);
        for (frame, stall_ms) in [(240usize, 3), (240, 6), (960, 3), (960, 6)] {
            let link = Stalls::new(RATE as i64 * stall_ms / 1000, 8 * frame as i64);
            let arrivals = link.apply(&OpusSender::new(frame, 41).send_all(&signal, 0), 2);
            let until = 30 * SECOND;
            let mut chosen = Rig::choosing(Codec::opus().unwrap());
            chosen.run(&arrivals, until);
            let mut fixed = Rig::new(Codec::opus().unwrap());
            fixed.run(&arrivals, until);
            let stats = chosen.stats();
            let spread = link.stall + link.jitter + BLOCK as i64;
            let expected = spread + MARGIN + (spread - BLOCK as i64) / 4;
            let delay = delay_frames(&chosen);
            assert!(
                (delay - expected).abs() <= BLOCK as i64,
                "{frame}, {stall_ms} ms: delay {delay}, expected {expected}: {stats:?}"
            );
            assert!(delay < i64::from(OPENING), "{frame}, {stall_ms} ms: {stats:?}");
            let extra = stats.late - stats.late_at_opening;
            assert!(extra * 1000 <= stats.pushed, "{frame}, {stall_ms} ms: {stats:?}");
            assert_eq!(stats.late_at_opening, fixed.stats().late, "{frame}, {stall_ms} ms");
        }
    }

    /// Spikes of 30 ms, which 15 ms never covered: the delay stays where it opened, and the slot
    /// plays exactly what a held slot plays, sample for sample.
    #[test]
    fn a_link_that_needs_all_of_the_opening_delay_never_moves_it() {
        let signal = quantized(&voice(30 * RATE as usize));
        let mut sender = L16Sender::new(240, 42);
        let arrivals = impair(&sender.send_all(&signal, 0, 1.0), Impairment::WIFI, 3);
        let until = 30 * SECOND;
        let mut chosen = Rig::choosing(Codec::L16);
        chosen.run(&arrivals, until);
        let mut fixed = Rig::new(Codec::L16);
        fixed.run(&arrivals, until);
        assert_eq!(chosen.out, fixed.out);
        assert_eq!(chosen.stats().playout_delay_ms, 15.0);
        assert_eq!(chosen.stats().late, chosen.stats().late_at_opening);
    }

    /// The link worsens after the delay has fallen. The slot rises to cover the new stalls, and
    /// at most back to where it opened: three late half seconds, then the next quiet moment, and
    /// nothing is late after that. A stall of 9 ms against a floor of 3.67 is late every time, so
    /// this is the worst case a fall can come to.
    #[test]
    fn a_link_that_worsens_makes_the_delay_rise_in_the_next_quiet_moment() {
        let signal = quantized(&voice(40 * RATE as usize));
        let mut sender = L16Sender::new(240, 43);
        let sent = sender.send_all(&signal, 0, 1.0);
        let worsens_at = 20 * SECOND;
        let (calm, rough) = sent.split_at(sent.partition_point(|a| a.at < worsens_at));
        let mut arrivals = calm.to_vec();
        let stalls = Stalls::new(432, SECOND / 4);
        arrivals.extend(stalls.apply(rough, 4));
        arrivals.sort_by_key(|a| a.at);

        let mut rig = Rig::choosing(Codec::L16);
        rig.run(&arrivals, worsens_at);
        assert_eq!(delay_frames(&rig), FLOOR, "fell before it worsened: {:?}", rig.stats());
        assert_eq!(rig.stats().late, 0);
        rig.run(&arrivals, worsens_at + 3 * SECOND);
        let settled = rig.stats();
        let delay = delay_frames(&rig);
        let spread = stalls.stall + stalls.jitter;
        assert!(
            delay >= spread && delay <= i64::from(OPENING) + 48,
            "rose to {delay}: {settled:?}"
        );
        rig.run(&arrivals, 40 * SECOND);
        assert_eq!(rig.stats().late, settled.late, "nothing late once it rose: {:?}", rig.stats());
        assert_eq!(rig.stats().late_at_opening, 0, "a held slot never lost one of these");
        assert!(settled.late * 100 <= rig.stats().pushed, "{settled:?}");
    }

    /// A singer never stops: the delay stays where it opened and nothing is dropped to move it.
    /// When they stop, it falls. Then a voice in bursts: every burst comes out whole, at one lag or
    /// the other, because the move went in the quiet between two of them.
    #[test]
    fn the_delay_moves_only_in_a_quiet_moment() {
        let singing = quantized(&source(20 * RATE as usize));
        let mut sender = L16Sender::new(240, 44);
        let mut sent = sender.send_all(&singing, 0, 1.0);
        sent.extend(sender.send_all(&vec![0.0; 2 * RATE as usize], 20 * SECOND, 1.0));
        let mut rig = Rig::choosing(Codec::L16);
        rig.run(&sent, 20 * SECOND);
        assert_eq!(rig.stats().playout_delay_ms, 15.0, "{:?}", rig.stats());
        rig.run(&sent, 21 * SECOND);
        assert_eq!(delay_frames(&rig), FLOOR, "{:?}", rig.stats());

        let signal = quantized(&voice(14 * RATE as usize));
        let mut sender = L16Sender::new(240, 45);
        let arrivals = sender.send_all(&signal, 0, 1.0);
        let mut rig = Rig::choosing(Codec::L16);
        rig.run(&arrivals, 14 * SECOND);
        assert_eq!(delay_frames(&rig), FLOOR, "{:?}", rig.stats());
        let opened = anchor_of(&arrivals);
        let burst = (RATE as f32 * 0.45) as usize;
        for start in (0..signal.len() - burst).step_by(burst) {
            let voiced = &signal[start..start + (RATE as f32 * 0.3) as usize];
            let fit = compare(voiced, 0, &rig.out, opened + start as i64, 800);
            assert!(fit.error < 1e-3, "the burst at {start} was cut: {fit:?}");
        }
    }

    /// Short bursts with gaps just long enough for the gate, on frames long enough that the
    /// next burst's onset is already decoded and waiting when the gate opens. The fall drops
    /// only what the gate has heard is quiet, so every burst still comes out whole, at the lag
    /// before the fall or the one after.
    #[test]
    fn a_fall_never_cuts_an_onset_already_waiting_to_play() {
        let (burst, gap) = (RATE as usize * 30 / 1000, RATE as usize * 40 / 1000);
        let tone = source(burst);
        let mut signal = Vec::new();
        while signal.len() < 14 * RATE as usize {
            signal.extend_from_slice(&tone);
            signal.extend(std::iter::repeat_n(0.0, gap));
        }
        let cases: [(Codec, usize, f32); 2] =
            [(Codec::opus().unwrap(), 960, 0.1), (Codec::L16, 350, 1e-6)];
        for (codec, frame, tolerance) in cases {
            let (arrivals, lookahead) = match codec {
                Codec::L16 => (L16Sender::new(frame, 51).send_all(&quantized(&signal), 0, 1.0), 0),
                Codec::Opus(_) => {
                    let mut sender = OpusSender::new(frame, 51);
                    (sender.send_all(&signal, 0), sender.lookahead as i64)
                }
            };
            let mut rig = Rig::choosing(codec);
            rig.run(&arrivals, 14 * SECOND);
            assert_eq!(delay_frames(&rig), FLOOR, "{frame}: {:?}", rig.stats());
            let reference = if tolerance < 1e-3 { quantized(&signal) } else { signal.clone() };
            let opened = anchor_of(&arrivals) + lookahead;
            for start in (0..reference.len() - burst - gap).step_by(burst + gap) {
                let fit = compare(
                    &reference[start..start + burst],
                    0,
                    &rig.out,
                    opened + start as i64,
                    800,
                );
                assert!(fit.error < tolerance, "{frame}: the burst at {start} was cut: {fit:?}");
            }
        }
    }

    /// A slow sender at the floor gets skew corrections a held stream would not have needed
    /// yet. Packets held up late in the song must still be counted the way the held stream
    /// counts them, so the comparison stays the real one.
    #[test]
    fn late_at_opening_counts_what_a_held_stream_counts_under_skew() {
        let signal = vec![0.0; 40 * RATE as usize];
        let mut arrivals = L16Sender::new(240, 52).send_all(&signal, 0, 0.9998);
        for arrival in arrivals.iter_mut().skip(7000).step_by(97).take(20) {
            arrival.at += 600;
        }
        arrivals.sort_by_key(|a| a.at);
        let mut chosen = Rig::choosing(Codec::L16);
        chosen.run(&arrivals, 40 * SECOND);
        let mut held = Rig::new(Codec::L16);
        held.run(&arrivals, 40 * SECOND);
        let (chosen, held) = (chosen.stats(), held.stats());
        assert!(chosen.resyncs > held.resyncs + 1, "skew corrected apart: {chosen:?} {held:?}");
        assert!(held.late > 0, "{held:?}");
        assert_eq!(chosen.late_at_opening, held.late, "{chosen:?} {held:?}");
    }

    /// A stall holds the first few packets 30 ms, and the first packet anchors the stream, so it
    /// opens 30 ms behind where it could be. The fall takes that off with the rest: what comes out
    /// after it is where a stream with no stall plays.
    #[test]
    fn a_first_packet_that_was_late_costs_nothing_after_the_fall() {
        let signal = settle_then_tones(RATE as usize);
        let tones = 12 * RATE as usize;
        let lag = |arrivals: &[Arrival]| {
            let mut rig = Rig::choosing(Codec::L16);
            rig.run(arrivals, 13 * SECOND);
            assert_eq!(delay_frames(&rig), FLOOR, "{:?}", rig.stats());
            compare(&signal[tones..], 240, &rig.out, tones as i64 + 400, 1200).lag
        };
        let clean = L16Sender::new(240, 46).send_all(&signal, 0, 1.0);
        let mut stalled = clean.clone();
        let released = clean[0].at + 1440;
        stalled.iter_mut().take_while(|a| a.at < released).for_each(|a| a.at = released);
        assert!((lag(&stalled) - lag(&clean)).abs() < BLOCK as i64);
    }

    /// A new SSRC is a new singer on an unknown link, so the delay opens again; so does a reset.
    #[test]
    fn a_new_stream_opens_at_the_opening_delay_again() {
        let signal = vec![0.0; 12 * RATE as usize];
        let mut first = L16Sender::new(240, 47);
        let mut arrivals = first.send_all(&signal, 0, 1.0);
        let mut second = L16Sender::new(240, 48);
        arrivals.extend(second.send_all(&signal, 12 * SECOND, 1.0));
        let mut rig = Rig::choosing(Codec::L16);
        rig.run(&arrivals, 11 * SECOND);
        assert_eq!(delay_frames(&rig), FLOOR);
        rig.run(&arrivals, 12 * SECOND + 4800);
        assert_eq!(rig.stats().playout_delay_ms, 15.0, "{:?}", rig.stats());
        rig.run(&arrivals, 23 * SECOND);
        assert_eq!(delay_frames(&rig), FLOOR);
        rig.reset_slot();
        rig.run(&arrivals, 23 * SECOND + 4800);
        assert_eq!(rig.stats().playout_delay_ms, 15.0, "{:?}", rig.stats());
    }

    /// What the guide clock takes off the playhead stays true after the fall: within the block a
    /// packet can wait, the delay a sung sample really has.
    #[test]
    fn latency_frames_follows_the_delay_down() {
        let signal = settle_then_tones(RATE as usize);
        let tones = 12 * RATE as usize;
        for frame in [120usize, 240] {
            let mut sender = L16Sender::new(frame, 49);
            let arrivals = sender.send_all(&signal, 0, 1.0);
            let mut jitter = JitterBuffer::chosen(Codec::L16);
            let out = pull_direct(&mut jitter, &arrivals, 13 * SECOND);
            let fit = compare(&signal[tones..], frame, &out, tones as i64 + 400, 1200);
            let measured = fit.lag - tones as i64;
            let reported = i64::from(jitter.latency_frames());
            assert!(
                (0..BLOCK as i64).contains(&(measured - reported)),
                "{frame}: measured {measured}, reported {reported}"
            );
        }
    }

    /// Two hundred parts per million either way, a long way off for a phone against a sound card,
    /// for a minute. Ten seconds of that drift reads as 2 ms of spread, so the delay falls that far
    /// short of the floor, and skew correction holds it there on every frame size with no packet
    /// late for it. A step of 20 ms would have put the slow sender's delay back over the opening
    /// delay at its first correction.
    #[test]
    fn clock_skew_at_the_floor_makes_nothing_late() {
        let signal = vec![0.0; 60 * RATE as usize];
        for frame in [120usize, 240, 480, 960] {
            for rate in [1.0002f64, 0.9998] {
                let mut sender = OpusSender::new(frame, 50);
                let on_time: Vec<_> = sender
                    .send_all(&signal, 0)
                    .into_iter()
                    .map(|a| Arrival { at: (a.at as f64 / rate).round() as i64, ..a })
                    .collect();
                let mut jitter = JitterBuffer::chosen(Codec::opus().unwrap());
                pull_direct(&mut jitter, &on_time, 60 * SECOND);
                let stats = jitter.stats();
                assert_eq!(stats.late, 0, "{frame} at {rate}: {stats:?}");
                assert!(
                    (3.6..6.5).contains(&stats.playout_delay_ms),
                    "{frame} at {rate}: {stats:?}"
                );
                assert!(stats.resyncs >= 2, "{frame} at {rate}: skew corrected: {stats:?}");
            }
        }
    }
}
