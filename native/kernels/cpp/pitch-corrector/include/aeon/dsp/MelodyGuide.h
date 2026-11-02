#pragma once

#include "aeon/dsp/Pitch.h"

#include <string_view>
#include <vector>

namespace aeon::dsp {

/** One note of a song's melody on the track's clock, the pipeline's `pitch-guide.json` shape. */
struct GuideNote {
  int midi = 0;
  double startMs = 0;
  double endMs = 0;
};

/** What the guide asks for at one moment: the classes to pull toward and the note behind them, -1 for the key. */
struct GuideTarget {
  PitchClassMask mask = kEveryPitchClass;
  int midi = -1;

  bool operator==(const GuideTarget &other) const { return mask == other.mask && midi == other.midi; }
  bool operator!=(const GuideTarget &other) const { return !(*this == other); }
};

/**
 * A song's melody, which a host follows on its own audio clock. Melody mode pulls toward the
 * note sounding now and falls back to the key's scale between notes; an empty guide is key
 * mode throughout. A host asks `targetAt` where it starts and again at each `nextEdgeAfter`.
 */
class MelodyGuide {
 public:
  MelodyGuide() = default;
  explicit MelodyGuide(std::vector<GuideNote> notes);

  /**
   * The text form a host takes across a string boundary: `midi startMs endMs` per note, notes
   * separated by `;`. Anything malformed is skipped, so a bad note costs that note only.
   */
  static MelodyGuide parse(std::string_view text);

  bool empty() const { return notes_.empty(); }
  std::size_t size() const { return notes_.size(); }
  /** The latest-starting note sounding at `ms`, so an overlap favours the newer note; the key's classes when none is. */
  GuideTarget targetAt(double ms, PitchClassMask key) const;
  /** The first note start or end after `ms`; infinity when none is left. */
  double nextEdgeAfter(double ms) const;

 private:
  std::vector<GuideNote> notes_;
  std::vector<double> edges_;
};

}  // namespace aeon::dsp
