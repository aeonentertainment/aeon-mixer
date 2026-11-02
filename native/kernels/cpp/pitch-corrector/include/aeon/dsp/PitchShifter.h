#pragma once

#include <memory>

namespace aeon::dsp {

/**
 * Transposes a monophonic voice in the time domain, the way zita-at1 does: the input is read
 * back from a short delay line at the transposition's rate, and when the read point has drifted
 * a period from where it started it jumps back or ahead by one pitch period, refined by
 * correlation, under a short crossfade. A steady pitch comes out exact whatever the voice's
 * harmonics; the formants move with the transposition, which over the semitone or so a tuned
 * singer needs is too little to hear. The delay stays between `minDelaySamples()` and that
 * plus 1.125 periods of the voice: a jump waits for a whole period of drift, and correlation may
 * stretch it by an eighth.
 */
class PitchShifter {
 public:
  explicit PitchShifter(double sampleRate);
  ~PitchShifter();
  PitchShifter(const PitchShifter &) = delete;
  PitchShifter &operator=(const PitchShifter &) = delete;

  /** The transposition from now on, clamped to six semitones either way. */
  void setSemitones(float semitones);
  /** The voice's period from the detector, held to 82 Hz at the longest; a jump moves the read point by about this much. */
  void setPeriod(float samples);
  /** `in` and `out` hold `frames` samples each and must not overlap. */
  void process(const float *in, float *out, int frames);
  /** Forgets the voice it was shifting; allocates nothing. */
  void reset();
  /** How far the output trails the input now, in samples. */
  float delaySamples() const;
  int minDelaySamples() const;

 private:
  struct State;
  std::unique_ptr<State> state_;
};

}  // namespace aeon::dsp
