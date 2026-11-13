//! A port of SpeexDSP's multidelay block frequency-domain echo canceller, `libspeexdsp/mdf.c`
//! at 7a158783df74efe7c2d1c6ee8363c1e695c71226, float path, one mic and one speaker. It keeps
//! the alternately updated MDF, the two-path filter and Valin's learning rate, which adapts with
//! no explicit double-talk detector. What it leaves out: the DC notch, since an Opus or PCM slot
//! carries no DC, and the playback buffer, since the reference here is the master the engine
//! itself rendered.
//!
//! Four changes, each explained where it is made: the sanity check's threshold in `process`, a
//! frame whose mic does not hear the room, also in `process`, where the adaptation starts in
//! `reset`, and what it adapts on. The echo estimate convolves the whole reference, but the
//! gradient, and the power that normalises it, see the reference less the mic's own voice: a
//! singer's voice is in the master they hear, and a filter adapted on it learns to predict the
//! singer from their own past, which a sung vowel makes easy, and takes the singer out with the
//! echo.

// Copyright (C) 2003-2008 Jean-Marc Valin
//
// Redistribution and use in source and binary forms, with or without modification, are
// permitted provided that the following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of
//    conditions and the following disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of
//    conditions and the following disclaimer in the documentation and/or other materials
//    provided with the distribution.
//
// 3. The name of the author may not be used to endorse or promote products derived from this
//    software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
// PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY DIRECT,
// INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR
// BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
// STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use realfft::num_complex::Complex32;

use super::fft::{Fft, SILENT, Spectrum};
use super::{BINS, FRAME, WINDOW};

const PREEMPH: f32 = 0.9;
const MIN_LEAK: f32 = 0.005;
const VAR1_SMOOTH: f32 = 0.36;
const VAR2_SMOOTH: f32 = 0.7225;
const VAR1_UPDATE: f32 = 0.5;
const VAR2_UPDATE: f32 = 0.25;
const VAR_BACKTRACK: f32 = 4.0;
/// A mic carrying a tenth of the echo the filter expects, or less, does not hear the room...
const UNHEARD: f32 = 10.0;
/// ...unless what it carries has the echo's shape: this share of its energy, pre-emphasised, lies
/// along the estimate's.
const COHERENT: f32 = 0.5;
/// Under one 16-bit step, RMS over the frame, a mic carries nothing at all.
const QUIETEST_MIC: f32 = FRAME as f32;
const N: f32 = WINDOW as f32;

/// mdf.c's state for one mic. Signals are on its 16-bit scale, full scale 32768. The caller
/// hands each frame's reference window, pre-emphasised, already aligned to the echo's delay.
pub struct Mdf {
    partitions: usize,
    fft: Fft,
    /// Reference spectra, newest first: `partitions + 1` of them, for the gradient's one-frame lag.
    x: Vec<Spectrum>,
    /// The same of the reference less the mic's own voice, which the gradient adapts on.
    others: Vec<Spectrum>,
    background: Vec<Spectrum>,
    foreground: Vec<Spectrum>,
    prop: Vec<f32>,
    window: [f32; WINDOW],
    input: [f32; FRAME],
    e: [f32; WINDOW],
    y: [f32; WINDOW],
    wtmp: [f32; WINDOW],
    big_e: Spectrum,
    big_y: Spectrum,
    phi: Spectrum,
    power: [f32; BINS],
    power_1: [f32; BINS],
    rf: [f32; BINS],
    yf: [f32; BINS],
    xf: [f32; BINS],
    eh: [f32; BINS],
    yh: [f32; BINS],
    spec_average: f32,
    beta0: f32,
    beta_max: f32,
    /// `e^(-2 pi i k / WINDOW)` for each bin `k`, which turns a window's spectrum as what it
    /// holds moves a sample along it.
    turn: Spectrum,
    state: State,
}

/// What `reset` returns to.
#[derive(Clone, Copy)]
struct State {
    cancel_count: u64,
    saturated: u32,
    screwed_up: u32,
    adapted: bool,
    sum_adapt: f32,
    leak_estimate: f32,
    pey: f32,
    pyy: f32,
    davg1: f32,
    davg2: f32,
    dvar1: f32,
    dvar2: f32,
    mem_d: f32,
    mem_e: f32,
}

const FRESH: State = State {
    cancel_count: 0,
    saturated: 0,
    screwed_up: 0,
    adapted: false,
    sum_adapt: 0.0,
    leak_estimate: 0.0,
    pey: 1.0,
    pyy: 1.0,
    davg1: 0.0,
    davg2: 0.0,
    dvar1: 0.0,
    dvar2: 0.0,
    mem_d: 0.0,
    mem_e: 0.0,
};

