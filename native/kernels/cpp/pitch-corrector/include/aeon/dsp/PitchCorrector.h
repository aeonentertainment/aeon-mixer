#pragma once

#include "aeon/dsp/Pitch.h"

#include <memory>

namespace aeon::dsp {

/**
 * Pulls one mono voice from 82 Hz up toward the nearest allowed pitch class. Every 5 ms
 * PitchReader reads the input; the distance from where the voice is now to the nearest class in
 * the target, scaled by the strength and eased in over the retune time, becomes a transposition
 * that PitchShifter applies. Once NoteCentre hears the voice swing around a held note, the ease
 * slows to the centre's 120 ms, so the note is corrected and the vibrato is left alone. The output
 * trails the input by 1.2 ms plus at most 1.125 periods of the voice. At strength 0 the output is
 * the input itself with no delay, and turning the strength up from or down to 0 crossfades over
 * 20 ms; the reader runs either way.
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

  /** The newest reading, of where the voice was at its window's centre. */
  Pitch detected() const;
  /** Frames until the next reading to publish, one every 10 ms; a host that publishes them processes at most this many at a time. */
  int framesUntilReading() const;
  /** How far the newest reading's centre trails the newest sample it read: half the window it took, which follows the voice. */
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
