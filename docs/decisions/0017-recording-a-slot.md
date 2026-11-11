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
copies it into a ring of 256 records, allocated and touched on the control thread when the
recording begins, and returns. A writer thread for that recording drains the ring every 20 ms and
writes each record to the file. A file write can stall for as long as the disk likes, and a stalled
network thread delivers every slot's next packet late, so the network thread never writes. A full
ring drops the record and counts it, never the packet. The writer can be stalled five seconds on
20 ms frames, or 0.64 s on 2.5 ms ones, before a record is lost.

The first build woke the writer on every record, through a channel it blocked on. Measured with
`cargo run --release -p transport --example tap_cost` on an Apple M-series laptop, 4000 records
paced 2.5 ms apart, the network thread's added cost per datagram was:

| Recording                    | Median       | p99        | p99.9      | Worst      |
| ---------------------------- | ------------ | ---------- | ---------- | ---------- |
| off                          | 0.04-0.08 µs | 0.1-0.3 µs | 0.2-2.3 µs | 0.3-192 µs |
| on, a wake per record        | 5.1-5.8 µs   | 19-22 µs   | not taken  | 77-616 µs  |
| on, ring drained every 20 ms | 0.4-0.8 µs   | 2-4 µs     | 12-36 µs   | 48-291 µs  |

"Off" is the same paced loop with no tap in it, so it is what the machine adds by itself; ranges
are over five runs. The wake was a system call per datagram. The ring costs a 2 kB copy, and most
of what is left is that copy landing on a thread that has just slept: in a burst the same copy
costs 0.08 to 0.17 µs. Before the control thread touched the ring's pages, one run's worst was
16 ms, the network thread taking the page faults. The tap's time delays only what the thread reads
next: under a microsecond at the median, against a 2.67 ms block, so the slot plays the same
packets in the same blocks.

Each record is one write. A process that dies, or a laptop that loses power mid-song, leaves a file
that reads up to the last packet the writer drained, at most 20 ms before it.

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
first pull. A host turns recording on after `start`, so there is nothing of that in the file to
discard.

The reference is libopus with nothing in front of it. Each packet is decoded at its RTP timestamp
offset from the first packet's, by one decoder in timestamp order, and time no packet covers is
libopus's own concealment. That is a receiver with all the time in the world. The slot's output is
read on the reference's clock, from the anchor the jitter buffer gave the first packet, so a
100 ms window holds the same stretch of voice in both. The slot's level next to the reference, and
next to its own meter's `level` readings, separates three causes: a quiet meter, a quiet jitter
buffer, and a quiet phone.

## Privacy

A recording holds a singer's voice and the phone's LAN address. It is made only when a person turns
it on, stays on the device, and nothing in this package sends it anywhere (never-compromise 3).
The host chooses the path and deletes the file. Nothing here cleans up after it.

## Not yet

- A `push` slot records nothing. The issue asks for its PCM, and nothing has needed it yet.
- A `measure-latency` row with recording on and off waits for that tool (#61). Its mic path is
  the offline driver with no socket, which recording never touches, so the row would read the
  same twice. What recording could cost the mic path is the network thread's time, above.
- Stage isolation, turning effects off one at a time on the same recording, and a before-and-after
  diff, are the rest of #96.
