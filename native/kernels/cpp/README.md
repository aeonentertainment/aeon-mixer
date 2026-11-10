# Vendored C++

Each directory is a verbatim copy of upstream at the commit named here, except `pitch-corrector/`,
which this repo now owns. Fix bugs in the others upstream and copy again; do not edit them in place.
`shim/` is ours: the `extern "C"` face `src/ffi.rs` declares.
`build.rs` compiles the shims and the corrector's sources with the `cc` crate as C++17, on
MSVC, clang and gcc alike.

| Directory              | Upstream                                                                                                                  | Commit                                                | Licence                                                             |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------- |
| `pitch-corrector/`     | [aeon-karaoke](https://github.com/aeonentertainment/aeon-karaoke) `lib/dsp/pitch-corrector/{include,src}`, then ours      | `1fe41b840f009d4ad0c1d3e47f713a50870b0808` on `main`  | Aeon Entertainment's own code; Aeon Karaoke carries no licence file |
| `signalsmith-linear/`  | [Signalsmith-Audio/linear](https://github.com/Signalsmith-Audio/linear) `fft.h`, `stft.h`                                 | `7bb1c8f36794d6980080ce817c91f8da8fe56a66`, tag 0.6.6 | MIT, [LICENSE.txt](signalsmith-linear/LICENSE.txt)                  |
| `signalsmith-stretch/` | [Signalsmith-Audio/signalsmith-stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch) `signalsmith-stretch.h` | `a670068d9aeb64913331d5cc29337b19a457a7df`, tag 1.4.0 | MIT, [LICENSE.txt](signalsmith-stretch/LICENSE.txt)                 |

Notes on the copies:

- `pitch-corrector/` started as that commit. `PitchReader`, the corrector's use of it and the
  period refinement in `YinDetector` are this repo's, for
  [decision 0010](../../../docs/decisions/0010-pitch-reading.md), and Aeon Karaoke's GStreamer
  element keeps its own copy until it goes. Do not copy over them from upstream.
- One `signalsmith-linear` serves both kernels. The corrector's own vendored
  `lib/dsp/vendor/signalsmith-linear/fft.h` is byte-identical to linear 0.6.6's, and 0.6.6 is
  the version signalsmith-stretch's `main` pins (its header is unchanged since tag 1.4.0).
  Two copies would be an ODR violation waiting to happen.
- `MelodyGuide.{h,cpp}` are copied with the rest but not compiled. The guide is ported to Rust
  in `engine/src/slot/effects/melody.rs`, on frames instead of milliseconds, so the audio thread
  never parses text.
- The C++ kernels allocate only when built. The tests' `assert_no_alloc` guard sees Rust's
  allocator, not C++'s, so that rests on the code: the corrector documents it, and
  signalsmith-stretch's `process` only resizes buffers within the capacity `configure` gave
  them.
