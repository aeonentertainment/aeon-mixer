#include "aeon/dsp/PitchReader.h"

#include "aeon/dsp/YinDetector.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <memory>
#include <vector>

namespace aeon::dsp {

namespace {

/** Shortest first, in seconds; 512, 1024 and 2048 samples at 48 kHz. The last is the window #87 chose. */
constexpr std::array<double, 3> kWindowSeconds = {512.0 / 48000, 1024.0 / 48000, 2048.0 / 48000};
/** YIN's rule of thumb is two periods; the margin keeps a voice's vibrato on one window. */
constexpr float kPeriodsInWindow = 2.2f;
/** A reading this sure is a voice to correct: YIN's dip under its threshold. */
constexpr float kVoicedConfidence = 0.9f;
/** E2, the bottom of a bass's range. A lower voice would need jumps that break the 15 ms budget, so it passes uncorrected. */
constexpr float kLowestCorrectedHz = 82.0f;
/** How long a voice survives without a sure reading, so one dropped reading mid-note does not let go. */
constexpr double kUnvoicedHoldSeconds = 0.030;
/** A short window that reads this far from the last note has more likely locked onto a harmonic: let a longer one judge. */
constexpr float kLeapSemitones = 9.0f;
/** The pitch line averages this long, so it follows a glide and not one period's jitter. */
constexpr double kSlopeSeconds = 0.020;
/** A pause longer than this, or a step bigger than this, starts a new line: a new note is not a glide. */
constexpr double kLongestLineGapSeconds = 0.040;
constexpr float kLargestLineStepSemitones = 1.5f;

int powerOfTwoNear(double samples) {
  return 1 << std::max(4, int(std::lround(std::log2(samples))));
}

}  // namespace

struct PitchReader::State {
  double rate;
  std::vector<std::unique_ptr<YinDetector>> detectors;
  int longest;
  std::vector<float> history;
  std::vector<float> window;
  int historyPos = 0;
  long long written = 0;

  Pitch pitch;
  int lag;
  /** The longest window's reading of the newest samples, once one has been taken since the last push. */
  Pitch longestPitch;
  bool longestFresh = false;
  float midi = 0;
  bool voiced = false;
  double unvoicedSeconds = 0;
  long long lastCentre = -1;
  long long lastRead = 0;
  float slope = 0;

  explicit State(double sampleRate) : rate(sampleRate) {
    for (const double seconds : kWindowSeconds) {
      detectors.push_back(std::make_unique<YinDetector>(sampleRate, powerOfTwoNear(sampleRate * seconds)));
    }
    longest = detectors.back()->windowSamples();
    history.resize(size_t(longest));
    window.resize(size_t(longest));
    lag = longest / 2;
  }

  /** The newest `samples`, oldest first, contiguous at the end of `window`. */
  const float *newest(int samples) {
    float *to = window.data() + (longest - samples);
    int from = (historyPos + longest - samples) % longest;
    for (int i = 0; i < samples; ++i) {
      to[i] = history[size_t(from)];
      from = from + 1 == longest ? 0 : from + 1;
    }
    return to;
  }

  bool isVoice(Pitch reading) const { return PitchReader::isVoice(reading); }

  /** Whether `reading`, from a window short of the longest, can be trusted without asking a longer one. */
  bool vouches(Pitch reading, int samples) const {
    if (!isVoice(reading) || float(samples) * reading.hz < kPeriodsInWindow * float(rate)) return false;
    return !voiced || std::abs(midiOf(reading.hz) - midi) <= kLeapSemitones;
  }

  /** The first short window that vouches becomes the reading. */
  bool tryShortWindows() {
    for (size_t k = 0; k + 1 < detectors.size(); ++k) {
      const int samples = detectors[k]->windowSamples();
      const Pitch reading = detectors[k]->analyze(newest(samples));
      if (!vouches(reading, samples)) continue;
      pitch = reading;
      lag = samples / 2;
      heardVoice();
      return true;
    }
    return false;
  }

  void heardVoice() {
    const float nextMidi = midiOf(pitch.hz);
    const long long centre = written - lag;
    const double gap = double(centre - lastCentre) / rate;
    const float step = nextMidi - midi;
    if (lastCentre < 0 || gap >= kLongestLineGapSeconds || std::abs(step) >= kLargestLineStepSemitones) {
      slope = 0;
    } else if (gap > 0) {
      // Going from a longer window to a shorter one moves the centre forward; the other way back
      slope += (step / float(gap * rate) - slope) * float(1 - std::exp(-gap / kSlopeSeconds));
    }
    lastCentre = centre;
    midi = nextMidi;
    voiced = true;
    unvoicedSeconds = 0;
    lastRead = written;
  }
};

PitchReader::PitchReader(double sampleRate) : state_(std::make_unique<State>(sampleRate)) {}

PitchReader::~PitchReader() = default;

void PitchReader::push(const float *samples, int frames) {
  State &s = *state_;
  for (int i = 0; i < frames; ++i) {
    s.history[size_t(s.historyPos)] = samples[i];
    s.historyPos = s.historyPos + 1 == s.longest ? 0 : s.historyPos + 1;
  }
  s.written += frames;
  if (frames > 0) s.longestFresh = false;
}

bool PitchReader::read() {
  State &s = *state_;
  if (s.tryShortWindows()) return true;
  const int samples = s.longest;
  s.pitch = s.detectors.back()->analyze(s.newest(samples));
  s.lag = samples / 2;
  s.longestPitch = s.pitch;
  s.longestFresh = true;
  if (s.isVoice(s.pitch)) {
    s.heardVoice();
    return true;
  }
  s.unvoicedSeconds += double(s.written - s.lastRead) / s.rate;
  s.lastRead = s.written;
  if (s.unvoicedSeconds > kUnvoicedHoldSeconds) s.voiced = false;
  return false;
}

bool PitchReader::glance() { return state_->tryShortWindows(); }

bool PitchReader::isVoice(Pitch reading) {
  return reading.hz >= kLowestCorrectedHz && reading.confidence >= kVoicedConfidence;
}

void PitchReader::reset() {
  State &s = *state_;
  std::fill(s.history.begin(), s.history.end(), 0.0f);
  s.historyPos = 0;
  s.written = 0;
  s.pitch = {};
  s.lag = s.longest / 2;
  s.longestPitch = {};
  s.longestFresh = false;
  s.midi = 0;
  s.voiced = false;
  s.unvoicedSeconds = 0;
  s.lastCentre = -1;
  s.lastRead = 0;
  s.slope = 0;
}

Pitch PitchReader::pitch() const { return state_->pitch; }

int PitchReader::lagSamples() const { return state_->lag; }

Pitch PitchReader::longest() {
  State &s = *state_;
  if (!s.longestFresh) {
    s.longestPitch = s.detectors.back()->analyze(s.newest(s.longest));
    s.longestFresh = true;
  }
  return s.longestPitch;
}

int PitchReader::longestLagSamples() const { return state_->longest / 2; }

bool PitchReader::voiced() const { return state_->voiced; }

float PitchReader::midi() const { return state_->midi; }

float PitchReader::midiAfter(float samples) const { return state_->midi + state_->slope * samples; }

}  // namespace aeon::dsp
