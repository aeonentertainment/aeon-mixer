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
/** E2, the bottom of a bass's range: the longest jump, so the delay stays inside 13.5 ms. */
constexpr double kLongestPeriodSeconds = 1.0 / 82;
/** Until the detector has heard a voice there is nothing to jump over; any plausible period will do. */
constexpr double kStartingPeriodSeconds = 1.0 / 200;
/** Cubic interpolation reads two samples ahead of the read point, and a sample is written before it is read. */
constexpr int kInterpolationReach = 3;
/** Where a rest leaves the read point, in periods above the floor: low, with room for a voice shifted up to run a while before it repeats a period. */
constexpr double kRestingPeriods = 0.25;

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
  /** A jump ahead found the waveform still matching better further on, and waits until the delay reaches this. */
  double waitUntil = 0;
  bool voiced = false;
  /** Whether this rest has already moved the read point down. */
  bool settled = false;

  explicit State(double sampleRate)
      : fadeIn(size_t(std::lround(sampleRate * kCrossfadeSeconds))),
        period(float(sampleRate * kStartingPeriodSeconds)),
        shortestPeriod(float(sampleRate * kShortestPeriodSeconds)),
        longestPeriod(float(sampleRate * kLongestPeriodSeconds)),
        rate(sampleRate) {
    const double highest = floorFor(std::pow(2.0, kMaxSemitones / 12));
    ring.resize(size_t(powerOfTwoAtLeast(highest + 2.5 * longestPeriod + 2 * double(fadeIn.size()))));
    mask = (long long)(ring.size()) - 1;
    read = -floorFor(1);
    const int crossfade = int(fadeIn.size());
    for (int i = 0; i < crossfade; ++i) fadeIn[size_t(i)] = float(0.5 - 0.5 * std::cos(M_PI * (i + 0.5) / crossfade));
  }

  /**
   * How close behind the newest sample the read point may come at `r`: room for the
   * interpolation's reach, and for what the old read point gains on the input during a jump's
   * crossfade when reading faster than time. 5 samples shifting down, 8 at a quarter tone up, 59
   * at six semitones.
   */
  double floorFor(double r) const {
    return kInterpolationReach + 2 + std::ceil(std::max(0.0, r - 1) * double(fadeIn.size()));
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

  /** How alike the stretch behind the read point and the stretch behind where a jump of `lag` would land. */
  float correlation(int direction, long long lag) const {
    const auto from = (long long)(std::floor(read));
    const long long to = from + direction * lag;
    float cross = 0, energyFrom = 0, energyTo = 0;
    for (int k = 1; k <= int(fadeIn.size()); ++k) {
      const float a = at(from - k), b = at(to - k);
      cross += a * b;
      energyFrom += a * a;
      energyTo += b * b;
    }
    return cross / std::sqrt(energyFrom * energyTo + 1e-12f);
  }

  struct Jump {
    double lag;
    /** The match was still improving at the furthest lag the read point can take yet. */
    bool early;
  };

  /**
   * The jump nearest one period that best continues the waveform, finer than a sample: the lag
   * within an eighth of a period either way whose stretch behind the landing point correlates best
   * with the stretch behind the read point. A jump ahead may not land within `floor` of the newest
   * sample, so it can only take the lags the delay already covers.
   */
  Jump bestJump(int direction, double floor) const {
    const int nominal = int(std::lround(period));
    const int reach = std::max(2, nominal / 8);
    const int furthest = std::min(nominal + reach, int(std::floor(longestPeriod)) + 1);
    const auto from = (long long)(std::floor(read));
    int best = nominal;
    int last = nominal - reach;
    float bestScore = -2;
    for (int lag = nominal - reach; lag <= furthest; ++lag) {
      if (direction > 0 && double(written - (from + lag)) < floor) break;
      last = lag;
      const float score = correlation(direction, lag);
      if (score > bestScore) {
        bestScore = score;
        best = lag;
      }
    }
    if (direction > 0 && best == last && last < furthest) return {double(best), true};
    // A voice's period is not a whole number of samples; whole-sample jumps bias a high voice's
    // pitch by up to 1.8 cents (decision 0004)
    const float before = correlation(direction, best - 1), after = correlation(direction, best + 1);
    const float curvature = before - 2 * bestScore + after;
    double lag = best;
    if (curvature < 0) lag += std::clamp(0.5 * double(before - after) / double(curvature), -0.5, 0.5);
    if (direction > 0) lag = std::min(lag, double(written) - read - floor);
    return {std::min(lag, double(furthest)), false};
  }

  void startJump(double by) {
    fadeFrom = read;
    read += by;
    fadeLeft = int(fadeIn.size());
  }

  /**
   * Jumps a period back when the read point closes on the floor, or ahead once it has fallen a
   * period behind and the waveform matches there. In a rest there is nothing to keep whole, so the
   * read point moves down to a quarter period above the floor, once.
   */
  void keepInRange() {
    const double floor = floorFor(ratio);
    const double delay = double(written) - read;
    if (!voiced && !settled) {
      settled = true;
      const double resting = floor + kRestingPeriods * period;
      if (delay > resting + 1) return startJump(delay - resting);
    }
    if (delay < floor) return startJump(-bestJump(-1, floor).lag);
    if (delay <= floor + period + 1 || delay < waitUntil) return;
    const Jump jump = bestJump(1, floor);
    if (jump.early) {
      waitUntil = delay + 2;
      return;
    }
    waitUntil = 0;
    startJump(jump.lag);
  }
};

