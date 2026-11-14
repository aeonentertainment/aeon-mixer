//! The device follower of docs/design.md 8.3: it keeps the engine on the room's output, moving it
//! when the default changes or the open device goes. Decision 0016 has the reasons. Pure Rust: the
//! OS sits behind `System`, so tests run it on fake devices.

mod current;
mod dock;
mod follower;
mod following;
mod playing;
mod refused;
mod start_error;
mod system;

pub use current::Current;
pub use follower::Follower;
pub use playing::Playing;
pub use refused::Refused;
pub use start_error::StartError;
pub use system::System;

#[cfg(test)]
mod tests;