/// What one frame told the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    /// The filter has adapted far enough for its echo estimate to mean something.
    pub adapted: bool,
    /// mdf.c's sanity check failed for long enough that the filter was reset.
    pub reset: bool,
    /// The mic did not hear the room, so the frame passed untouched and the filter learned
    /// nothing from it.
    pub unheard: bool,
}

impl Mdf {
    pub fn new(rate: u32, partitions: usize) -> Self {
        let mut mdf = Self {
            partitions,
            fft: Fft::new(),
            x: vec![SILENT; partitions + 1],
            others: vec![SILENT; partitions + 1],
            background: vec![SILENT; partitions],
            foreground: vec![SILENT; partitions],
            prop: vec![0.0; partitions],
            window: std::array::from_fn(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N).cos()
            }),
            input: [0.0; FRAME],
            e: [0.0; WINDOW],
            y: [0.0; WINDOW],
            wtmp: [0.0; WINDOW],
            big_e: SILENT,
            big_y: SILENT,
            phi: SILENT,
            power: [0.0; BINS],
            power_1: [1.0; BINS],
            rf: [0.0; BINS],
            yf: [0.0; BINS],
            xf: [0.0; BINS],
            eh: [0.0; BINS],
            yh: [0.0; BINS],
            spec_average: FRAME as f32 / rate as f32,
            beta0: 2.0 * FRAME as f32 / rate as f32,
            beta_max: 0.5 * FRAME as f32 / rate as f32,
            turn: std::array::from_fn(|k| {
                Complex32::from_polar(1.0, -2.0 * std::f32::consts::PI * k as f32 / N)
            }),
            state: FRESH,
        };
        mdf.reset();
        mdf
    }

    /// Forgets the echo path: what a new speaker-to-mic path, or a moved reference, needs.
    pub fn reset(&mut self) {
        self.state = FRESH;
        let spectra = self.x.iter_mut().chain(&mut self.others);
        for spectrum in spectra.chain(&mut self.background).chain(&mut self.foreground) {
            *spectrum = SILENT;
        }
        self.e = [0.0; WINDOW];
        self.y = [0.0; WINDOW];
        self.power = [0.0; BINS];
        self.power_1 = [1.0; BINS];
        self.eh = [0.0; BINS];
        self.yh = [0.0; BINS];
        // mdf.c falls 10 to 1 across the whole tail, for an echo that could be anywhere in it. Here
        // the delay finder has put the echo at the front, so the start falls 10 to 1 across the
        // first eight partitions, 21 ms, and the tail waits for the proportionate update
        let decay = (-0.3f32).exp();
        self.prop[0] = 0.7;
        for i in 1..self.partitions {
            self.prop[i] = self.prop[i - 1] * decay;
        }
        let sum: f32 = self.prop.iter().sum();
        self.prop.iter_mut().for_each(|p| *p = 0.8 * *p / sum);
    }

    /// Replaces the reference spectra the filter convolves with: `window(j, whole, others)`
    /// writes the pre-emphasised windows of the frame `j + 1` frames before the next one. For a
    /// reference that moved while the echo path did not, so the filter keeps what it learned.
    pub fn refill(
        &mut self,
        mut window: impl FnMut(usize, &mut [f32; WINDOW], &mut [f32; WINDOW]),
    ) {
        let (mut whole, mut others) = ([0.0; WINDOW], [0.0; WINDOW]);
        for j in 0..=self.partitions {
            window(j, &mut whole, &mut others);
            self.fft.forward(&whole, &mut self.x[j]);
            self.fft.forward(&others, &mut self.others[j]);
        }
    }

    /// The reference the filter convolves with moved by a sample, one older when `older`, as a
    /// source that splices a sample moves its echo, so every window it holds slides by one.
    /// `edges(j)` gives, for the windows of the frame `j + 1` frames before the next one, the
    /// sample that enters each less the one that leaves it, pre-emphasised: of the whole
    /// reference and of the others. Exact, where `refill` reads and transforms every window
    /// again at a hundred times the cost.
    pub fn slide(&mut self, older: bool, mut edges: impl FnMut(usize) -> (f32, f32)) {
        let x = self.x.iter_mut().zip(&mut self.others).enumerate();
        for (j, (whole, others)) in x {
            let (whole_edge, others_edge) = edges(j);
            for (spectrum, edge) in [(whole, whole_edge), (others, others_edge)] {
                let edge = edge / N;
                for (bin, turn) in spectrum.iter_mut().zip(&self.turn) {
                    // A window a sample older is this one turned a sample on, with the sample
                    // before it in and its newest out; one newer, the reverse
                    *bin = if older { turn * *bin + edge } else { turn.conj() * (*bin + edge) };
                }
            }
        }
    }

    /// Lets the filter run without learning for `frames` frames: while the reference may be
    /// misaligned, what it would learn is wrong.
    pub fn hold(&mut self, frames: u32) {
        self.state.saturated = self.state.saturated.max(frames);
    }

    /// One frame. `mic` and `out` are on the 16-bit scale; `reference` is this frame's window,
    /// the frame before and this one, pre-emphasised, and `others` the same less the mic's own
    /// voice.
    pub fn process(
        &mut self,
        mic: &[f32; FRAME],
        reference: &[f32; WINDOW],
        others: &[f32; WINDOW],
        out: &mut [f32; FRAME],
    ) -> Frame {
        let m = self.partitions;
        let s = &mut self.state;
        s.cancel_count += 1;
        let ss = 0.35 / m as f32;
        let ss_1 = 1.0 - ss;
        // What the last frame took out of its last sample, where this frame's echo goes on from
        let echo_before = s.mem_d - s.mem_e;

        for (input, &sample) in self.input.iter_mut().zip(mic) {
            *input = sample - PREEMPH * s.mem_d;
            s.mem_d = sample;
        }

        self.x.rotate_right(1);
        self.fft.forward(reference, &mut self.x[0]);
        self.others.rotate_right(1);
        self.fft.forward(others, &mut self.others[0]);
        // mdf.c sums the reference's energy twice, and its constants are tuned to that
        let mut sxx = inner(&others[FRAME..], &others[FRAME..]);

        // The foreground filter's echo estimate and error
        spectral_mul_accum(&self.x[..m], &self.foreground, &mut self.big_y);
        self.fft.inverse(&self.big_y, &mut self.e);
        for i in 0..FRAME {
            self.e[i] = self.input[i] - self.e[i + FRAME];
        }
        let sff = inner(&self.e[..FRAME], &self.e[..FRAME]);

        // A mic that carries nothing at all, or a tenth of the echo the filter expects and little
        // of its shape, is not hearing the room: its source stopped, lost its input or was muted.
        // There is nothing to take out and nothing to learn, so the frame passes untouched, the
        // filter keeps the path for when the room is heard again, and the sanity check starts over.
        // mdf.c subtracts its estimate from the silence and hands the echo over inverted, then
        // resets the filter after 50 frames, and its unadapted rate, at its highest on a silent
        // mic, soon calls a filter that learned nothing adapted. A room turned down keeps the
        // echo's shape, and is learned again as mdf.c does. The levels are weighed as the slot
        // hears them, since the pre-emphasis would count the edge of a mic that just stopped as a
        // click; the shape pre-emphasised, as the filter sees it, since in a frame of 2.67 ms the
        // low notes of any two signals look alike
        let (mut echo, mut expected, mut along) = (echo_before, 0.0, 0.0);
        for (&estimate, &input) in self.e[FRAME..].iter().zip(&self.input) {
            echo = estimate + PREEMPH * echo;
            expected += echo * echo;
            along += estimate * input;
        }
        let heard = inner(mic, mic);
        let shape = inner(&self.input, &self.input) * inner(&self.e[FRAME..], &self.e[FRAME..]);
        let lost = UNHEARD * heard < expected && along * along < COHERENT * shape;
        if heard < QUIETEST_MIC || lost {
            out.copy_from_slice(mic);
            s.mem_e = mic[FRAME - 1];
            s.screwed_up = 0;
            // The next frame's gradient has no error of this one's to follow
            self.big_e = SILENT;
            return Frame { adapted: s.adapted, reset: false, unheard: true };
        }

        if s.adapted {
            adjust_prop(&self.background, &mut self.prop);
        }
        if s.saturated == 0 {
            for j in (0..m).rev() {
                weighted_spectral_mul_conj(
                    &self.power_1,
                    self.prop[j],
                    &self.others[j + 1],
                    &self.big_e,
                    &mut self.phi,
                );
                for (w, phi) in self.background[j].iter_mut().zip(&self.phi) {
                    *w += phi;
                }
            }
        } else {
            s.saturated -= 1;
        }

        // Alternately updated MDF: constrain the first partition and one other per frame
        for j in 0..m {
            if j == 0 || (m > 1 && s.cancel_count % (m as u64 - 1) == j as u64 - 1) {
                self.fft.inverse(&self.background[j], &mut self.wtmp);
                self.wtmp[FRAME..].fill(0.0);
                self.fft.forward(&self.wtmp, &mut self.background[j]);
            }
        }

        self.rf = [0.0; BINS];
        self.yf = [0.0; BINS];
        self.xf = [0.0; BINS];

        // The background filter's estimate, and how far it is from the foreground's
        spectral_mul_accum(&self.x[..m], &self.background, &mut self.big_y);
        self.fft.inverse(&self.big_y, &mut self.y);
        for i in 0..FRAME {
            self.e[i] = self.e[i + FRAME] - self.y[i + FRAME];
        }
        let dbf = 10.0 + inner(&self.e[..FRAME], &self.e[..FRAME]);
        for i in 0..FRAME {
            self.e[i] = self.input[i] - self.y[i + FRAME];
        }
        let mut see = inner(&self.e[..FRAME], &self.e[..FRAME]);

        // Copy the background to the foreground when it is significantly better, or back when
        // it is significantly worse
        let diff = sff - see;
        s.davg1 = 0.6 * s.davg1 + 0.4 * diff;
        s.davg2 = 0.85 * s.davg2 + 0.15 * diff;
        s.dvar1 = VAR1_SMOOTH * s.dvar1 + (0.4 * sff) * (0.4 * dbf);
        s.dvar2 = VAR2_SMOOTH * s.dvar2 + (0.15 * sff) * (0.15 * dbf);
        let update_foreground = diff * diff.abs() > sff * dbf
            || s.davg1 * s.davg1.abs() > VAR1_UPDATE * s.dvar1
            || s.davg2 * s.davg2.abs() > VAR2_UPDATE * s.dvar2;
        if update_foreground {
            s.davg1 = 0.0;
            s.davg2 = 0.0;
            s.dvar1 = 0.0;
            s.dvar2 = 0.0;
            self.foreground.copy_from_slice(&self.background);
            // A smooth transition, so the switch leaves no block edge
            for i in 0..FRAME {
                self.e[i + FRAME] =
                    self.window[i + FRAME] * self.e[i + FRAME] + self.window[i] * self.y[i + FRAME];
            }
        } else {
            let reset_background = -diff * diff.abs() > VAR_BACKTRACK * (sff * dbf)
                || -s.davg1 * s.davg1.abs() > VAR_BACKTRACK * s.dvar1
                || -s.davg2 * s.davg2.abs() > VAR_BACKTRACK * s.dvar2;
            if reset_background {
                self.background.copy_from_slice(&self.foreground);
                for i in 0..FRAME {
                    self.y[i + FRAME] = self.e[i + FRAME];
                    self.e[i] = self.input[i] - self.y[i + FRAME];
                }
                see = sff;
                s.davg1 = 0.0;
                s.davg2 = 0.0;
                s.dvar1 = 0.0;
                s.dvar2 = 0.0;
            }
        }

        // The output: the mic less the foreground's estimate, de-emphasised
        for i in 0..FRAME {
            let sample = self.input[i] - self.e[i + FRAME] + PREEMPH * s.mem_e;
            if (mic[i] <= -32000.0 || mic[i] >= 32000.0) && s.saturated == 0 {
                s.saturated = 1;
            }
            out[i] = sample;
            s.mem_e = sample;
        }

        // The error the next frame's gradient uses
        for i in 0..FRAME {
            self.e[i + FRAME] = self.e[i];
            self.e[i] = 0.0;
        }
        let sey = inner(&self.e[FRAME..], &self.y[FRAME..]);
        let syy = inner(&self.y[FRAME..], &self.y[FRAME..]);
        let sdd = inner(&self.input, &self.input);
        self.fft.forward(&self.e, &mut self.big_e);
        self.y[..FRAME].fill(0.0);
        self.fft.forward(&self.y, &mut self.big_y);
        power_spectrum_accum(&self.big_e, &mut self.rf);
        power_spectrum_accum(&self.big_y, &mut self.yf);

        let mut frame = Frame::default();
        let sane = syy >= 0.0 && sxx >= 0.0 && see >= 0.0;
        if !(sane && sff < N * 1e9 && syy < N * 1e9 && sxx < N * 1e9) {
            // Things have gone really bad
            s.screwed_up += 50;
            out.fill(0.0);
        } else if sff > 4.0 * sdd + N * 10000.0 {
            // Adding echo instead of removing it; see whether it improves. mdf.c's fixed-point
            // build asks for the error to be 6 dB over the mic, and its float build, where the
            // shift is a no-op, for any excess, which a singer over the echo trips
            s.screwed_up += 1;
        } else {
            s.screwed_up = 0;
        }
        if s.screwed_up >= 50 {
            self.reset();
            frame.reset = true;
            return frame;
        }

        see = see.max(N * 100.0);
        sxx += inner(&others[FRAME..], &others[FRAME..]);
        power_spectrum_accum(&self.others[0], &mut self.xf);
        for j in 0..BINS {
            self.power[j] = ss_1 * self.power[j] + 1.0 + ss * self.xf[j];
        }

        let (mut pey, mut pyy) = (1.0f32, 1.0f32);
        for j in (0..BINS).rev() {
            let eh = self.rf[j] - self.eh[j];
            let yh = self.yf[j] - self.yh[j];
            pey += eh * yh;
            pyy += yh * yh;
            self.eh[j] = (1.0 - self.spec_average) * self.eh[j] + self.spec_average * self.rf[j];
            self.yh[j] = (1.0 - self.spec_average) * self.yh[j] + self.spec_average * self.yf[j];
        }
        let pyy = pyy.sqrt();
        let pey = pey / pyy;

        let alpha = (self.beta0 * syy).min(self.beta_max * see) / see;
        let alpha_1 = 1.0 - alpha;
        s.pey = alpha_1 * s.pey + alpha * pey;
        s.pyy = alpha_1 * s.pyy + alpha * pyy;
        s.pyy = s.pyy.max(1.0);
        // mdf.c does not hope for better than about 23 dB from the leakage estimate
        s.pey = s.pey.max(MIN_LEAK * s.pyy).min(s.pyy);
        s.leak_estimate = s.pey / s.pyy;

        // Residual to error ratio, bounded below by the echo still in the error
        let mut rer = (0.0001 * sxx + 3.0 * s.leak_estimate * syy) / see;
        rer = rer.max(sey * sey / (1.0 + see * syy)).min(0.5);

        if !s.adapted && s.sum_adapt > m as f32 && s.leak_estimate * syy > 0.03 * syy {
            s.adapted = true;
        }
        if s.adapted {
            for i in 0..BINS {
                let e = self.rf[i] + 1.0;
                let r = (s.leak_estimate * self.yf[i]).min(0.5 * e);
                let r = 0.7 * r + 0.3 * rer * e;
                self.power_1[i] = r / (e * (self.power[i] + 10.0));
            }
        } else {
            let mut adapt_rate = 0.0;
            if sxx > N * 1000.0 {
                adapt_rate = (0.25 * sxx).min(0.25 * see) / see;
            }
            for i in 0..BINS {
                self.power_1[i] = adapt_rate / (self.power[i] + 10.0);
            }
            s.sum_adapt += adapt_rate;
        }
        frame.adapted = s.adapted;
        frame
    }
}

