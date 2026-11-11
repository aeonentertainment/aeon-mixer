//! A slot's recording (docs/design.md 8.4, decision 0017): every datagram its receiver reads,
//! with the moment it read it, in a pcap file a replay can feed through the real chain. The
//! receiver hands each datagram to a `Tap` on the network thread; the tap copies it into a
//! bounded channel; a writer thread per `Recording` writes it out, so no file write ever happens
//! on the network thread.

pub mod pcap;
pub mod record_switch;
pub mod recording;
pub mod tap;

pub use record_switch::RecordSwitch;
pub use recording::{Recording, Written};
pub use tap::Tap;
