#pragma once

#include <memory>

namespace aeon::dsp {

/**
 * Transposes a monophonic voice in the time domain, the way zita-at1 does: the input is read
 * back from a short delay line at the transposition's rate, and when the read point has drifted
 * a period from where it started it jumps back or ahead by one pitch period, refined by
 * correlation to a fraction of a sample, under a short crossfade. A steady pitch comes out exact
 * whatever the voice's harmonics; the formants move with the transposition, which over the
 * semitone or so a tuned singer needs is too little to hear.
 *
 * The delay rides between `minDelaySamples()`, which follows the ratio, and that plus one period
 * of the voice: a jump ahead waits for a whole period of drift and no longer, unless the waveform
 * still matches better further on. A rest has no waveform to keep whole, so `setVoiced(false)`
 * lets the read point move down to a quarter period above the floor, where the next phrase starts
 * with room to shift up before it repeats a period. Decision 0021 has the numbers.
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
  /** Whether the detector hears a voice; once a rest begins, the read point moves down to near the floor. */
  void setVoiced(bool voiced);
  /** `in` and `out` hold `frames` samples each and must not overlap. */
  void process(const float *in, float *out, int frames);
  /** Forgets the voice it was shifting; allocates nothing. */
  void reset();
  /** How far the output trails the input now, in samples. */
  float delaySamples() const;
  /** The closest the read point may come to the newest sample at the ratio now: 5 samples, plus what a jump's crossfade gains when shifting up. */
  int minDelaySamples() const;

 private:
  struct State;
  std::unique_ptr<State> state_;
};

}  // namespace aeon::dsp
