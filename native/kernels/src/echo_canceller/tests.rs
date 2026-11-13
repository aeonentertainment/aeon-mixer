use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::fixtures::{at_db, db, on_clock, pink_noise, rms, room, talker, through};
use crate::test_support::RATE;

const SECOND: usize = RATE as usize;

/// `far` played to the room and `mic` heard in it, through a canceller frame by frame under the
/// allocation guard, as a slot runs it: the mic's frame first, then the master's frame pushed.
/// `known(frame)` is the source's latency at each frame.
fn cancel(far: &[f32], mic: &[f32], known: impl Fn(usize) -> u32) -> (Vec<f32>, EchoCanceller) {
    let delay = |frame| SourceDelay { latency_frames: known(frame), spliced_frames: 0 };
    cancel_spliced(far, mic, delay)
}

/// The same, with the source's whole delay at each frame, its splices too.
fn cancel_spliced(
    far: &[f32],
    mic: &[f32],
    delay: impl Fn(usize) -> SourceDelay,
) -> (Vec<f32>, EchoCanceller) {
    let mut history = EchoHistory::new();
    let silent = EchoHistory::new();
    let mut canceller = EchoCanceller::new(RATE);
    let mut out = mic.to_vec();
    for (index, (far, block)) in
        far.as_chunks::<FRAME>().0.iter().zip(out.as_chunks_mut::<FRAME>().0.iter_mut()).enumerate()
    {
        assert_no_alloc(|| {
            canceller.process(&history, &silent, delay(index), block);
            history.push(far, far);
        });
    }
    (out, canceller)
}

/// The level removed over `[from, to)` seconds, mic against output.
fn removed(mic: &[f32], out: &[f32], from: f64, to: f64) -> f32 {
    let range = (from * SECOND as f64) as usize..(to * SECOND as f64) as usize;
    db(rms(&mic[range.clone()])) - db(rms(&out[range]))
}

/// A karaoke room's far end: a voice over a pink bed, as `aec:measure` builds it.
fn far_end(frames: usize) -> Vec<f32> {
    let voice = at_db(talker(frames, 110.0, 3), -18.0);
    let bed = at_db(pink_noise(frames, 7), -26.0);
    voice.iter().zip(&bed).map(|(v, b)| v + b).collect()
}

#[test]
fn history_reads_what_was_pushed_at_its_lag() {
    let mut history = EchoHistory::new();
    let block: Vec<f32> = (0..FRAME).map(|i| i as f32).collect();
    history.push(&block, &block.iter().map(|s| s + 2.0).collect::<Vec<_>>());
    let mut newest = [0.0; 4];
    history.read(0, &mut newest);
    assert_eq!(newest, [125.0, 126.0, 127.0, 128.0], "mono, the mean of the two channels");
    let mut before = [9.0; 4];
    history.read(FRAME - 2, &mut before);
    assert_eq!(before, [0.0, 0.0, 1.0, 2.0], "frames before the first push are silence");
}

// Off the path, nothing: a mic that hears no echo passes bit for bit, because nothing is found.
#[test]
fn a_mic_with_no_echo_passes_untouched() {
    let far = far_end(3 * SECOND);
    let mic = at_db(talker(3 * SECOND, 180.0, 9), -20.0);
    let (out, canceller) = cancel(&far, &mic, |_| 0);
    assert!(out.iter().zip(&mic).all(|(o, m)| o.to_bits() == m.to_bits()));
    assert_eq!(canceller.findings(), None);
}

// The finder places a pure delay within its 16-frame resolution, for a house mic's few
// milliseconds and a phone's few hundred.
#[test]
fn the_path_is_found_where_the_echo_is() {
    for delay in [3 * FRAME / 2, 70 * 48, 300 * 48] {
        let far = far_end(3 * SECOND);
        let echo = through(&far, &[(delay, 0.316)]);
        let (_, canceller) = cancel(&far, &echo, |_| 0);
        let found = canceller.findings().expect("found and adapted").path_frames as i64;
        assert!((found - delay as i64).abs() <= 16, "{delay} frames found at {found}");
    }
}