PitchShifter::PitchShifter(double sampleRate) : state_(std::make_unique<State>(sampleRate)) {}

PitchShifter::~PitchShifter() = default;

void PitchShifter::reset() {
  State &s = *state_;
  std::fill(s.ring.begin(), s.ring.end(), 0.0f);
  s.written = 0;
  s.read = -s.floorFor(1);
  s.fadeLeft = 0;
  s.ratio = 1;
  s.period = float(s.rate * kStartingPeriodSeconds);
  s.waitUntil = 0;
  s.voiced = false;
  s.settled = false;
}

void PitchShifter::setSemitones(float semitones) {
  state_->ratio = float(std::pow(2.0, std::clamp(semitones, -kMaxSemitones, kMaxSemitones) / 12));
}

void PitchShifter::setPeriod(float samples) {
  State &s = *state_;
  s.period = std::clamp(samples, s.shortestPeriod, s.longestPeriod);
}

void PitchShifter::setVoiced(bool voiced) {
  State &s = *state_;
  if (voiced) s.settled = false;
  s.voiced = voiced;
}

void PitchShifter::process(const float *in, float *out, int frames) {
  State &s = *state_;
  for (int n = 0; n < frames; ++n) {
    s.ring[size_t(s.written & s.mask)] = in[n];
    ++s.written;
    if (s.fadeLeft == 0) s.keepInRange();
    float y = s.sampleAt(s.read);
    // The floor holds for the ratio of the moment; a ratio raised mid-fade must not carry either
    // read point past what the next sample's interpolation can reach
    const double newest = double(s.written + 1 - kInterpolationReach);
    if (s.fadeLeft > 0) {
      const float gain = s.fadeIn[s.fadeIn.size() - size_t(s.fadeLeft)];
      y = gain * y + (1 - gain) * s.sampleAt(s.fadeFrom);
      s.fadeFrom = std::min(s.fadeFrom + s.ratio, newest);
      --s.fadeLeft;
    }
    out[n] = y;
    s.read = std::min(s.read + s.ratio, newest);
  }
}

float PitchShifter::delaySamples() const { return float(double(state_->written) - state_->read); }

int PitchShifter::minDelaySamples() const { return int(state_->floorFor(state_->ratio)); }

}  // namespace aeon::dsp
