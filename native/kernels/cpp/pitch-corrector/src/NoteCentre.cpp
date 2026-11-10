#include "aeon/dsp/NoteCentre.h"

#include <algorithm>
#include <cmath>

namespace aeon::dsp {

namespace {

/** A held note's centre averages this long: a vibrato from 4 Hz up keeps 95 % of its depth. */
constexpr double kHoldSeconds = 0.120;
/** A landing note's centre follows the voice this closely, so crossings show once it holds still. */
constexpr double kLandSeconds = 0.040;
/** The voice must cross its centre twice within two of these to be swinging: a half cycle of 3.3 Hz, under the slowest vibrato's. */
constexpr float kSwingSeconds = 0.150;
/** Crossing the centre means passing it by more than a reading wobbles, up to 7 cents on a low voice; the narrowest vibrato swings 14. */
constexpr float kCrossingSemitones = 0.08f;
/** How far a voice may stray before its note lands again: 1.5 times the peak of its own swing, a sine's peak being root two times its RMS. */
constexpr float kStrayPerSwing = 2.1f;
/** The least stray that lands again, above a 30 cent swing on the midpoint of two notes. */
constexpr float kLeastStraySemitones = 0.4f;
/** The most: a semitone away is another note. */
constexpr float kMostStraySemitones = 1.0f;
/** The swing's size averages over this long, two cycles of a 5 Hz vibrato. */
constexpr double kSwingSizeSeconds = 0.400;

float smoothingOver(double hopSeconds, double seconds) { return float(1 - std::exp(-hopSeconds / seconds)); }

}  // namespace

double NoteCentre::holdSeconds() { return kHoldSeconds; }

NoteCentre::NoteCentre(double hopSeconds)
    : hop_(float(hopSeconds)),
      holdSmoothing_(smoothingOver(hopSeconds, kHoldSeconds)),
      landSmoothing_(smoothingOver(hopSeconds, kLandSeconds)),
      swingSmoothing_(smoothingOver(hopSeconds, kSwingSizeSeconds)) {}

void NoteCentre::hear(float midiNow) {
  if (!heard_) {
    heard_ = true;
    centre_ = midiNow;
    swingPower_ = 0;
    holding_ = false;
    forgetCrossings();
    return;
  }
  centre_ += (midiNow - centre_) * (holding_ ? holdSmoothing_ : landSmoothing_);
  const float away = midiNow - centre_;

  sinceCrossed_ += hop_;
  sinceCrossedBefore_ += hop_;
  const int side = away > kCrossingSemitones ? 1 : away < -kCrossingSemitones ? -1 : 0;
  if (side != 0 && side != side_) {
    if (side_ != 0) {
      sinceCrossedBefore_ = sinceCrossed_;
      sinceCrossed_ = 0;
    }
    side_ = side;
  }

  const float stray = std::clamp(kStrayPerSwing * std::sqrt(swingPower_), kLeastStraySemitones, kMostStraySemitones);
  const bool swinging = sinceCrossed_ < kSwingSeconds && sinceCrossedBefore_ < 2 * kSwingSeconds;
  holding_ = swinging && std::abs(away) <= stray;
  // Only a held swing teaches its size, so a leap between notes cannot widen what counts as straying
  if (holding_) swingPower_ += (std::min(away * away, stray * stray) - swingPower_) * swingSmoothing_;
}

void NoteCentre::lose() {
  heard_ = false;
  holding_ = false;
}

void NoteCentre::land() {
  holding_ = false;
  forgetCrossings();
}

void NoteCentre::forgetCrossings() {
  side_ = 0;
  sinceCrossed_ = kNever;
  sinceCrossedBefore_ = kNever;
}

}  // namespace aeon::dsp
