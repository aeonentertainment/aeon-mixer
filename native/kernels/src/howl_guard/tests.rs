use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::fixtures::{at_db, db, rms, talker};
use crate::test_support::RATE;

const BLOCK: usize = 128;

/// `signal` through a guard, block by block under the allocation guard.
fn guarded(mut signal: Vec<f32>) -> (Vec<f32>, HowlGuard) {
    let mut guard = HowlGuard::new(RATE);
    for block in signal.as_chunks_mut::<BLOCK>().0.iter_mut() {
        assert_no_alloc(|| guard.process(block));
    }
    (signal, guard)
}

fn sine(hz: f32, frames: usize, level_db: impl Fn(usize) -> f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let amplitude = 10f32.powf(level_db(i) / 20.0);
            amplitude * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()
        })
        .collect()
}

// A singer is not a howl: a voice-shaped talker at a hot level for ten seconds, its pitch moving
// and its harmonics standing beside each peak, passes bit for bit.
#[test]
fn a_voice_passes_untouched() {
    let voice = at_db(talker(10 * RATE as usize, 160.0, 5), -12.0);
    let (out, guard) = guarded(voice.clone());
    assert_eq!(guard.catches(), 0);
    assert!(out.iter().zip(&voice).all(|(o, v)| o.to_bits() == v.to_bits()));
}

/// A melody of `notes`, each a MIDI note and its level in dBFS, held `note_ms` with `breath_ms`
/// of silence after it: `harmonics` gives each partial's level against the fundamental, in dB,
/// and `vibrato_cents` a 5.5 Hz vibrato's depth either way.
fn melody(
    notes: &[(f32, f32)],
    note_ms: usize,
    breath_ms: usize,
    harmonics: &[f32],
    vibrato_cents: f32,
) -> Vec<f32> {
    let rate = RATE as usize;
    let (note, breath) = (note_ms * rate / 1000, breath_ms * rate / 1000);
    let mut out = Vec::with_capacity(notes.len() * (note + breath));
    let mut phase = 0.0f32;
    for &(midi, level_db) in notes {
        let amplitude = 10f32.powf(level_db / 20.0);
        for i in 0..note {
            let t = i as f32 / RATE as f32;
            let cents = vibrato_cents * (std::f32::consts::TAU * 5.5 * t).sin();
            let hz = 440.0 * 2f32.powf((midi - 69.0 + cents / 100.0) / 12.0);
            phase = (phase + hz / RATE as f32).fract();
            // Each note fades in and out over 10 ms, as a voice does
            let edge = (i.min(note - i) as f32 / (0.01 * RATE as f32)).min(1.0);
            let partials: f32 = std::iter::once(0.0)
                .chain(harmonics.iter().copied())
                .enumerate()
                .map(|(h, db)| {
                    10f32.powf(db / 20.0) * (std::f32::consts::TAU * (h + 1) as f32 * phase).sin()
                })
                .sum();
            out.push(amplitude * edge * partials);
        }
        out.resize(out.len() + breath, 0.0);
    }
    out
}

// A whistled phrase is a tone with no harmonic at all, and it may climb a semitone a note while it
// swells. E7 to B7 a semitone at a time, legato, 300 ms notes with 20 ms between, from -36 dBFS up
// 2 dB a note to -22, is a melody, not a howl, and passes bit for bit. Followed through each
// semitone it grew 6 dB in three notes and was ducked twice.
#[test]
fn a_whistled_melody_climbing_and_swelling_is_left_alone() {
    let notes: Vec<(f32, f32)> =
        (0..8).map(|n| (100.0 + n as f32, -36.0 + 2.0 * n as f32)).collect();
    let whistle = melody(&notes, 300, 20, &[], 0.0);
    let (out, guard) = guarded(whistle.clone());
    assert_eq!(guard.catches(), 0, "a whistled melody was ducked");
    assert!(out.iter().zip(&whistle).all(|(o, w)| o.to_bits() == w.to_bits()));
}

