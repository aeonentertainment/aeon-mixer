//! The control thread's end of a mixer's return stream (docs/design.md 8.7): the send task on
//! the network thread and the ring's drop count. Dropping it stops the stream.

use std::net::{IpAddr, SocketAddr};

use engine::graph::Graph;
use engine::return_ring::{ReturnDrops, return_ring};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use transport::packetizer::Stream;
use transport::send::Sender;
use transport::{Network, TaskHandle};

/// `MixerConfig.rtpReturn`, range-checked in `src/` before it crosses.
#[napi(object)]
pub struct RtpReturnInput {
    /// An IP literal: a name would be resolved on the control thread.
    pub host: String,
    pub port: u16,
    pub payload_type: u8,
    pub ssrc: u32,
}

/// What `Mixer.stats` says about the return stream.
#[napi(object)]
pub struct RtpReturnStats {
    /// Blocks the audio thread dropped because the network thread had not drained the ring.
    pub dropped_blocks: u32,
}

pub struct RtpReturn {
    _sender: TaskHandle,
    drops: ReturnDrops,
}

impl RtpReturn {
    /// Wires the master of `graph` to a send task on the network thread. The graph is not live
    /// yet, so the stream starts with its swap.
    pub fn open(input: &RtpReturnInput, graph: &mut Graph, network: &Network) -> Result<Self> {
        let host: IpAddr = input.host.parse().map_err(|_| {
            Error::from_reason(format!("rtpReturn.host must be an IP address, got {}", input.host))
        })?;
        let (producer, consumer) = return_ring();
        let drops = consumer.drops();
        let stream = Stream { payload_type: input.payload_type, ssrc: input.ssrc };
        let sender = Sender::connect(SocketAddr::new(host, input.port), consumer, stream)?;
        let handle = network.add(sender)?;
        graph.set_return(producer);
        Ok(Self { _sender: handle, drops })
    }

    pub fn stats(&self) -> RtpReturnStats {
        RtpReturnStats { dropped_blocks: self.drops.read() }
    }
}
