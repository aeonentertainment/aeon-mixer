//! The return branch's second half (docs/design.md 8.7): the task that sends what the
//! `Packetizer` makes. Pacing is the output clock: a packet goes out as soon as the device has
//! rendered its 480 frames and the next tick or readiness event finds it.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use engine::return_ring::ReturnConsumer;
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};

use crate::Task;
use crate::packetizer::{Packetizer, Stream};

/// The task that sends a mixer's return stream from an ephemeral port to one destination.
pub struct Sender {
    socket: UdpSocket,
    packetizer: Packetizer,
}

impl Sender {
    /// Built on the control thread from the mixer's `rtpReturn`, then handed to `Network::add`.
    /// The destination is an address, never a name to resolve: the mixer stays on the LAN and a
    /// lookup would block the control thread.
    pub fn connect(
        destination: SocketAddr,
        ring: ReturnConsumer,
        stream: Stream,
    ) -> io::Result<Self> {
        let packetizer = Packetizer::new(ring, stream).map_err(io::Error::other)?;
        let local = match destination {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        let socket = UdpSocket::bind(local)?;
        socket.connect(destination)?;
        Ok(Self { socket, packetizer })
    }

    /// Everything due goes out now. A datagram the socket will not take is dropped, not kept:
    /// by the next tick it is late audio.
    fn drain(&mut self) {
        while let Some(datagram) = self.packetizer.next_datagram() {
            let _ = self.socket.send(datagram);
        }
    }
}

impl Task for Sender {
    fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
        registry.register(&mut self.socket, token, Interest::WRITABLE)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        registry.deregister(&mut self.socket)
    }

    fn ready(&mut self) {
        self.drain();
    }

    fn tick(&mut self) {
        self.drain();
    }
}
