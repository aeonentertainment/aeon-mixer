# 0017: Recording a slot, and the file it makes

**Status:** accepted, 2026-11-10, ahead of the code in #96.

## The problem

A problem heard in a real room, such as a quiet voice, a click or a late word, could not be taken
back to a desk. #100 is the first case: a phone singer sits 30 to 40 dB too low under the engine,
and its done-when needs that phone's own stream decoded through a slot and by libopus alone, to
compare the two levels. So a slot has to record what reaches it, and a tool has to replay that
offline through the real chain. Both have to cost the mic path nothing (never-compromise 1).

## Where the recording is made

On the network thread, as the receiver reads each datagram. That is the last place the packets
exist before the audio thread, and the moment the receiver reads one is the moment the jitter
buffer could first have it, so it is the clock a replay needs. The audio thread is never involved.

The receiver pushes the packet into the slot's ring first, then hands a copy to a `Tap`. The tap
copies it into a bounded channel and returns. A writer thread for that recording takes it from
there and writes it to the file. A file write can stall for as long as the disk likes, and a
stalled network thread delivers every slot's next packet late, so the network thread never
writes. A full channel drops the record and counts it, never the packet. At 256 records, the
writer can be stalled five seconds on 20 ms frames, or 0.6 s on 2.5 ms ones, before a record is
lost.

The record is written as soon as the writer has it, one write per record. A process that dies, or
a laptop that loses power mid-song, leaves a file that reads up to its last packet.

## The format: classic pcap

| Format                       | Arrival time    | Payload | Who opens it                                           | Cost to write                     |
| ---------------------------- | --------------- | ------- | ------------------------------------------------------ | --------------------------------- |
| pcap, microseconds, Ethernet | 1 µs            | whole   | Wireshark, tcpdump, scapy, libwebrtc's NetEq tools     | 58 bytes of made-up headers each  |
| pcap, nanoseconds            | 1 ns            | whole   | Wireshark, tcpdump; libwebrtc refuses it               | the same                          |
| pcapng                       | 1 µs by default | whole   | Wireshark, tcpdump 4.x; libwebrtc only as experimental | block framing, options, more code |
| rtpdump (`#!rtpplay1.0`)     | 1 ms            | whole   | rtptools, Wireshark, libwebrtc's NetEq tools           | 8 bytes each                      |
| libwebrtc's RTC event log    | 1 µs            | none    | libwebrtc's own tools                                  | protobuf                          |
| a format of our own          | anything        | whole   | only our tool                                          | least                             |

Classic pcap with microsecond times and an Ethernet link layer, each datagram behind an Ethernet,
IPv4 and UDP header the writer makes up from the sender's address and the slot's port.

- **Arrival times stay exact enough.** A block is 2 667 µs and a frame 20.8 µs, so a microsecond
  puts every packet in the block it was read before, which is all the jitter buffer can tell.
  rtpdump's millisecond offset can move a packet across a block edge, which changes which block
  drains it and so whether it is late.
- **Other tools open it.** libwebrtc's `test/rtp_file_reader.cc`, behind `neteq_rtpplay` and
  `rtp_analyze`, accepts pcap only with the microsecond magic `0xa1b2c3d4` or its byte swap,
  version 2.4, and link type 0 or 1, as of its current source. A nanosecond file or a raw-IP link
  type would be refused there. pcapng is read there only as experimental. A Wireshark built with Opus
  plays the stream from the same file once the port is decoded as RTP.
- **It is cheap.** The headers are 58 bytes behind each datagram, made on the writer thread.
  Opus at 32 kb/s in 20 ms frames is about 6 kB a second with them.
- **A person may already have one.** A `tcpdump -w` of a phone stream on the device's interface
  is the same format, so `replay` reads both. pcapng from Wireshark converts with
  `editcap -F pcap in.pcapng out.pcap`.

Rejected:

- **rtpdump.** Millisecond arrivals, above.
- **The RTC event log.** It keeps RTP headers and no payload, by design ("Do not add code to log
  user payload data without a privacy review!" in its `.proto`). It cannot say what a slot
  decoded.
- **pcapng.** More to write and read for nothing a single stream needs, and libwebrtc reads it
  only as experimental.
- **Our own format.** Nobody else could open it, and pcap is already as cheap as the record needs.

## What a record holds

The whole datagram as the socket returned it, RTCP included, and not the parsed `Packet`. Replay
then parses it through the receiver's own `packet_from`, so a fault in the parse or the RTCP
filter replays too. The IPv4 source is the sender, the destination port the slot's. The
destination address is 0.0.0.0, because the socket is bound on every interface and a datagram
read from it does not say which one it came in on.

## Replay

`native/tools/replay` is a new tool rather than more of `measure-corrector`. That tool's input
model is a clip it encodes itself, with its own arrival type and LAN model, and its rows are about
the corrector. Replay reads a file, runs one slot and prints levels. Sharing would mean a third
mode in a tool about something else.

It maps each recorded arrival to an output frame, counted from the first datagram, and pushes
the packet into the real ring before the first block that starts after it. The phase of the
blocks against the live callback is not in the file, so a replay can anchor up to a block away
from where the live slot did. Live, the slot also discarded what reached its ring before its
first pull. The host turns recording on after `start`, so there is nothing of that in the file to
discard.

The reference is libopus with nothing in front of it. Each packet is decoded at its RTP timestamp
offset from the first packet's, by a fresh decoder in sequence order, and time no packet covers is
libopus's own concealment. That is a receiver with all the time in the world. The slot's level next
to it, and next to its own meter's `Level` events, separates three causes: a quiet meter, a quiet
jitter buffer, and a quiet phone.

## Privacy

A recording holds a singer's voice and the phone's LAN address. It is made only when a person turns
it on, stays on the device, and nothing in this package sends it anywhere (never-compromise 3).
The host chooses the path and deletes the file. Nothing here cleans up after it.

## Not yet

- A `push` slot records nothing. The issue asks for its PCM, and nothing has needed it yet.
- Stage isolation, turning effects off one at a time on the same recording, and a before-and-after
  diff, are the rest of #96.