// A room: the direct path 40 ms after the master, 10 dB down, and a 300 ms reverberant tail.
// The filter's 171 ms covers the tail to 34 dB down; the reflections past it are what is left.
#[test]
fn a_room_echo_comes_out_and_stays_out() {
    let frames = 10 * SECOND;
    let far = far_end(frames);
    let path = room(40 * 48, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(frames, 99), -60.0);
    let mic: Vec<f32> = through(&far, &path).iter().zip(&floor).map(|(e, f)| e + f).collect();
    let (out, canceller) = cancel(&far, &mic, |_| 0);
    let bleed = db(rms(&through(&far, &path))) - db(rms(&far));

    let first_second = removed(&mic, &out, 0.0, 1.0);
    let steady = removed(&mic, &out, 3.0, 10.0);
    println!("removed: {first_second:.1} dB in the first second, {steady:.1} dB from 3 s");
    assert!(steady >= 25.0, "steady state removes {steady:.1} dB");
    let findings = canceller.findings().expect("adapted");
    assert!(
        (findings.bleed_db - bleed).abs() < 1.0,
        "bleed read {:.1} dB against {bleed:.1}",
        findings.bleed_db
    );
}

/// The level removed in each whole second.
fn per_second(mic: &[f32], out: &[f32]) -> Vec<f32> {
    (0..mic.len() / SECOND).map(|s| removed(mic, out, s as f64, s as f64 + 1.0)).collect()
}

