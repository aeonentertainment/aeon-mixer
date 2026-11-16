# 0028: An rtp slot keeps the sender it is receiving from

**Status:** accepted, 2026-11-15, ahead of the code in #201.

## The problem

An `rtp` slot listens on every interface, since phones send over the LAN, and it took a datagram
from any address. The receiver read the source with `recv_from` and dropped it, and the jitter
buffer re-anchored on any new SSRC at once. A second sender that reached a slot's port took the
slot from the singer for as long as it kept sending, and the two took turns re-anchoring it.

The LAN transport is unauthenticated by design (SECURITY.md). This narrows what an
unauthenticated sender can do inside that model. It is not authentication.

## Decision

Once a slot plays a stream, it plays only that stream's sender, until the stream ends.

- **The sender is the address.** The receiver puts each datagram's source address on the packet
  it pushes, and the jitter buffer keeps the address with the stream it anchors. The SSRC is no
  sender's proof: it is a field in the packet that anyone can copy.
- **Another address is refused.** While a stream plays, a packet from another address is
  dropped before it touches the stream. It counts in no stat, and it does not keep the stream
  alive.
- **The same address, a new SSRC,** is the same phone starting its stream over, and re-anchors at
  once, as before.
- **Ending a stream frees the slot.** `resetSlot` ends it at once, which Aeon Karaoke calls when a
  channel leaves its slot. A second with no packet from the sender ends it as it always has
  (`REANCHOR_GAP`), and the next packet from any address anchors a new one. Opus DTX sends
  through a pause, so a singer who stops singing keeps the slot.
- **The audio thread holds it.** The stream, the gap and `resetSlot` all live in the jitter
  buffer already, so the lock is one comparison there and no new signal crosses between the
  threads.
- **No API change.** The host passes nothing new.

## Roaming and rejoining

A phone that moves between access points of one Wi-Fi network keeps its address: the lease is
the network's, and the source port is its socket's. Nothing changes for it.

A phone whose address does change mid-song, to another subnet or to a new socket after a rejoin,
is refused until its old stream has been quiet a second, then anchors as a new stream. Its
singer loses up to a second, concealed, that a same-SSRC move used to play through. A rejoin
with a new SSRC re-anchored before this too; the change is the second's wait.

## mediasoup and RTCP

mediasoup sends each slot from one PlainTransport on loopback, from a fixed address, so it holds
its slot the same way a phone does. RTCP multiplexed onto the port is still dropped by its payload
type in the receiver, before a packet exists to compare.

## What it does not do

- **The first sender wins.** A sender that reaches an idle slot before the phone holds it until
  `resetSlot` or a second's quiet.
- **A spoofed source address gets through.** A sender on the LAN that writes the phone's address
  and port into its datagrams looks like the phone.
- **A flood still costs.** Refused packets fill the slot's ring before the audio thread drops
  them, so a flood can crowd out the singer's packets. That is a denial of service, not a
  takeover, and outside this decision.

## The costs

One address comparison per packet on the audio thread. `Packet` grows by a `SocketAddr`, 32 bytes
beside its 1500-byte payload, in a 64-packet ring. No buffer, no latency.

## What others do

- **mediasoup's PlainTransport** in comedia mode takes the remote address from the first RTP
  packet and never replaces it. A later packet from another address is dropped and logged as
  "ignoring RTP packet from unknown IP:port" (`worker/src/RTC/PlainTransport.cpp`,
  `OnRtpDataReceived`, on the `v3` branch). A slot is reused across singers, so here the lock
  ends with the stream.
- **RFC 3550 section 8.2** keeps the first source transport address seen for each SSRC and
  treats a packet with that SSRC from another address as a collision or a loop. That stops a
  copied SSRC and nothing else: a sender with a new SSRC would still take the slot.

## Rejected

- **The host names the sender.** Aeon Karaoke's grant flow knows the phone's IP, but the source
  port is the phone's socket's, and mediasoup's is on loopback. It would close the window before the
  first packet, but it changes `MixerConfig` or the slot settings, a versioned API change, for
  a race the lock already narrows to a slot's first packet. It stays open as an additive change
  if that window matters.
- **The receiver holds the lock.** The network thread has no output clock to measure the
  second's gap on and no sight of `resetSlot`, so releasing the lock would need a new signal
  from the audio thread to it, for state the jitter buffer already has.
- **Locking on the SSRC.** Anyone can write the singer's SSRC.