fn inner(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn spectral_mul_accum(x: &[Spectrum], w: &[Spectrum], acc: &mut Spectrum) {
    *acc = SILENT;
    for (x, w) in x.iter().zip(w) {
        for ((acc, x), w) in acc.iter_mut().zip(x).zip(w) {
            *acc += x * w;
        }
    }
}

fn weighted_spectral_mul_conj(
    power_1: &[f32; BINS],
    prop: f32,
    x: &Spectrum,
    e: &Spectrum,
    out: &mut Spectrum,
) {
    for (((out, power), x), e) in out.iter_mut().zip(power_1).zip(x).zip(e) {
        *out = (x.conj() * e) * (prop * power);
    }
}

fn power_spectrum_accum(x: &Spectrum, acc: &mut [f32; BINS]) {
    for (acc, x) in acc.iter_mut().zip(x) {
        *acc += x.norm_sqr();
    }
}

/// Proportionate adaptation: partitions holding more of the echo path adapt faster.
fn adjust_prop(w: &[Spectrum], prop: &mut [f32]) {
    let mut max_sum = 1.0f32;
    for (prop, w) in prop.iter_mut().zip(w) {
        // mdf.c sums the packed real spectrum's squares, which counts each complex bin once
        let tmp = 1.0 + w.iter().map(Complex32::norm_sqr).sum::<f32>();
        *prop = tmp.sqrt();
        max_sum = max_sum.max(*prop);
    }
    let mut prop_sum = 1.0;
    for prop in prop.iter_mut() {
        *prop += 0.1 * max_sum;
        prop_sum += *prop;
    }
    prop.iter_mut().for_each(|p| *p = 0.99 * *p / prop_sum);
}
