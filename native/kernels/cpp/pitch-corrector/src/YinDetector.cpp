#include "aeon/dsp/YinDetector.h"

#include "signalsmith-linear/fft.h"

#include <algorithm>
#include <cmath>
#include <complex>
#include <vector>

namespace aeon::dsp {

namespace {

/** The first dip under this in the normalised difference function is the period; the paper's and librosa's 0.1. */
constexpr float kThreshold = 0.1f;
/** Mean square under this (-60 dBFS) is silence, whatever the difference function says. */
constexpr float kSilence = 1e-6f;

}  // namespace

struct YinDetector::State {
  double sampleRate;
  int window;
  int tauMin;
  int tauMax;
  signalsmith::linear::RealFFT<float> fft;
  std::vector<float> padded;
  std::vector<float> autocorrelation;
  std::vector<float> difference;
  std::vector<float> trueDifference;
  std::vector<float> perPair;
  std::vector<std::complex<float>> spectrum;

  State(double rate, int windowSamples, float minHz, float maxHz)
      : sampleRate(rate),
        window(windowSamples),
        tauMin(std::max(2, int(rate / maxHz))),
        tauMax(std::min(windowSamples - 2, int(std::ceil(rate / minHz)))),
        fft(size_t(2 * windowSamples)),
        padded(size_t(2 * windowSamples)),
        autocorrelation(size_t(2 * windowSamples)),
        difference(size_t(tauMax) + 2),
        trueDifference(size_t(tauMax) + 2),
        perPair(size_t(tauMax) + 2),
        spectrum(size_t(windowSamples)) {}
};

YinDetector::YinDetector(double sampleRate, int windowSamples, float minHz, float maxHz)
    : state_(std::make_unique<State>(sampleRate, windowSamples, minHz, maxHz)) {}

YinDetector::~YinDetector() = default;

int YinDetector::windowSamples() const { return state_->window; }

// The whole-window form #87 measured (librosa's yin) picks the period: the autocorrelation r
// over all of the window, zero-padded so nothing wraps, and d(tau) = 2 (r(0) - r(tau)) - sum
// over m < tau of x(m)^2. Integrating over the whole window rather than half of it is what keeps
// a strong second harmonic from passing for the pitch. That form also counts the window's last
// tau samples, which have no partner at that lag, so it scores a long period as less periodic
// than it is: the confidence comes from the true difference, with that tail taken out.
Pitch YinDetector::analyze(const float *x) {
  State &s = *state_;
  const int n = s.window;

  std::copy(x, x + n, s.padded.begin());
  std::fill(s.padded.begin() + n, s.padded.end(), 0.0f);
  s.fft.fft(s.padded.data(), s.spectrum.data());
  // Bin 0 packs DC as its real part and Nyquist as its imaginary part, both real
  s.spectrum[0] = {s.spectrum[0].real() * s.spectrum[0].real(), s.spectrum[0].imag() * s.spectrum[0].imag()};
  for (size_t k = 1; k < s.spectrum.size(); ++k) s.spectrum[k] = std::norm(s.spectrum[k]);
  s.fft.ifft(s.spectrum.data(), s.autocorrelation.data());
  const float unscale = 1.0f / float(2 * n);
  const float energy = s.autocorrelation[0] * unscale;
  if (energy / float(n) < kSilence) return {};

  // The cumulative mean normalisation makes d'(0) 1 and a perfect period dip to 0
  float head = 0;
  float tail = 0;
  float running = 0;
  float trueRunning = 0;
  s.difference[0] = 1;
  s.trueDifference[0] = 1;
  s.perPair[0] = 0;
  for (int tau = 1; tau <= s.tauMax; ++tau) {
    head += x[tau - 1] * x[tau - 1];
    tail += x[n - tau] * x[n - tau];
    const float d = std::max(0.0f, 2 * (energy - s.autocorrelation[size_t(tau)] * unscale) - head);
    const float trueD = std::max(0.0f, d - tail);
    running += d;
    trueRunning += trueD;
    s.perPair[size_t(tau)] = trueD / float(n - tau);
    s.difference[size_t(tau)] = running > 0 ? d * float(tau) / running : 1;
    s.trueDifference[size_t(tau)] = trueRunning > 0 ? trueD * float(tau) / trueRunning : 1;
  }

  // The first dip under the threshold; when there is none, the deepest, with its low confidence
  int tau = s.tauMin;
  while (tau < s.tauMax && s.difference[size_t(tau)] >= kThreshold) ++tau;
  if (tau < s.tauMax) {
    while (tau + 1 < s.tauMax && s.difference[size_t(tau) + 1] < s.difference[size_t(tau)]) ++tau;
  } else {
    tau = int(std::min_element(s.difference.begin() + s.tauMin, s.difference.begin() + s.tauMax) - s.difference.begin());
  }

  // The tail and the running mean that find the dip also lean it toward shorter periods, by up to
  // 4 cents in a short window. The difference per paired sample has neither, so the dip's true
  // bottom and the parabola through it for a period finer than one sample come from that
  const auto paired = [&s](int at) { return s.perPair[size_t(at)]; };
  const int confidenceAt = tau;
  while (tau > s.tauMin && paired(tau - 1) < paired(tau)) --tau;
  while (tau + 1 <= s.tauMax && paired(tau + 1) < paired(tau)) ++tau;
  float offset = 0;
  if (tau > s.tauMin && tau + 1 <= s.tauMax) {
    const float curvature = paired(tau - 1) - 2 * paired(tau) + paired(tau + 1);
    if (curvature > 0) offset = std::clamp(0.5f * (paired(tau - 1) - paired(tau + 1)) / curvature, -1.0f, 1.0f);
  }
  return {float(s.sampleRate / (float(tau) + offset)), std::clamp(1.0f - s.trueDifference[size_t(confidenceAt)], 0.0f, 1.0f)};
}

}  // namespace aeon::dsp
