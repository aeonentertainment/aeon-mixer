//! A mic and what the room was played, through the canceller as a slot runs it: the mic's
//! block, then the master's block pushed to the history.

use engine::block::BLOCK;
use kernels::{EchoCanceller, EchoFindings, EchoHistory};

use engine::block::RATE;

pub struct Cancelled {
    pub out: Vec<f32>,
    pub findings: Option<EchoFindings>,
}

pub fn cancel(played: &[f32], mic: &[f32]) -> Cancelled {
    let mut history = EchoHistory::new();
    let silent = EchoHistory::new();
    let mut canceller = EchoCanceller::new(RATE);
    let frames = played.len().min(mic.len()) / BLOCK * BLOCK;
    let mut out = mic[..frames].to_vec();
    for (played, block) in
        played.as_chunks::<BLOCK>().0.iter().zip(out.as_chunks_mut::<BLOCK>().0.iter_mut())
    {
        canceller.process(&history, &silent, 0, block);
        history.push(played, played);
    }
    Cancelled { out, findings: canceller.findings() }
}
