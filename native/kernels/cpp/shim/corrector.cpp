#include "kernels.h"

#include "aeon/dsp/PitchCorrector.h"

#include <new>

using aeon::dsp::PitchCorrector;

extern "C" {

PitchCorrector *aeon_corrector_new(double rate) noexcept {
  try {
    return new PitchCorrector(rate);
  } catch (...) {
    return nullptr;
  }
}

void aeon_corrector_free(PitchCorrector *corrector) noexcept { delete corrector; }

void aeon_corrector_set_target(PitchCorrector *corrector, std::uint16_t mask, float strength, float retune_ms) noexcept {
  corrector->setTarget(mask, strength, retune_ms);
}

void aeon_corrector_process(PitchCorrector *corrector, const float *in, float *out, std::int32_t frames) noexcept {
  corrector->process(in, out, frames);
}

void aeon_corrector_reset(PitchCorrector *corrector) noexcept { corrector->reset(); }

AeonPitch aeon_corrector_detected(const PitchCorrector *corrector) noexcept {
  const aeon::dsp::Pitch pitch = corrector->detected();
  return {pitch.hz, pitch.confidence};
}

AeonPitch aeon_corrector_published(const PitchCorrector *corrector) noexcept {
  const aeon::dsp::Pitch pitch = corrector->published();
  return {pitch.hz, pitch.confidence};
}

std::int32_t aeon_corrector_published_lag_samples(const PitchCorrector *corrector) noexcept {
  return corrector->publishedLagSamples();
}

std::int32_t aeon_corrector_frames_until_reading(const PitchCorrector *corrector) noexcept {
  return corrector->framesUntilReading();
}

std::int32_t aeon_corrector_reading_lag_samples(const PitchCorrector *corrector) noexcept {
  return corrector->readingLagSamples();
}

float aeon_corrector_error_semitones(const PitchCorrector *corrector) noexcept { return corrector->errorSemitones(); }

bool aeon_corrector_correcting(const PitchCorrector *corrector) noexcept { return corrector->correcting(); }

float aeon_corrector_published_error_semitones(const PitchCorrector *corrector) noexcept {
  return corrector->publishedErrorSemitones();
}

std::int32_t aeon_corrector_latency_samples(const PitchCorrector *corrector) noexcept {
  return corrector->latencySamples();
}

}  // extern "C"
