//! One slot's datagrams from a capture, as the receiver would have pushed them: parsed by the
//! receiver's own `packet_from`, so RTCP and anything else that is not RTP drop out the same way,
//! on an output clock that starts at the first datagram.

use std::collections::BTreeMap;

use engine::block::RATE;
use engine::ingest::Packet;
use transport::receive::packet_from;

use crate::capture::Datagram;

/// A packet and the output frame it reached the socket at.
#[derive(Clone, Copy)]
pub struct Arrival {
    pub at: i64,
    pub packet: Packet,
}

pub struct Stream {
    pub port: u16,
    /// Every address that sent to the port, in the order each first did.
    pub senders: Vec<(std::net::Ipv4Addr, u16)>,
    pub arrivals: Vec<Arrival>,
    /// Datagrams on the port that were not RTP: RTCP, mostly.
    pub dropped: usize,
}

impl Stream {
    /// The datagrams sent to `port`, or to the one port in the capture when it is `None`.
    pub fn of(datagrams: &[Datagram], port: Option<u16>) -> Result<Self, String> {
        let port = match port {
            Some(port) => port,
            None => {
                let mut ports = BTreeMap::<u16, usize>::new();
                for datagram in datagrams {
                    *ports.entry(datagram.port).or_default() += 1;
                }
                match ports.len() {
                    0 => return Err("the capture holds no UDP datagram".into()),
                    1 => *ports.keys().next().unwrap_or(&0),
                    _ => return Err(format!("several ports, so name one with --port: {ports:?}")),
                }
            }
        };
        let on_port: Vec<&Datagram> = datagrams.iter().filter(|d| d.port == port).collect();
        let mut senders = Vec::new();
        for datagram in &on_port {
            if !senders.contains(&datagram.source) {
                senders.push(datagram.source);
            }
        }
        let first = on_port.first().ok_or(format!("no datagram to port {port}"))?.at_micros;
        let arrivals: Vec<Arrival> = on_port
            .iter()
            .filter_map(|datagram| {
                let packet = packet_from(&datagram.bytes)?;
                Some(Arrival { at: frames_of_micros(datagram.at_micros - first), packet })
            })
            .collect();
        Ok(Self { port, senders, dropped: on_port.len() - arrivals.len(), arrivals })
    }
}

/// Microseconds to the nearest output frame. A frame is 20.8 µs, so a time the recording
/// truncated to the microsecond comes back to the frame it was taken on.
pub fn frames_of_micros(micros: i64) -> i64 {
    (micros * i64::from(RATE) + 500_000).div_euclid(1_000_000)
}
