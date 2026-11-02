#pragma once

#include "aeon/dsp/Pitch.h"

#include <memory>

namespace aeon::dsp {

/**
 * YIN (de Cheveigné and Kawahara, 2002) with the difference function taken from the whole
 * window's autocorrelation by FFT, the form #87 measured. Stateless between calls: hand
 * `analyze` the newest `windowSamples()` samples every hop; a reading describes the window's
 * centre. 2048 samples at 48 kHz answer a pitch change in about 40 ms, 1024 in about 20 ms.
 * Outside silence it always names its best period: the confidence says how periodic the window
 * is, above 0.9 when YIN's dip is under its threshold.
 */
class YinDetector {
 public:
  YinDetector(double sampleRate, int windowSamples, float minHz = 50, float maxHz = 1100);
  ~YinDetector();
  YinDetector(const YinDetector &) = delete;
  YinDetector &operator=(const YinDetector &) = delete;

  /** The pitch of `window`, oldest sample first, `windowSamples()` long. */
  Pitch analyze(const float *window);
  int windowSamples() const;

 private:
  struct State;
  std::unique_ptr<State> state_;
};

}  // namespace aeon::dsp
