//! One slot's socket: bound on the control thread, read on the network thread into the slot's
//! packet ring. Both ingest kinds arrive as RTP, Opus from a phone or mediasoup and L16 from
//! the house-mic sender, so a receiver does not care which; the slot's decoder does.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use engine::ingest::{Packet, PacketProducer};
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};

use crate::Task;
use crate::rtp::Header;

/// Room for a datagram over the MTU, so an oversized one is read and dropped rather than cut.
const DATAGRAM: usize = 2048;

pub struct Receiver {
    socket: UdpSocket,
    packets: PacketProducer,
    datagram: Box<[u8; DATAGRAM]>,
}

impl Receiver {
    /// Binds on every interface at `port`, or at a port of the OS's choice for `None`, which
    /// `port` reports. The phone's grant flow and mediasoup send to whichever it is.
    pub fn bind(port: Option<u16>, packets: PacketProducer) -> io::Result<Self> {
        let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port.unwrap_or(0)));
        let socket = UdpSocket::bind(address)?;
        Ok(Self { socket, packets, datagram: Box::new([0; DATAGRAM]) })
    }

    pub fn port(&self) -> io::Result<u16> {
        self.socket.local_addr().map(|address| address.port())
    }
}

/// An RTP datagram as the ring's packet; `None` for anything else, which is dropped. mediasoup
/// multiplexes RTCP onto the RTP port, and a report parses as RTP with a payload type in RFC
/// 5761's reserved range and a timestamp where the SSRC goes, which would re-anchor the stream.
pub fn packet_from(datagram: &[u8]) -> Option<Packet> {
    let (header, payload) = Header::parse(datagram)?;
    if RTCP_PAYLOAD_TYPES.contains(&header.payload_type) {
        return None;
    }
    Packet::new(header.seq, header.timestamp, header.ssrc, header.marker, payload)
}

/// RFC 5761 section 4: RTCP packet types 192 to 223 read as these payload types.
const RTCP_PAYLOAD_TYPES: std::ops::RangeInclusive<u8> = 64..=95;

impl Task for Receiver {
    fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
        registry.register(&mut self.socket, token, Interest::READABLE)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        registry.deregister(&mut self.socket)
    }

    fn ready(&mut self) {
        loop {
            match self.socket.recv_from(&mut self.datagram[..]) {
                Ok((len, _)) => {
                    if let Some(packet) = packet_from(&self.datagram[..len]) {
                        self.packets.push(&packet);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rtp_datagram_becomes_a_packet_and_anything_else_is_dropped() {
        let mut datagram = [0u8; 14];
        Header { marker: false, payload_type: 111, seq: 5, timestamp: 960, ssrc: 42 }
            .write((&mut datagram[..12]).try_into().unwrap());
        datagram[12..].copy_from_slice(&[1, 2]);
        let packet = packet_from(&datagram).unwrap();
        assert_eq!((packet.seq, packet.timestamp, packet.ssrc, packet.marker), (5, 960, 42, false));
        assert_eq!(packet.payload(), &[1, 2]);
        assert!(packet_from(b"not rtp at all").is_none());

        // A sender report on the same port: version 2, packet type 200, then its own fields
        let mut report = [0u8; 28];
        report[0] = 0x81;
        report[1] = 200;
        assert!(packet_from(&report).is_none(), "RTCP never reaches the ring");
    }
}
