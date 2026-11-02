#include "aeon/dsp/PitchShifter.h"

#include <algorithm>
#include <cmath>
#include <vector>

namespace aeon::dsp {

namespace {

constexpr float kMaxSemitones = 6;
/** The crossfade a jump hides under: 128 samples at 48 kHz. */
constexpr double kCrossfadeSeconds = 128.0 / 48000;
constexpr double kShortestPeriodSeconds = 1.0 / 1100;
/** E2, the bottom of a bass's range: the longest jump, so the delay stays inside 15 ms. */
constexpr double kLongestPeriodSeconds = 1.0 / 82;
/** Until the detector has heard a voice there is nothing to jump over; any plausible period will do. */
constexpr double kStartingPeriodSeconds = 1.0 / 200;
/** Cubic interpolation reads one sample behind and two ahead of the read point. */
constexpr int kInterpolationReach = 3;

int powerOfTwoAtLeast(double samples) {
  int size = 16;
  while (size < samples) size *= 2;
  return size;
}

}  // namespace

struct PitchShifter::State {
  std::vector<float> ring;
  long long mask;
  long long written = 0;
  double read;
  double fadeFrom = 0;
  int fadeLeft = 0;
  std::vector<float> fadeIn;
  float ratio = 1;
  float period;
  float shortestPeriod;
  float longestPeriod;
  double rate;
  int minDelay;

  explicit State(double sampleRate)
      : fadeIn(size_t(std::lround(sampleRate * kCrossfadeSeconds))),
        period(float(sampleRate * kStartingPeriodSeconds)),
        shortestPeriod(float(sampleRate * kShortestPeriodSeconds)),
        longestPeriod(float(sampleRate * kLongestPeriodSeconds)),
        rate(sampleRate) {
    const int crossfade = int(fadeIn.size());
    // While a jump back fades, the old read point keeps closing on the newest sample at up to
    // six semitones' rate; it must not reach it before the fade ends
    const double maxRatio = std::pow(2.0, kMaxSemitones / 12);
    minDelay = kInterpolationReach + 2 + int(std::ceil((maxRatio - 1) * crossfade));
    ring.resize(size_t(powerOfTwoAtLeast(minDelay + 2.5 * longestPeriod + 2 * crossfade)));
    mask = (long long)(ring.size()) - 1;
    read = -minDelay;
    for (int i = 0; i < crossfade; ++i) fadeIn[size_t(i)] = float(0.5 - 0.5 * std::cos(M_PI * (i + 0.5) / crossfade));
  }

  float at(long long index) const { return ring[size_t(index & mask)]; }

  /** Catmull-Rom between the two samples around `position`. */
  float sampleAt(double position) const {
    const double floor = std::floor(position);
    const auto i = (long long)(floor);
    const float t = float(position - floor);
    const float y0 = at(i - 1), y1 = at(i), y2 = at(i + 1), y3 = at(i + 2);
    return y1 + 0.5f * t * (y2 - y0 + t * (2 * y0 - 5 * y1 + 4 * y2 - y3 + t * (3 * (y1 - y2) + y3 - y0)));
  }

  /**
   * The jump nearest one period that best continues the waveform: the lag within an eighth of
   * a period either way whose stretch behind the landing point correlates best with the
   * stretch behind the read point.
   */
  int bestJump(int direction) const {
    const int nominal = int(std::lround(period));
    const int reach = std::max(2, nominal / 8);
    const int window = int(fadeIn.size());
    const auto from = (long long)(std::floor(read));
    int best = nominal;
    float bestScore = -2;
    for (int lag = nominal - reach; lag <= nominal + reach; ++lag) {
      const long long to = from + direction * lag;
      if (direction > 0 && written - to < minDelay) break;
      float cross = 0, energyFrom = 0, energyTo = 0;
      for (int k = 1; k <= window; ++k) {
        const float a = at(from - k), b = at(to - k);
        cross += a * b;
        energyFrom += a * a;
        energyTo += b * b;
      }
      const float score = cross / std::sqrt(energyFrom * energyTo + 1e-12f);
      if (score > bestScore) {
        bestScore = score;
        best = lag;
      }
    }
    return best;
  }

  /** Jumps a period back when the read point closes on the newest sample, or ahead when it has fallen a period behind. */
  void keepInRange() {
    const double delay = double(written) - read;
    const double reach = std::max(2.0f, period / 8);
    int direction = 0;
    if (delay < minDelay) {
      direction = -1;
    } else if (delay > minDelay + period + reach + 1) {
      direction = 1;
    }
    if (direction == 0) return;
    fadeFrom = read;
    read += direction * bestJump(direction);
    fadeLeft = int(fadeIn.size());
  }
};

PitchShifter::PitchShifter(double sampleRate) : state_(std::make_unique<State>(sampleRate)) {}

PitchShifter::~PitchShifter() = default;

void PitchShifter::reset() {
  State &s = *state_;
  std::fill(s.ring.begin(), s.ring.end(), 0.0f);
  s.written = 0;
  s.read = -s.minDelay;
  s.fadeLeft = 0;
  s.ratio = 1;
  s.period = float(s.rate * kStartingPeriodSeconds);
}

void PitchShifter::setSemitones(float semitones) {
  state_->ratio = float(std::pow(2.0, std::clamp(semitones, -kMaxSemitones, kMaxSemitones) / 12));
}

void PitchShifter::setPeriod(float samples) {
  State &s = *state_;
  s.period = std::clamp(samples, s.shortestPeriod, s.longestPeriod);
}

void PitchShifter::process(const float *in, float *out, int frames) {
  State &s = *state_;
  for (int n = 0; n < frames; ++n) {
    s.ring[size_t(s.written & s.mask)] = in[n];
    ++s.written;
    if (s.fadeLeft == 0) s.keepInRange();
    float y = s.sampleAt(s.read);
    if (s.fadeLeft > 0) {
      const float gain = s.fadeIn[s.fadeIn.size() - size_t(s.fadeLeft)];
      y = gain * y + (1 - gain) * s.sampleAt(s.fadeFrom);
      s.fadeFrom += s.ratio;
      --s.fadeLeft;
    }
    out[n] = y;
    s.read += s.ratio;
  }
}

float PitchShifter::delaySamples() const { return float(double(state_->written) - state_->read); }

int PitchShifter::minDelaySamples() const { return state_->minDelay; }

}  // namespace aeon::dsp
