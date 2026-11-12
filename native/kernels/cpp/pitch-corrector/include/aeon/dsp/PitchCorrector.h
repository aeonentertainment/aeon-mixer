#pragma once

#include "aeon/dsp/Pitch.h"

#include <memory>

namespace aeon::dsp {

/**
 * Pulls one mono voice from 82 Hz up toward the nearest allowed pitch class. Every 10 ms YIN
 * reads the input; the distance to the nearest class in the target, scaled by the strength and
 * eased in over the retune time, becomes a transposition that PitchShifter applies. The output trails the
 * input by at most one period of the voice and 0.25 ms, 1.3 ms when shifting six semitones up. At strength 0 the output is the input
 * itself with no delay, and turning the strength up from or down to 0 crossfades over 20 ms;
 * the detector runs either way.
 *
 * A host calls `process` from its audio thread and `setTarget` between calls. Nothing
 * allocates after construction.
 */
class PitchCorrector {
 public:
  explicit PitchCorrector(double sampleRate);
  ~PitchCorrector();
  PitchCorrector(const PitchCorrector &) = delete;
  PitchCorrector &operator=(const PitchCorrector &) = delete;

  /** What to pull toward (`pitchClassOf(note)` for a melody note), how far from 0 to 1, and how fast in milliseconds. */
  void setTarget(PitchClassMask mask, float strength, float retuneMs);
  /** `in` and `out` hold `frames` samples each and must not overlap. */
  void process(const float *in, float *out, int frames);
  /** Forgets the voice it was hearing, for a new stream on the same channel; keeps the target and allocates nothing. */
  void reset();

  /** The newest detector reading. */
  Pitch detected() const;
  /** Frames until the next detector reading; a host that publishes every reading processes at most this many at a time. */
  int framesUntilReading() const;
  /** How far a reading's centre trails the newest sample it read: half the detector's window. */
  int readingLagSamples() const;
  /** The newest reading's distance to the class it is pulled toward, in semitones; 0 while unvoiced. */
  float errorSemitones() const;
  /** How far the output trails the input now, in samples; 0 while bypassed. */
  int latencySamples() const;

 private:
  struct State;
  std::unique_ptr<State> state_;
};

}  // namespace aeon::dsp
