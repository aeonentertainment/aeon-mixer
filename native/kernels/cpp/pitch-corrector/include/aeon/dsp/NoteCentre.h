#pragma once

namespace aeon::dsp {

/**
 * Where the note a voice is holding sits, so correction moves the note and leaves the singer's
 * vibrato around it. A note lands first: its centre follows the voice within 40 ms, so a scoop, a
 * step or a drift is corrected at the retune the singer chose. Once the voice swings across that
 * centre, twice within 0.3 s, the note is held: the centre averages over 120 ms, which a vibrato
 * from 4 Hz up barely moves. A held voice that stops swinging, or strays further than its own
 * swing reaches, lands again. docs/decisions/0013-note-centre.md has the numbers behind each
 * choice.
 *
 * PitchCorrector feeds it every sure reading. Nothing allocates.
 */
class NoteCentre {
 public:
  explicit NoteCentre(double hopSeconds);
  /** How long a held note's centre averages, which is also how slowly its correction may move. */
  static double holdSeconds();

  /** A sure reading of where the voice is now, in fractional MIDI. */
  void hear(float midiNow);
  /** The voice has gone; the next reading starts a note. */
  void lose();
  /** The note changes under the voice, as at a guide's edge: it lands before it holds again. */
  void land();

  /** True while the note is held and its centre averages slowly. */
  bool holding() const { return holding_; }
  /** Fractional MIDI note the voice is centred on. */
  float centre() const { return centre_; }

 private:
  static constexpr float kNever = 1e9f;

  void forgetCrossings();

  float hop_;
  float holdSmoothing_;
  float landSmoothing_;
  float swingSmoothing_;
  bool heard_ = false;
  bool holding_ = false;
  float centre_ = 0;
  /** Mean square of the held voice's distance from its centre, in semitones squared. */
  float swingPower_ = 0;
  /** Which side of the centre the voice was last clearly on: -1, 0 before it has been on either, or 1. */
  int side_ = 0;
  float sinceCrossed_ = kNever;
  float sinceCrossedBefore_ = kNever;
};

}  // namespace aeon::dsp
