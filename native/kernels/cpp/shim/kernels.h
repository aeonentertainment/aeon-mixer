// The C face of the kernels, which src/ffi.rs declares again in Rust. Every function is
// noexcept: a constructor that cannot allocate returns null instead of throwing across the
// boundary, and nothing else throws.
#pragma once

#include <cstdint>

extern "C" {

struct AeonPitch {
  float hz;
  float confidence;
};

}  // extern "C"
