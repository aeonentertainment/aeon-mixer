#include "kernels.h"

#include "aeon/dsp/YinDetector.h"

#include <new>

using aeon::dsp::YinDetector;

extern "C" {

YinDetector *aeon_yin_new(double rate, std::int32_t window_samples) noexcept {
  try {
    return new YinDetector(rate, window_samples);
  } catch (...) {
    return nullptr;
  }
}

void aeon_yin_free(YinDetector *yin) noexcept { delete yin; }

AeonPitch aeon_yin_analyze(YinDetector *yin, const float *window) noexcept {
  const aeon::dsp::Pitch pitch = yin->analyze(window);
  return {pitch.hz, pitch.confidence};
}

}  // extern "C"
