//! The return branch of docs/design.md 8.7: the master's mix, from the return ring to mediasoup
//! as 10 ms Opus frames over RTP. The `Packetizer` turns the ring into datagrams and knows no
//! socket; the `Sender` is the task that puts them on the wire. Pacing is the output clock: a
//! packet goes out as soon as the device has rendered its 480 frames.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use engine::block::RATE;
use engine::return_ring::ReturnConsumer;
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};

use crate::Task;
use crate::rtp::{HEADER_LEN, Header};

/// 10 ms at 48 kHz, the frame the display's jitter buffer is tuned for.
pub const FRAME: usize = 480;

/// libopus's largest single-frame packet.
const MAX_PAYLOAD: usize = 1275;

/// The loss the encoder plans for. In-band FEC spends no bits until this is above zero; 10 % is
/// what a busy room's Wi-Fi drops on a bad minute.
const EXPECTED_LOSS_PERCENT: i32 = 10;

/// What every packet of the stream says about itself: the receiver's codec mapping and the
/// stream's identity, both set by whoever opened the receiving end.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    pub payload_type: u8,
    pub ssrc: u32,
}

pub struct Packetizer {
    ring: ReturnConsumer,
    encoder: opus::Encoder,
    header: Header,
    frames: [[f32; 2]; FRAME],
    datagram: [u8; HEADER_LEN + MAX_PAYLOAD],
}

impl Packetizer {
    /// Stereo Opus for music, FEC on. The first sequence number and timestamp are random, as RFC
    /// 3550 asks, so a new mixer reusing the SSRC does not look like a replay of the last one.
    pub fn new(ring: ReturnConsumer, stream: Stream) -> Result<Self, opus::Error> {
        let mut encoder =
            opus::Encoder::new(RATE, opus::Channels::Stereo, opus::Application::Audio)?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(EXPECTED_LOSS_PERCENT)?;
        let seed = RandomState::new().build_hasher().finish();
        let header = Header {
            marker: true,
            payload_type: stream.payload_type,
            seq: seed as u16,
            timestamp: (seed >> 32) as u32,
            ssrc: stream.ssrc,
        };
        Ok(Self {
            ring,
            encoder,
            header,
            frames: [[0.0; 2]; FRAME],
            datagram: [0; HEADER_LEN + MAX_PAYLOAD],
        })
    }

    /// The next datagram once the ring holds a frame's worth, `None` until then. The marker bit
    /// is on the first packet only. A frame libopus refuses is skipped: its timestamp passes and
    /// its sequence number does not, which a receiver reads as a gap.
    pub fn next_datagram(&mut self) -> Option<&[u8]> {
        if !self.ring.pop_exact(&mut self.frames) {
            return None;
        }
        let (head, payload) = self.datagram.split_first_chunk_mut::<HEADER_LEN>()?;
        let encoded = self.encoder.encode_float(self.frames.as_flattened(), payload);
        let header = self.header;
        self.header.timestamp = header.timestamp.wrapping_add(FRAME as u32);
        let len = encoded.ok()?;
        header.write(head);
        self.header.marker = false;
        self.header.seq = header.seq.wrapping_add(1);
        Some(&self.datagram[..HEADER_LEN + len])
    }

    /// Blocks the audio thread dropped because the ring was full.
    pub fn dropped(&self) -> u32 {
        self.ring.dropped()
    }
}

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

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use engine::block::BLOCK;
    use engine::command::event_ring;
    use engine::driver::OfflineDriver;
    use engine::graph::Graph;
    use engine::ingest::Codec;
    use engine::params::Params;
    use engine::return_ring::return_ring;
    use engine::slot::Slot;
    use engine::slot::source::Source;

    use super::*;

    const STREAM: Stream = Stream { payload_type: 111, ssrc: 0xA301 };

    /// A 440 Hz sine at half scale, centre-panned, so the mix is the same on both sides.
    struct Sine(f32);

    impl Source for Sine {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = 0.5 * self.0.sin();
                self.0 = (self.0 + TAU * 440.0 / RATE as f32) % TAU;
            }
        }
    }

    /// The offline driver rendering one slot into a master with a return ring, and the
    /// packetizer on the other end of it.
    fn rig() -> (OfflineDriver, Packetizer) {
        let (producer, consumer) = return_ring();
        let mut graph = Graph::new(vec![Slot::new(Box::new(Sine(0.0)))]);
        graph.set_return(producer);
        let params = Params::default();
        let (mut events, _drain) = event_ring();
        let driver =
            OfflineDriver::new(Box::new(move |out| graph.render(&params, out, &mut events)));
        (driver, Packetizer::new(consumer, STREAM).unwrap())
    }

    #[test]
    fn one_packet_per_480_rendered_frames_in_sequence() {
        let (mut driver, mut packetizer) = rig();
        let mut block = [0.0; 2 * BLOCK];
        let mut headers = Vec::new();
        for _ in 0..100 {
            driver.pump_into(&mut block);
            while let Some(datagram) = packetizer.next_datagram() {
                let (header, payload) = Header::parse(datagram).unwrap();
                assert!(!payload.is_empty());
                headers.push(header);
            }
            let rendered = driver.frames_rendered() as usize;
            assert_eq!(headers.len(), rendered / FRAME, "after {rendered} frames");
        }

        assert!(headers[0].marker && headers[1..].iter().all(|h| !h.marker));
        assert!(headers.iter().all(|h| (h.payload_type, h.ssrc) == (111, 0xA301)));
        for pair in headers.windows(2) {
            assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1));
            assert_eq!(pair[1].timestamp, pair[0].timestamp.wrapping_add(FRAME as u32));
        }
        assert_eq!(packetizer.dropped(), 0);
    }

    // The ingest decoder folds the stereo mix to mono, which for a centred slot is either side.
    // Opus is perceptual, so the tolerance is a signal-to-error ratio, not a per-sample bound:
    // 25 dB, where this tone measures about 33
    #[test]
    fn the_payload_decodes_back_to_the_rendered_mix() {
        const MIN_SNR_DB: f32 = 25.0;
        let (mut driver, mut packetizer) = rig();
        let lookahead = packetizer.encoder.get_lookahead().unwrap() as usize;
        let mut decoder = Codec::opus().unwrap();
        let mut rendered: Vec<f32> = Vec::new();
        let mut decoded: Vec<f32> = Vec::new();
        let mut block = [0.0; 2 * BLOCK];
        let mut frame = [0.0; FRAME];
        for _ in 0..(RATE as usize / 2).div_ceil(BLOCK) {
            driver.pump_into(&mut block);
            rendered.extend(block.iter().step_by(2));
            while let Some(datagram) = packetizer.next_datagram() {
                let (_, payload) = Header::parse(datagram).unwrap();
                assert_eq!(decoder.decode(payload, &mut frame), FRAME);
                decoded.extend(frame);
            }
        }

        // Past the codec's start-up, the decoded audio is the rendered audio about `lookahead`
        // late; libopus does not promise the exact sample, so the best of a few lags is taken
        let settled = 4 * FRAME;
        let decoded = &decoded[settled..];
        let snr_db = |lag: usize| {
            let late = &rendered[settled - lag..][..decoded.len()];
            let signal: f32 = late.iter().map(|r| r * r).sum();
            let error: f32 = decoded.iter().zip(late).map(|(d, r)| (d - r).powi(2)).sum();
            10.0 * (signal / error).log10()
        };
        let snr_db = (lookahead - 8..=lookahead + 8).map(snr_db).fold(f32::MIN, f32::max);
        assert!(snr_db > MIN_SNR_DB, "{snr_db:.1} dB");
    }
}
