#include "aeon/dsp/PitchCorrector.h"

#include "aeon/dsp/NoteCentre.h"
#include "aeon/dsp/PitchReader.h"
#include "aeon/dsp/PitchShifter.h"

#include <algorithm>
#include <cmath>
#include <vector>

namespace aeon::dsp {

namespace {

constexpr double kHopSeconds = 0.005;
/** A host hears every second reading, every 10 ms; the display and the score need no more, and only those ask the longest window. */
constexpr int kReadingsPerPublished = 2;
constexpr double kCrossfadeSeconds = 0.020;
/** Semitones past the midpoint between two classes before the target changes class, so vibrato there cannot flip it. */
constexpr float kHysteresis = 0.2f;
constexpr int kNoTarget = -1;

}  // namespace

struct PitchCorrector::State {
  double rate;
  int hop;
  PitchReader reader;
  NoteCentre note;
  PitchShifter shifter;
  std::vector<float> wet;
  int hopFill = 0;
  int readingsUntilPublished = kReadingsPerPublished;

  PitchClassMask mask = kEveryPitchClass;
  float strength = 0;
  float retuneMs = 0;

  /** Where the voice is by the time the output plays what the shifter is reading. */
  float midiNow = 0;
  int targetMidi = kNoTarget;
  float error = 0;
  float targetShift = 0;
  float shift = 0;
  float wetGain = 0;
  float crossfadeStep;

  explicit State(double sampleRate)
      : rate(sampleRate),
        hop(std::max(1, int(std::lround(sampleRate * kHopSeconds)))),
        reader(sampleRate),
        note(kHopSeconds),
        shifter(sampleRate),
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

  // While a note lands, which note comes from the reading itself, so a glide does not reach a new
  // note before the voice does; a held note's comes from its centre, so vibrato across a midpoint
  // cannot flip it. How far to move comes from where the voice is now either way
  void retarget() {
    const float heard = note.holding() ? note.centre() : reader.midi();
    const int next = reader.voiced() ? nearestTarget(heard) : kNoTarget;
    // A held note that turns into another lands on it at the singer's retune, not the centre's pace
    if (note.holding() && next != targetMidi) note.land();
    targetMidi = next;
    const bool correcting = targetMidi != kNoTarget;
    error = correcting ? float(targetMidi) - reader.midi() : 0;
    targetShift = correcting ? strength * (float(targetMidi) - midiNow) : 0;
  }

  void read(bool toPublish) {
    if (toPublish ? reader.read() : reader.glance()) {
      shifter.setPeriod(float(rate / reader.pitch().hz));
      // A reading describes its window's centre and waits up to a hop for the next, and the
      // shifter's own delay takes back what it trails the input by
      const float behind = float(reader.lagSamples()) + 0.5f * float(hop) - shifter.delaySamples();
      midiNow = reader.midiAfter(std::max(0.0f, behind));
      note.hear(midiNow);
    } else if (!reader.voiced()) {
      note.lose();
    }
    retarget();
  }
};

PitchCorrector::PitchCorrector(double sampleRate) : state_(std::make_unique<State>(sampleRate)) {}

PitchCorrector::~PitchCorrector() = default;

void PitchCorrector::reset() {
  State &s = *state_;
  s.reader.reset();
  s.hopFill = 0;
  s.readingsUntilPublished = kReadingsPerPublished;
  s.midiNow = 0;
  s.targetMidi = kNoTarget;
  s.error = 0;
  s.targetShift = 0;
  s.shift = 0;
  // Back to the dry input, crossfading into correction again as on a fresh start
  s.wetGain = 0;
  s.note.lose();
  s.shifter.reset();
}

void PitchCorrector::setTarget(PitchClassMask mask, float strength, float retuneMs) {
  State &s = *state_;
  const PitchClassMask next = mask & kEveryPitchClass;
  if (next != s.mask) s.note.land();
  s.mask = next;
  s.strength = std::clamp(strength, 0.0f, 1.0f);
  s.retuneMs = std::max(0.0f, retuneMs);
  // A new target counts from the last reading at once, so a guide boundary lands where the host put it
  s.retarget();
}

void PitchCorrector::process(const float *in, float *out, int frames) {
  State &s = *state_;
  const float wetTarget = s.strength > 0 ? 1.0f : 0.0f;
  int done = 0;
  while (done < frames) {
    const int n = std::min(frames - done, s.hop - s.hopFill);
    s.reader.push(in + done, n);

    // A held note moves its correction no faster than its centre, so the vibrato stays the singer's
    const double easeSeconds = s.note.holding() ? std::max(double(s.retuneMs) / 1000.0, NoteCentre::holdSeconds()) : s.retuneMs / 1000.0;
    const double easeSamples = easeSeconds * s.rate;
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
      const bool toPublish = s.readingsUntilPublished == 1;
      s.readingsUntilPublished = toPublish ? kReadingsPerPublished : s.readingsUntilPublished - 1;
      s.read(toPublish);
    }
    done += n;
  }
}

Pitch PitchCorrector::detected() const { return state_->reader.pitch(); }

int PitchCorrector::framesUntilReading() const {
  const State &s = *state_;
  return s.readingsUntilPublished * s.hop - s.hopFill;
}

int PitchCorrector::readingLagSamples() const { return state_->reader.lagSamples(); }

float PitchCorrector::errorSemitones() const { return state_->error; }

int PitchCorrector::latencySamples() const {
  const State &s = *state_;
  return s.wetGain > 0 ? int(std::lround(s.shifter.delaySamples())) : 0;
}

}  // namespace aeon::dsp