// The bug hunt's case: E7 at -30 dBFS for 400 ms, a 20 ms breath, then F7 6 dB louder. Two notes,
// each steady, are not a loop running away.
#[test]
fn a_louder_next_note_a_semitone_up_is_left_alone() {
    let whistle = melody(&[(100.0, -30.0), (101.0, -24.0)], 400, 20, &[], 0.0);
    let (_, guard) = guarded(whistle);
    assert_eq!(guard.catches(), 0, "the louder second note was ducked");
}

// A soprano's high vowel has its second harmonic only 15 dB down, a third 20 dB down, and a
// 5.5 Hz vibrato 25 cents either way. A5 to E6 a semitone a note, legato, 400 ms notes with 20 ms
// between, from -30 dBFS up 2 dB a note to -16, is left alone. At A5 a semitone is about a bin, so
// the guard that followed a bin at a time ducked this too.
#[test]
fn a_soprano_climbing_and_swelling_is_left_alone() {
    let notes: Vec<(f32, f32)> =
        (0..8).map(|n| (81.0 + n as f32, -30.0 + 2.0 * n as f32)).collect();
    let soprano = melody(&notes, 400, 20, &[-15.0, -20.0], 25.0);
    let (_, guard) = guarded(soprano);
    assert_eq!(guard.catches(), 0, "a soprano's phrase was ducked");
}

// A steady tone, a whistle or a held pure note, is not running away either. The guard tells a
// howl by its growth, so a tone that holds still at any level is left alone.
#[test]
fn a_steady_tone_is_left_alone() {
    let (_, guard) = guarded(sine(1000.0, 5 * RATE as usize, |_| -10.0));
    assert_eq!(guard.catches(), 0);
}

// A loop 2 dB over unity with an 80 ms round trip grows 25 dB a second. From -50 dBFS it is
// caught before it passes -30, within 400 ms, and the slot comes out 12 dB down. This tone is
// open loop, so the duck cannot stop it, and the guard ducks again each time it runs on: a
// loop the first duck leaves over unity is turned down further.
#[test]
fn a_tone_that_runs_away_is_ducked_and_ducked_again() {
    const GROWTH_DB_PER_SECOND: f32 = 25.0;
    let frames = 2 * RATE as usize;
    let level = |i: usize| -50.0 + GROWTH_DB_PER_SECOND * i as f32 / RATE as f32;
    let tone = sine(1200.0, frames, level);
    let (out, guard) = guarded(tone.clone());
    let caught = out
        .as_chunks::<BLOCK>()
        .0
        .iter()
        .zip(tone.as_chunks::<BLOCK>().0.iter())
        .position(|(o, t)| rms(o) < 0.9 * rms(t))
        .expect("ducked")
        * BLOCK;
    let caught_ms = caught as f32 * 1000.0 / RATE as f32;
    println!("caught at {caught_ms:.0} ms, at {:.1} dBFS", level(caught));
    assert!(caught_ms < 400.0 && level(caught) < -30.0, "caught at {caught_ms:.0} ms");
    let after = caught + RATE as usize / 50..caught + RATE as usize / 10;
    let ducked = db(rms(&out[after.clone()])) - db(rms(&tone[after]));
    assert!((ducked + 12.0).abs() < 0.5, "ducked by {ducked:.1} dB");
    assert!(guard.catches() > 1, "caught {} times", guard.catches());
}

// A pitch corrector that hears a howl retunes it, so a loop through a correcting slot can climb
// a semitone at a time while it grows. The guard follows it across each step, more than a bin at
// 1 kHz, and catches it as it would a howl on one pitch: within 400 ms, before it passes -30 dBFS.
#[test]
fn a_howl_a_corrector_moves_a_semitone_at_a_time_is_caught() {
    let frames = 2 * RATE as usize;
    let level = |i: usize| -50.0 + 25.0 * i as f32 / RATE as f32;
    let step = RATE as usize / 10;
    let mut phase = 0.0f32;
    let tone = (0..frames)
        .map(|i| {
            let hz = 1000.0 * 2f32.powf((i / step) as f32 / 12.0);
            phase = (phase + hz / RATE as f32).fract();
            10f32.powf(level(i) / 20.0) * (std::f32::consts::TAU * phase).sin()
        })
        .collect();
    let (_, catches) = guarded_catching(tone);
    let caught = *catches.first().expect("caught");
    let caught_ms = caught as f32 * 1000.0 / RATE as f32;
    println!("caught at {caught_ms:.0} ms, at {:.1} dBFS", level(caught));
    assert!(caught_ms < 400.0 && level(caught) < -30.0, "caught at {caught_ms:.0} ms");
}

