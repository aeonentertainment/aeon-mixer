#pragma once

#include "aeon/dsp/Pitch.h"

#include <memory>

namespace aeon::dsp {

/**
 * What a voice is singing, read as soon as a window can vouch for it. Each reading tries the
 * shortest YIN window first and takes the first that is sure, holds at least 2.2 periods of what
 * it found, and does not leap from the last note; the longest window has the last word, so a low
 * voice reads as late as it must and a high one as soon as it can. A reading that stays sure also
 * tracks the slope of the pitch, so a caller can say where the voice has gone since the window's
 * centre. docs/decisions/0010-pitch-reading.md has the numbers behind each choice.
 *
 * A host calls `push` with every sample, and `read` and `glance` from its audio thread. Nothing
 * allocates after construction.
 */
class PitchReader {
 public:
  explicit PitchReader(double sampleRate);
  ~PitchReader();
  PitchReader(const PitchReader &) = delete;
  PitchReader &operator=(const PitchReader &) = delete;

  void push(const float *samples, int frames);
  /** Reads the newest samples, up to the longest window. True when the reading is a voice to correct. */
  bool read();
  /** `read` without the longest window, whose cost buys least between readings: a short one that vouches is a reading, and otherwise nothing has happened. */
  bool glance();
  /** Forgets the voice it was hearing; allocates nothing. */
  void reset();

  /** The newest reading, sure or not. */
  Pitch pitch() const;
  /** How far the newest reading's centre trails the newest sample it read. */
  int lagSamples() const;
  /** True until the voice has been gone for a moment, so one dropped reading mid-note does not let go. */
  bool voiced() const;
  /** Fractional MIDI note of the last sure reading. */
  float midi() const;
  /** Where the voice's pitch line puts it `samples` past the last sure reading's centre. */
  float midiAfter(float samples) const;

 private:
  struct State;
  std::unique_ptr<State> state_;
};

}  // namespace aeon::dsp
