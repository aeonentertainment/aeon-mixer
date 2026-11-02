/// Which WASAPI path to open. The engine uses `Auto`; measure-sink forces one to compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasapiMode {
    /// Shared when it reaches the asked period, else exclusive, else shared at whatever period
    /// the engine allows.
    Auto,
    /// IAudioClient3 shared low-latency at the nearest period it allows.
    Shared,
    /// Exclusive, event-driven. Every other app on the device goes silent.
    Exclusive,
}
