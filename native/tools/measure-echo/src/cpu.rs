//! CPU per slot: thread time over audio time for the canceller on a minute of a room's echo, in
//! the slot's own blocks. It is this machine's number. Build with `--release`.

use cpu_time::ThreadTime;
use engine::block::{BLOCK, RATE};
use kernels::fixtures::{at_db, pink_noise, room, talker, through};
use kernels::{EchoCanceller, EchoHistory};

const SECONDS: usize = 60;
const REPEATS: usize = 5;

pub struct Cpu {
    /// Percent of one core, the median of `REPEATS` runs.
    pub canceller: f64,
    pub block_micros: f64,
}

pub fn run() -> Cpu {
    let frames = SECONDS * RATE as usize;
    let far: Vec<f32> = at_db(talker(frames, 110.0, 3), -18.0)
        .iter()
        .zip(&at_db(pink_noise(frames, 7), -26.0))
        .map(|(v, b)| v + b)
        .collect();
    let echo = through(&far, &room(40 * 48, 0.316, 0.3, 0.25, 11));
    let mic: Vec<f32> =
        echo.iter().zip(&at_db(talker(frames, 180.0, 9), -20.0)).map(|(e, v)| e + v).collect();
    let percent = |seconds: f64| seconds / SECONDS as f64 * 100.0;

    let canceller = median((0..REPEATS).map(|_| {
        let mut history = EchoHistory::new();
        let silent = EchoHistory::new();
        let mut canceller = EchoCanceller::new(RATE);
        let mut block = [0.0; BLOCK];
        let started = ThreadTime::now();
        for (played, heard) in
            far.as_chunks::<BLOCK>().0.iter().zip(mic.as_chunks::<BLOCK>().0.iter())
        {
            block.copy_from_slice(heard);
            canceller.process(&history, &silent, 0, &mut block);
            history.push(played, played);
        }
        percent(started.elapsed().as_secs_f64())
    }));
    let block_seconds = BLOCK as f64 / f64::from(RATE);
    Cpu { canceller, block_micros: canceller / 100.0 * block_seconds * 1e6 }
}

fn median(runs: impl Iterator<Item = f64>) -> f64 {
    let mut runs: Vec<f64> = runs.collect();
    runs.sort_by(f64::total_cmp);
    runs[runs.len() / 2]
}
