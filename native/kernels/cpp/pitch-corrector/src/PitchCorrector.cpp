#include "aeon/dsp/PitchCorrector.h"

#include "aeon/dsp/PitchShifter.h"
#include "aeon/dsp/YinDetector.h"

#include <algorithm>
#include <cmath>
#include <vector>

namespace aeon::dsp {

namespace {

constexpr double kHopSeconds = 0.010;
/** YIN over 2048 samples at 48 kHz, the #87 choice, scaled to the stream's rate. */
constexpr double kWindowSeconds = 2048.0 / 48000;
constexpr double kCrossfadeSeconds = 0.020;
/** A reading this sure is a voice to correct: YIN's dip under its threshold. */
constexpr float kVoicedConfidence = 0.9f;
/** E2, the bottom of a bass's range. A lower voice would need jumps past the 13.5 ms ceiling, so it passes uncorrected. */
constexpr float kLowestCorrectedHz = 82.0f;
/** Readings a held correction survives without a voice, so one dropped frame mid-note does not let go. */
constexpr int kUnvoicedHold = 3;
/** Semitones past the midpoint between two classes before the target changes class, so vibrato there cannot flip it. */
constexpr float kHysteresis = 0.2f;
constexpr int kNoTarget = -1;

int powerOfTwoNear(double samples) {
  return 1 << std::max(4, int(std::lround(std::log2(samples))));
}

}  // namespace

struct PitchCorrector::State {
  double rate;
  int hop;
  YinDetector detector;
  PitchShifter shifter;
  std::vector<float> history;
  std::vector<float> window;
  std::vector<float> wet;
  int historyPos = 0;
  int hopFill = 0;

  PitchClassMask mask = kEveryPitchClass;
  float strength = 0;
  float retuneMs = 0;

  Pitch pitch;
  float midi = 0;
  bool voiced = false;
  int unvoiced = 0;
  int targetMidi = kNoTarget;
  float error = 0;
  float targetShift = 0;
  float shift = 0;
  float wetGain = 0;
  float crossfadeStep;

  explicit State(double sampleRate)
      : rate(sampleRate),
        hop(std::max(1, int(std::lround(sampleRate * kHopSeconds)))),
        detector(sampleRate, powerOfTwoNear(sampleRate * kWindowSeconds)),
        shifter(sampleRate),
        history(size_t(detector.windowSamples())),
        window(size_t(detector.windowSamples())),
        wet(size_t(hop)),
        crossfadeStep(float(1.0 / (sampleRate * kCrossfadeSeconds))) {}

  /** The class nearest `m` in the mask, keeping the current one until `m` is clearly closer to another. */
  int nearestTarget(float m) const {
    int best = kNoTarget;
    float bestDistance = 0;
    const int base = int(std::lround(m));
    for (int k = -6; k <= 6; ++k) {
      const int candidate = base + k;
      if (!(mask & pitchClassOf(candidate))) continue;
      const float distance = std::abs(float(candidate) - m);
      if (best == kNoTarget || distance < bestDistance) {
        best = candidate;
        bestDistance = distance;
      }
    }
    if (best != kNoTarget && targetMidi != kNoTarget && (mask & pitchClassOf(targetMidi)) &&
        std::abs(float(targetMidi) - m) < bestDistance + 2 * kHysteresis) {
      return targetMidi;
    }
    return best;
  }

  void retarget() {
    targetMidi = voiced ? nearestTarget(midi) : kNoTarget;
    error = targetMidi == kNoTarget ? 0 : float(targetMidi) - midi;
    targetShift = strength * error;
  }

  void read() {
    for (int i = 0; i < detector.windowSamples(); ++i) {
      window[size_t(i)] = history[size_t((historyPos + i) % detector.windowSamples())];
    }
    pitch = detector.analyze(window.data());
    if (pitch.hz >= kLowestCorrectedHz && pitch.confidence >= kVoicedConfidence) {
      midi = midiOf(pitch.hz);
      voiced = true;
      unvoiced = 0;
      shifter.setPeriod(float(rate / pitch.hz));
    } else if (++unvoiced > kUnvoicedHold) {
      voiced = false;
    }
    shifter.setVoiced(voiced);
    retarget();
  }
};

PitchCorrector::PitchCorrector(double sampleRate) : state_(std::make_unique<State>(sampleRate)) {}

PitchCorrector::~PitchCorrector() = default;

void PitchCorrector::reset() {
  State &s = *state_;
  std::fill(s.history.begin(), s.history.end(), 0.0f);
  s.historyPos = 0;
  s.hopFill = 0;
  s.pitch = {};
  s.voiced = false;
  s.unvoiced = 0;
  s.targetMidi = kNoTarget;
  s.error = 0;
  s.targetShift = 0;
  s.shift = 0;
  // Back to the dry input, crossfading into correction again as on a fresh start
  s.wetGain = 0;
  s.shifter.reset();
}

void PitchCorrector::setTarget(PitchClassMask mask, float strength, float retuneMs) {
  State &s = *state_;
  s.mask = mask & kEveryPitchClass;
  s.strength = std::clamp(strength, 0.0f, 1.0f);
  s.retuneMs = std::max(0.0f, retuneMs);
  // A new target counts from the last reading at once, so a guide boundary lands where the host put it
  s.retarget();
}

void PitchCorrector::process(const float *in, float *out, int frames) {
  State &s = *state_;
  const int windowSamples = s.detector.windowSamples();
  const float wetTarget = s.strength > 0 ? 1.0f : 0.0f;
  int done = 0;
  while (done < frames) {
    const int n = std::min(frames - done, s.hop - s.hopFill);
    for (int i = 0; i < n; ++i) {
      s.history[size_t(s.historyPos)] = in[done + i];
      s.historyPos = (s.historyPos + 1) % windowSamples;
    }

    const double easeSamples = s.retuneMs * s.rate / 1000.0;
    s.shift = easeSamples <= 0 ? s.targetShift : s.shift + (s.targetShift - s.shift) * float(1 - std::exp(-n / easeSamples));
    s.shifter.setSemitones(s.shift);
    s.shifter.process(in + done, s.wet.data(), n);

    for (int i = 0; i < n; ++i) {
      s.wetGain = wetTarget > s.wetGain ? std::min(wetTarget, s.wetGain + s.crossfadeStep) : std::max(wetTarget, s.wetGain - s.crossfadeStep);
      const float dry = in[done + i];
      out[done + i] = dry + s.wetGain * (s.wet[size_t(i)] - dry);
    }

    s.hopFill += n;
    if (s.hopFill == s.hop) {
      s.hopFill = 0;
      s.read();
    }
    done += n;
  }
}

Pitch PitchCorrector::detected() const { return state_->pitch; }

int PitchCorrector::framesUntilReading() const { return state_->hop - state_->hopFill; }

int PitchCorrector::readingLagSamples() const { return state_->detector.windowSamples() / 2; }

float PitchCorrector::errorSemitones() const { return state_->error; }

int PitchCorrector::latencySamples() const {
  const State &s = *state_;
  return s.wetGain > 0 ? int(std::lround(s.shifter.delaySamples())) : 0;
}

}  // namespace aeon::dsp
