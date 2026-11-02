#pragma once

#include <cmath>
#include <cstdint>

namespace aeon::dsp {

/** The twelve pitch classes a voice may be pulled toward, as bits, C in bit 0. */
using PitchClassMask = std::uint16_t;

inline constexpr PitchClassMask kEveryPitchClass = 0x0FFF;

/** A melody note as a mask: one class, so a singer an octave away is corrected in their own octave. */
constexpr PitchClassMask pitchClassOf(int midi) {
  return static_cast<PitchClassMask>(1u << (((midi % 12) + 12) % 12));
}

/** Fractional MIDI note of a frequency: 69 is A4 at 440 Hz. */
inline float midiOf(float hz) {
  return 69.0f + 12.0f * std::log2(hz / 440.0f);
}

/** One detector reading; `hz` is 0 in silence. */
struct Pitch {
  float hz = 0;
  /** 0 to 1: one minus the depth of YIN's dip; above 0.9 is a voice YIN is sure of. */
  float confidence = 0;
};

}  // namespace aeon::dsp