// Through a correcting slot a howl can lose its shape for part of each trip round the loop, the
// moments the corrector moves it. A tone that grows as a 2 dB loop does but drops out for 55 ms
// of every 100, three analyses at a time, is still one tone, caught within 600 ms before it passes
// -30 dBFS.
#[test]
fn a_howl_that_drops_out_each_trip_is_caught() {
    let frames = 2 * RATE as usize;
    let level = |i: usize| -50.0 + 25.0 * i as f32 / RATE as f32;
    let trip = RATE as usize / 10;
    let gap = trip * 55 / 100;
    let tone = sine(1200.0, frames, level)
        .into_iter()
        .enumerate()
        .map(|(i, sample)| if i % trip < trip - gap { sample } else { 0.0 })
        .collect();
    let (_, catches) = guarded_catching(tone);
    let caught = *catches.first().expect("caught");
    let caught_ms = caught as f32 * 1000.0 / RATE as f32;
    println!("caught at {caught_ms:.0} ms, at {:.1} dBFS", level(caught));
    assert!(caught_ms < 600.0 && level(caught) < -30.0, "caught at {caught_ms:.0} ms");
}

// A howl can start from a note a tone already held: a whistle at 1.2 kHz held 500 ms at -40 dBFS,
// then a loop taking it a semitone up and growing 25 dB a second from there. The step starts the
// new note's track at once, so the howl is caught within 400 ms of it, before it passes -30 dBFS.
#[test]
fn a_howl_that_starts_from_a_held_note_is_caught() {
    let rate = RATE as usize;
    let step = rate / 2;
    let level =
        |i: usize| if i < step { -40.0 } else { -40.0 + 25.0 * (i - step) as f32 / RATE as f32 };
    let mut phase = 0.0f32;
    let tone = (0..2 * rate)
        .map(|i| {
            let hz = if i < step { 1200.0 } else { 1200.0 * 2f32.powf(1.0 / 12.0) };
            phase = (phase + hz / RATE as f32).fract();
            10f32.powf(level(i) / 20.0) * (std::f32::consts::TAU * phase).sin()
        })
        .collect();
    let (_, catches) = guarded_catching(tone);
    let caught = *catches.first().expect("caught");
    let after_ms = (caught - step) as f32 * 1000.0 / RATE as f32;
    println!("caught {after_ms:.0} ms after the step, at {:.1} dBFS", level(caught));
    assert!(after_ms < 400.0 && level(caught) < -30.0, "caught {after_ms:.0} ms after the step");
}

// A sung vibrato is wider than the quarter semitone a note is held to: 5.5 Hz at 60 and at 100
// cents either way. A soprano's legato phrase with it, A5 to E6 a semitone a note, 400 ms notes
// with 20 ms between, from -30 dBFS up 2 dB a note, is still a melody and is left alone.
#[test]
fn a_soprano_with_a_wide_vibrato_is_left_alone() {
    let notes: Vec<(f32, f32)> =
        (0..8).map(|n| (81.0 + n as f32, -30.0 + 2.0 * n as f32)).collect();
    for cents in [60.0, 100.0] {
        let (_, guard) = guarded(melody(&notes, 400, 20, &[-15.0, -20.0], cents));
        assert_eq!(guard.catches(), 0, "a soprano with a {cents} cent vibrato was ducked");
    }
}

