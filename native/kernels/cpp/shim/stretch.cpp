#include "kernels.h"

#include "signalsmith-stretch/signalsmith-stretch.h"

#include <new>

using Stretch = signalsmith::stretch::SignalsmithStretch<float>;

extern "C" {

/** `preset` 0 is the library's default, 1 its cheaper one, each with the library's own split of the work across calls. */
Stretch *aeon_stretch_new(std::int32_t channels, float rate, std::int32_t preset) noexcept {
  try {
    auto *stretch = new Stretch();
    if (preset == 1) {
      stretch->presetCheaper(channels, rate);
    } else {
      stretch->presetDefault(channels, rate);
    }
    return stretch;
  } catch (...) {
    return nullptr;
  }
}

void aeon_stretch_free(Stretch *stretch) noexcept { delete stretch; }

void aeon_stretch_set_transpose_semitones(Stretch *stretch, float semitones) noexcept {
  stretch->setTransposeSemitones(semitones);
}

/** `in` and `out` hold one pointer per channel; time stretches when the frame counts differ. */
void aeon_stretch_process(Stretch *stretch, const float *const *in, std::int32_t in_frames, float *const *out,
                          std::int32_t out_frames) noexcept {
  stretch->process(in, in_frames, out, out_frames);
}

void aeon_stretch_reset(Stretch *stretch) noexcept { stretch->reset(); }

std::int32_t aeon_stretch_latency_frames(const Stretch *stretch) noexcept {
  return stretch->inputLatency() + stretch->outputLatency();
}

}  // extern "C"
