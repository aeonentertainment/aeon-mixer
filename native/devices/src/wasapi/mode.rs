/// Which WASAPI path to open. The engine uses `Auto`; measure-sink forces one to compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasapiMode {
    /// Shared, and exclusive only when shared cannot open at all: a mix format other than f32
    /// at 48 kHz, or no IAudioClient3.
    Auto,
    /// IAudioClient3 shared low-latency at the nearest period it allows.
    Shared,
    /// Exclusive, event-driven. Every other app on the device goes silent.
    Exclusive,
}
