//! Compiles the vendored C++ in cpp/ and the shims that give it a C face. cpp/README.md says
//! where each source came from.

fn main() {
    let sources = [
        "cpp/shim/corrector.cpp",
        "cpp/shim/stretch.cpp",
        "cpp/shim/yin.cpp",
        "cpp/pitch-corrector/src/NoteCentre.cpp",
        "cpp/pitch-corrector/src/PitchCorrector.cpp",
        "cpp/pitch-corrector/src/PitchReader.cpp",
        "cpp/pitch-corrector/src/PitchShifter.cpp",
        "cpp/pitch-corrector/src/YinDetector.cpp",
    ];
    for path in sources.iter().chain(&["cpp/shim/kernels.h"]) {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=cpp/pitch-corrector/include");
    println!("cargo:rerun-if-changed=cpp/signalsmith-linear");
    println!("cargo:rerun-if-changed=cpp/signalsmith-stretch");

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .files(sources)
        .include("cpp")
        .include("cpp/pitch-corrector/include")
        // MSVC hides M_PI without it, and its min and max macros break std::min
        .define("_USE_MATH_DEFINES", None)
        .define("NOMINMAX", None)
        // DSP at -O0 makes the debug test run crawl; the kernels are optimised in every profile
        .opt_level(2)
        // The vendored code is upstream's to keep warning-free, not ours
        .warnings(false)
        .compile("kernels");
}
