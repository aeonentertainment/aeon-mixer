use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::fixtures::{at_db, db, pink_noise, rms, room, talker, through};
use crate::test_support::RATE;

const SECOND: usize = RATE as usize;

/// `far` played to the room and `mic` heard in it, through a canceller frame by frame under the
/// allocation guard, as a slot runs it: the mic's frame first, then the master's frame pushed.
/// `known(frame)` is the source's latency at each frame.
fn cancel(far: &[f32], mic: &[f32], known: impl Fn(usize) -> u32) -> (Vec<f32>, EchoCanceller) {
    let mut history = EchoHistory::new();
    let silent = EchoHistory::new();
    let mut canceller = EchoCanceller::new(RATE);
    let mut out = mic.to_vec();
    for (index, (far, block)) in
        far.as_chunks::<FRAME>().0.iter().zip(out.as_chunks_mut::<FRAME>().0.iter_mut()).enumerate()
    {
        assert_no_alloc(|| {
            canceller.process(&history, &silent, known(index), block);
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
