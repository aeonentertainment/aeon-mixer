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
  it pushes, and the jitter buffer holds the address of the first packet it takes as the slot's
  sender. The SSRC is no sender's proof: it is a field in the packet that anyone can copy.
- **Another address is refused.** While the sender holds the slot, a packet from another
  address is dropped before it touches the stream. It counts in no stat, and it does not keep
  the stream alive.
- **The same address, a new SSRC,** is the same phone starting its stream over, and re-anchors at
  once, as before.
- **Two things free the slot.** `resetSlot` frees it at once, which Aeon Karaoke calls when a
  channel leaves its slot. A second with no packet from the sender frees it too, the same second
  that ends a stream (`REANCHOR_GAP`), and the next packet from any address anchors a new one.
  Opus DTX sends through a pause, so a singer who stops singing keeps the slot.
- **A move keeps the sender.** A move to another output ends the stream, since the stopped
  clock paced it, and keeps its sender, whose next packet re-anchors. Another address cannot take
  the slot in the gap the move makes. Before this, a move ran the slot's reset.
- **`resetSlot` turns the old sender away.** After a reset, the sender it released is refused
  until it has been quiet a second, and each packet it sends meanwhile starts that second again.
  Without this, a packet still in flight from the old phone, or an old phone that missed its
  leave and keeps sending, would anchor the freed slot first and lock the next singer out. Any
  other address anchors at once.
- **The audio thread holds it.** The stream, the gap and `resetSlot` all live in the jitter
  buffer already, so the lock is one comparison there and no new signal crosses between the
  threads. The buffer ends a quiet stream and frees a quiet sender or released sender in one place,
  at the end of each block, so a packet never meets a stream that is already over.
- **No new API, one changed behaviour.** The host passes nothing new and no exported type
  changes. A slot now changes hands at once only through `resetSlot`. Before, a new phone's SSRC
  took the slot whether the host reset it or not, so a host that hands a slot over without
  `resetSlot` now waits for the old phone's second of quiet, and waits for good if the old phone
  keeps sending. That breaks such a host, so the change ships in the next minor release, and its
  notes say to call `resetSlot` whenever a slot's sender changes. Aeon Karaoke calls it when a
  channel leaves its slot, but not when a phone moves from the mediasoup relay to sending
  direct: that switch loses up to a second of the phone's audio, refused while the relay's
  address still holds the slot, until Aeon Karaoke calls `resetSlot` after closing the relay.

## Roaming and rejoining

A phone that moves between access points of one Wi-Fi network keeps its address: the lease is
the network's, and the source port is its socket's. Nothing changes for it.

A phone whose address does change mid-song, to another subnet or to a new socket after a rejoin,
is refused until its old stream has been quiet a second, then anchors as a new stream. Its
singer loses up to a second, concealed, that a same-SSRC move used to play through. A rejoin
with a new SSRC re-anchored before this too; the change is the second's wait.

A phone handed back the slot it was just reset out of waits for the same second of quiet, since
the slot cannot tell it from a straggler. That costs only a phone that keeps sending across the
handover, and one that keeps sending, as DTX does through a pause, does not get the slot back
until it stops.

## mediasoup and RTCP

mediasoup sends each slot from one PlainTransport on loopback, from a fixed address, so it holds
its slot the same way a phone does. RTCP multiplexed onto the port is still dropped by its payload
type in the receiver, before a packet exists to compare.

## What it does not do

- **The first sender wins.** A sender that reaches an idle slot before the phone holds it until
  `resetSlot` or a second's quiet. A released phone that keeps sending is turned away only until
  another sender takes the slot; once the slot is idle again, it is such a sender.
- **A spoofed source address gets through.** A sender on the LAN that writes the phone's address
  and port into its datagrams looks like the phone.
- **Replay sees what a capture holds.** A `recordSlot` capture keeps each sender's address, so a
  replay of it refuses a second sender as the slot did. It holds no `resetSlot`, so a handover in
  it replays as the second's wait. The libopus reference `replay` prints beside the slot decodes
  every sender, and `replay` says so when a capture has more than one. A session bundle's pcaps
  write 0.0.0.0 for a sender's IP (8.11), so a bundle replays the slot as live only while the
  port still tells its senders apart: #204.
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