/// A major scale up an octave from `tonic`, one MIDI note a step, `step_db` louder each note from
/// `from_db`.
fn scale_run(tonic: f32, from_db: f32, step_db: f32) -> Vec<(f32, f32)> {
    [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0]
        .iter()
        .enumerate()
        .map(|(n, degree)| (tonic + degree, from_db + step_db * n as f32))
        .collect()
}

// A soprano's melisma: sixteenths at 120 bpm, 125 ms a note, and at 160 bpm, 94 ms, a scale run up
// an octave from A5, legato, from -30 dBFS up 1 dB a note, its second harmonic 15 dB down and its
// third 20. Notes that short never hold a pitch long enough to start afresh, so the guard follows
// the run, and a run climbs in steps, not at a loop's rate. It is left alone.
#[test]
fn a_soprano_melisma_getting_louder_is_left_alone() {
    for note_ms in [125, 94] {
        let run = melody(&scale_run(81.0, -30.0, 1.0), note_ms, 0, &[-15.0, -20.0], 0.0);
        let (_, guard) = guarded(run);
        assert_eq!(guard.catches(), 0, "a melisma of {note_ms} ms notes was ducked");
    }
}

// The same run whistled, C7 to C8, 125 ms and 94 ms a note, from -36 dBFS up 1 dB a note, with no
// harmonic at all, is left alone.
#[test]
fn a_whistled_run_getting_louder_is_left_alone() {
    for note_ms in [125, 94] {
        let run = melody(&scale_run(96.0, -36.0, 1.0), note_ms, 0, &[], 0.0);
        let (_, guard) = guarded(run);
        assert_eq!(guard.catches(), 0, "a whistled run of {note_ms} ms notes was ducked");
    }
}

// The guard's known limit: a run it follows note to note, where no note is held long enough to
// start afresh, and its crescendo counted across the notes. A whistled chromatic run E7 to B7 in
// 94 ms notes, from -36 dBFS up 1 dB a note, is ducked 12 dB once, 682 ms in, and 2 dB a note is
// ducked twice, to 24 dB, from 309 ms. A 50 ms grace note a semitone under a phrase climbing E7 to
// B7 in 250 ms notes, up 2 dB a note, is followed into the whole phrase and ducked once, 832 ms in.
// The guard before ducked none of these, and the issue for real rooms to settle is #139. When a
// change stops these ducks, this test says so and decision 0020's limit is rewritten with it.
#[test]
fn a_fast_run_or_a_phrase_after_a_grace_note_is_ducked() {
    let ducks = |signal: Vec<f32>| {
        let (_, catches) = guarded_catching(signal);
        catches.iter().map(|frame| frame * 1000 / RATE as usize).collect::<Vec<_>>()
    };
    let run = |step_db: f32| -> Vec<(f32, f32)> {
        (0..8).map(|n| (100.0 + n as f32, -36.0 + step_db * n as f32)).collect()
    };
    let one_db = ducks(melody(&run(1.0), 94, 0, &[], 0.0));
    let two_db = ducks(melody(&run(2.0), 94, 0, &[], 0.0));
    let mut phrase = melody(&[(99.0, -36.0)], 50, 0, &[], 0.0);
    phrase.extend(melody(&run(2.0), 250, 0, &[], 0.0));
    let grace = ducks(phrase);
    println!("ducked at {one_db:?}, {two_db:?} and {grace:?} ms");
    assert_eq!((one_db.len(), two_db.len(), grace.len()), (1, 2, 1));
}

// Ducking, the guard is a gain and nothing more: a click after a catch leaves on the very sample
// it entered, only quieter, so it costs the mic path no delay (docs/design.md 8.9).
#[test]
fn a_click_while_ducked_leaves_on_the_sample_it_entered() {
    let rate = RATE as usize;
    let mut signal = sine(1200.0, rate, |i| -50.0 + 25.0 * i as f32 / RATE as f32);
    let click = rate + rate / 5 + 77;
    signal.resize(click + rate / 10, 0.0);
    signal[click] = 0.5;
    let (out, guard) = guarded(signal);
    assert!(guard.catches() >= 1, "the tone was caught first");
    let loudest = (click..out.len()).max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs()));
    assert_eq!(loudest, Some(click));
    assert!(out[click] < 0.5 * 0.6, "ducked: the click came out at {}", out[click]);
}

