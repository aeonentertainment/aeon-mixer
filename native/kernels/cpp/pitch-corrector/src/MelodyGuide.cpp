#include "aeon/dsp/MelodyGuide.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <limits>
#include <string>

namespace aeon::dsp {

MelodyGuide::MelodyGuide(std::vector<GuideNote> notes) : notes_(std::move(notes)) {
  notes_.erase(std::remove_if(notes_.begin(), notes_.end(), [](const GuideNote &n) { return !(n.endMs > n.startMs); }),
               notes_.end());
  std::stable_sort(notes_.begin(), notes_.end(), [](const GuideNote &a, const GuideNote &b) { return a.startMs < b.startMs; });
  edges_.reserve(notes_.size() * 2);
  for (const GuideNote &note : notes_) {
    edges_.push_back(note.startMs);
    edges_.push_back(note.endMs);
  }
  std::sort(edges_.begin(), edges_.end());
  edges_.erase(std::unique(edges_.begin(), edges_.end()), edges_.end());
}

MelodyGuide MelodyGuide::parse(std::string_view text) {
  std::vector<GuideNote> notes;
  while (!text.empty()) {
    const std::size_t end = text.find(';');
    // strtod needs a terminator; one note is a few dozen characters
    const std::string note(text.substr(0, end));
    text = end == std::string_view::npos ? std::string_view() : text.substr(end + 1);
    const char *cursor = note.c_str();
    char *next = nullptr;
    // Read as a number and rounded: an integer read would stop at a fraction and take it for the start
    const double midi = std::strtod(cursor, &next);
    if (next == cursor) continue;
    cursor = next;
    const double startMs = std::strtod(cursor, &next);
    if (next == cursor) continue;
    cursor = next;
    const double endMs = std::strtod(cursor, &next);
    if (next == cursor) continue;
    notes.push_back({int(std::lround(midi)), startMs, endMs});
  }
  return MelodyGuide(std::move(notes));
}

GuideTarget MelodyGuide::targetAt(double ms, PitchClassMask key) const {
  // Notes are in start order, so walking back from the last one started finds the newest sounding
  auto it = std::upper_bound(notes_.begin(), notes_.end(), ms, [](double t, const GuideNote &n) { return t < n.startMs; });
  while (it != notes_.begin()) {
    --it;
    if (ms < it->endMs) return {pitchClassOf(it->midi), it->midi};
  }
  return {key, -1};
}

double MelodyGuide::nextEdgeAfter(double ms) const {
  auto it = std::upper_bound(edges_.begin(), edges_.end(), ms);
  return it == edges_.end() ? std::numeric_limits<double>::infinity() : *it;
}

}  // namespace aeon::dsp