// A lag between frames reads the signal between them, to within 1e-4 of a 1 kHz tone at half
// scale, and a whole lag reads the frame itself.
#[test]
fn history_reads_between_frames() {
    let tone = |t: f64| (0.5 * (std::f64::consts::TAU * 1000.0 * t / f64::from(RATE)).sin()) as f32;
    let played: Vec<f32> = (0..4 * FRAME).map(|k| tone(k as f64)).collect();
    let mut history = EchoHistory::new();
    for block in played.chunks(FRAME) {
        history.push(block, block);
    }
    for lag in [100.0, 100.25, 100.5, 311.9] {
        let mut out = [0.0; 64];
        Between::new(lag).read(&history, 0, &mut out);
        let newest = (4 * FRAME - 64) as f64 - lag;
        let worst = (0..64).map(|i| (out[i] - tone(newest + i as f64)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{lag}: off by {worst:e}");
    }
    let (mut whole, mut read) = ([0.0; 64], [0.0; 64]);
    Between::new(100.0).read(&history, 0, &mut whole);
    history.read(100, &mut read);
    assert_eq!(whole, read);
}

// A mic on its own clock hears the room's echo slide against the master, 4.8 frames a second at
// 100 ppm and 9.6 at 200. The canceller learns the rate, to 1% measured and 3% asked, and follows
// it, and from the fifth second takes out within 3 dB of what it takes from the same mic on the
// master's clock: measured, 1.6 dB at most.
#[test]
fn a_mic_on_its_own_clock_has_its_echo_taken_out_as_on_the_masters() {
    let frames = 10 * SECOND;
    // A second more of the room, so the slower mic's last frames have an echo to hear
    let far = far_end(frames + SECOND);
    let echo = through(&far, &room(40 * 48, 0.316, 0.3, 0.25, 11));
    let floor = at_db(pink_noise(frames, 99), -60.0);
    let through_clock = |ppm: f64| {
        let heard = on_clock(&echo, ppm);
        let mic: Vec<f32> = heard.iter().zip(&floor).map(|(e, f)| e + f).collect();
        let (out, canceller) = cancel(&far[..frames], &mic, |_| 0);
        (per_second(&mic, &out), canceller.drift.frames_per_second(RATE))
    };
    let (same, _) = through_clock(0.0);
    for ppm in [100.0, -100.0, 200.0, -200.0] {
        let (removed, rate) = through_clock(ppm);
        println!("{ppm} ppm: {removed:.1?} dB out against {same:.1?}; {rate:.2} frames a second");
        let drift = ppm * 1e-6 * f64::from(RATE);
        assert!((rate - drift).abs() < 0.03 * drift.abs(), "{ppm}: {rate} against {drift}");
        for second in 4..removed.len() {
            assert!(removed[second] > same[second] - 3.0, "{ppm}, second {}", second + 1);
        }
    }
}

/// `mic` as a capture slot hands it over when it splices `frames`, 1 or -1, at each of `count`
/// blocks from `from`: each of those reads 127 or 129 frames into 128 by linear interpolation.
/// Returns it and the source's splices so far at each block.
fn spliced(mic: &[f32], from: usize, count: usize, frames: i64) -> (Vec<f32>, Vec<i64>) {
    let (mut out, mut totals) = (Vec::with_capacity(mic.len()), Vec::new());
    let (mut at, mut total) = (0, 0);
    for block in 0..mic.len() / FRAME - 2 {
        let step = if (from..from + count).contains(&block) { frames } else { 0 };
        let read = (FRAME as i64 - step) as usize;
        for i in 0..FRAME {
            let (index, rest) = (at + i * read / FRAME, (i * read) % FRAME);
            let a = mic[index];
            out.push(a + (mic[index + 1] - a) * rest as f32 / FRAME as f32);
        }
        at += read;
        total += step;
        totals.push(total);
    }
    (out, totals)
}

// A capture slot corrects a 480-frame mic's slip by splicing 32 frames in or out, a frame a
// block, and a 128-frame mic's by 128, and says so. The echo moves with each splice, and the
// canceller slides every window its filter holds with it. The second it happens in loses 1.7 dB
// of what it takes out for 32 splices, within a tolerance of 3, and 4.0 for 128, within 5; every
// second after is within 0.5 dB of a mic that never spliced.
#[test]
fn the_echo_is_followed_through_a_sources_splices() {
    let frames = 10 * SECOND;
    let far = far_end(frames);
    let path = room(40 * 48, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(frames, 99), -60.0);
    let raw: Vec<f32> = through(&far, &path).iter().zip(&floor).map(|(e, f)| e + f).collect();
    let from = 5 * SECOND / FRAME;
    let through_splices = |count: usize, frames: i64| {
        let (mic, totals) = spliced(&raw, from, count, frames);
        let delay = |block: usize| SourceDelay { latency_frames: 0, spliced_frames: totals[block] };
        let (out, _) = cancel_spliced(&far, &mic, delay);
        per_second(&mic, &out)
    };
    let still = through_splices(0, 0);
    for (count, frames) in [(32, 1), (32, -1), (128, 1), (128, -1)] {
        let removed = through_splices(count, frames);
        println!("{count} x {frames}: {removed:.1?} dB out against {still:.1?}");
        let within = if count == 32 { 3.0 } else { 5.0 };
        assert!(removed[5] > still[5] - within, "{count} x {frames}: the second it splices in");
        for second in 6..removed.len() {
            let case = format!("{count} x {frames}, second {second}");
            assert!((removed[second] - still[second]).abs() < 0.5, "{case}");
        }
    }
}

// A capture slot with a fine step, such as a 100-frame mic under 128-frame callbacks, splices a
// few frames every second or so. Its echo path moves 400 frames at 4 s, as a mic carried across
// the room does. Splices that small never hold the finder back, so the canceller finds the new
// path and takes the echo out again: 15 dB or more from the ninth second.
#[test]
fn a_source_that_splices_a_little_and_often_still_has_a_new_path_found() {
    let frames = 12 * SECOND;
    let far = far_end(frames);
    let floor = at_db(pink_noise(frames, 99), -60.0);
    let first = through(&far, &room(40 * 48, 0.316, 0.3, 0.25, 11));
    let moved = through(&far, &room(40 * 48 + 400, 0.316, 0.3, 0.25, 12));
    let at = 4 * SECOND;
    let raw: Vec<f32> =
        (0..frames).map(|k| if k < at { first[k] } else { moved[k] } + floor[k]).collect();
    // Four frames spliced in every 300 blocks, 0.8 s
    let (mut mic, mut totals) = (raw.clone(), vec![0; frames / FRAME]);
    for from in (0..frames / FRAME - 8).step_by(300) {
        let (next, more) = spliced(&mic, from, 4, 1);
        mic = next;
        mic.resize(frames, 0.0);
        for (total, extra) in totals.iter_mut().zip(more.iter().chain(std::iter::repeat(&4))) {
            *total += extra;
        }
    }
    let delay = |block: usize| SourceDelay { latency_frames: 0, spliced_frames: totals[block] };
    let (out, _) = cancel_spliced(&far, &mic, delay);
    let removed = per_second(&mic, &out);
    println!("{removed:.1?} dB out, the path moving 400 frames at 4 s");
    assert!(removed[8..].iter().all(|db| *db >= 15.0), "{removed:.1?}");
}