/// `signal` through a guard under the allocation guard, with the frame at the end of the block
/// each catch landed in.
fn guarded_catching(mut signal: Vec<f32>) -> (Vec<f32>, Vec<usize>) {
    let mut guard = HowlGuard::new(RATE);
    let mut catches = Vec::new();
    for (index, block) in signal.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
        assert_no_alloc(|| guard.process(block));
        if guard.catches() as usize > catches.len() {
            catches.push((index + 1) * BLOCK);
        }
    }
    (signal, catches)
}

// A tone that settled loud, fell 30 dB and stayed down, then grows as a loop does is judged
// from where it fell to, not from where it first settled: the fall itself is no howl, and the
// growth is caught within 400 ms of starting, before it passes -30 dBFS.
#[test]
fn a_tone_that_fell_and_then_grows_is_caught_from_where_it_fell() {
    let rate = RATE as usize;
    let grows_from = 2 * rate;
    let level = |i: usize| match i {
        i if i < rate => -10.0,
        i if i < grows_from => -40.0,
        i => -40.0 + 25.0 * (i - grows_from) as f32 / RATE as f32,
    };
    let (_, catches) = guarded_catching(sine(1200.0, 4 * rate, level));
    let caught = *catches.first().expect("caught");
    assert!(caught > grows_from, "caught at {caught} frames, before the tone grew");
    let after_ms = (caught - grows_from) as f32 * 1000.0 / RATE as f32;
    println!("caught {after_ms:.0} ms after it started to grow, at {:.1} dBFS", level(caught));
    assert!(after_ms < 400.0 && level(caught) < -30.0, "caught {after_ms:.0} ms in");
}

// After a catch the duck holds for half a second, lifts at 3 dB a second to its ceiling 6 dB
// under the gain that howled, and then rises with the ceiling at 0.1 dB a second until the slot
// is at unity again, from where it hands the voice over bit for bit. A quiet steady tone carries
// on after the howl stops, so the gain can be read. Tolerances: 0.1 dB in the hold, 0.5 dB on
// the release, 0.3 dB on the ceiling.
#[test]
fn after_a_catch_the_duck_holds_then_lifts_to_its_ceiling_and_back_to_unity() {
    let rate = RATE as usize;
    let howl = sine(1200.0, rate / 2, |i| -50.0 + 25.0 * i as f32 / RATE as f32);
    let probe = sine(300.0, 70 * rate, |_| -30.0);
    let mut signal = howl;
    signal.extend_from_slice(&probe);
    let input = signal.clone();
    let (out, catches) = guarded_catching(signal);
    assert_eq!(catches.len(), 1, "one catch: {catches:?}");
    let caught = catches[0];
    let gain_at = |seconds: f32| {
        let at = caught + (seconds * RATE as f32) as usize;
        let range = at..at + BLOCK;
        db(rms(&out[range.clone()])) - db(rms(&input[range]))
    };
    let close = |seconds: f32, expected: f32, tolerance: f32| {
        let gain = gain_at(seconds);
        assert!(
            (gain - expected).abs() <= tolerance,
            "{gain:.2} dB at {seconds} s, not {expected}"
        );
    };
    // The probe is under the howl until it stops, so the hold is read once it has
    let stopped = (rate / 2 - caught) as f32 / RATE as f32 + 0.05;
    close(stopped, -12.0, 0.1);
    close(0.45, -12.0, 0.1);
    close(1.5, -9.0, 0.5);
    close(10.0, -5.0, 0.3);
    close(40.0, -2.0, 0.3);
    let unity = caught + 62 * rate;
    assert!(
        out[unity..].iter().zip(&input[unity..]).all(|(o, i)| o.to_bits() == i.to_bits()),
        "not bit for bit 62 s after the catch"
    );
}
