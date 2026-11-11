# 0007: Each stream chooses its own playout delay

**Status:** accepted, 2026-11-06, on simulation and the offline driver. A real phone on real Wi-Fi
has not run it; [the run](#the-run-a-person-owes) is at the end, and the code stays held until
someone has done it.

## Decision

A slot's delay belongs to its stream, and the engine chooses it while the stream plays.

- A stream opens at 15 ms, what the engine has always used.
- Every half second the jitter buffer notes the slack against the due time of that half second's
  earliest and latest packets. A note is per half second, so a stall counts once however many
  packets it catches and however short the frames are.
- Once 20 notes leave at least a block of room, the delay falls, once. The anchor moves earlier
  until the latest of those packets would keep a millisecond plus a quarter of the spread over the
  block. It never goes under 3.67 ms: a block a packet can wait to be drained, and a millisecond.
- After the fall, three late notes out of 20 raise it to cover them, at most back to where it opened.
- A move waits for 21 ms of the slot under -30 dBFS. Falling decodes and drops what it skips, so the
  decoder stays whole. Rising fills with silence.
- A host that sets `playoutDelayMs` pins the delay, exactly as today. Leaving it out used to mean a
  fixed 15 ms and now means each slot chooses: a change to published behaviour, decided with the
  developer on #56. The package is before 1.0, where a minor release may change behaviour, so this
  note is the record, and the release that carries #72 says so in its notes.
- `stats()` adds `playoutDelayMs` and `lateAtOpening`, the late packets that would also have been
  late had the delay stayed where it opened.

## What changed from the issue

\#56 proposed the host choosing between songs from the last song's `stats()`, on the reasoning that
section 4 keeps a delay fixed for a song. I rejected that, for two reasons.

The first song has no history, and in karaoke the first song is the usual case: six people sharing
a night's ten songs make six of those ten someone's first. History also goes stale, and needs the
host to know a phone across songs, when the engine sees the stream it has to size. So the engine
decides, inside the stream. What this gives up is the letter of the old rule, covered below.

\#56 also says 20 ms frames arrive in 20 ms lumps a shorter delay cannot absorb. The code says
otherwise. A frame falls due when its first sample plays, and the sender has already spent the 20 ms
gathering it, so the next packet lands just as this one runs out. `measure-latency` holds rtp 20 ms
at 3.68 to 4.99 ms with no late packet. What a long frame costs is the sender's gathering, which
this cannot touch, and fewer packets to learn from.

The issue's done-when, no more late packets than a fixed 15 ms, cannot hold if the delay falls at
all, since anything between the two delays is late at one and not the other. The real question is
how many more, so the bar is at most 0.1 percent of packets.

## What others do

Read at source where the source is public.

| System                                                                                                    | What it measures                                                                       | How it sets the delay                                                                 | When it moves                                                       |
| --------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| [Jamulus](https://github.com/jamulussoftware/jamulus/blob/main/src/buffer.h)                              | Ten simulated buffers run on the real stream, each counting its errors over about 15 s | The smallest whose error rate is at or under 0.05 percent at 128 frames               | Continuously, through a slow up, faster down filter, audible or not |
| [JackTrip](https://github.com/jacktrip/jacktrip/blob/main/src/Regulator.cpp) `-q auto`                    | Packet interval, a long-term max and standard deviation                                | Max plus standard deviation plus headroom, which grows once glitches pass 0.6 percent | Once a second after a 3 s warmup                                    |
| [NetEq](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_coding/neteq/delay_manager.h) | Relative arrival delay, the max per 500 ms, in a forgetting histogram                  | The 95th percentile, 20 ms buckets, opening at 80 ms                                  | Always, by time-stretching                                          |
| [AOO](https://git.iem.at/aoo/aoo)                                                                         | Reports drops and reorders to the application                                          | A latency the application sets, changeable at any time                                | When asked                                                          |

I found nothing usable on how SoundJack sets its buffer. I took three things from these and left one.

The half-second note is NetEq's, and it is the most useful idea here. Jamulus counts errors per
access instead, and on Wi-Fi that mixes up how many packets a stall catches with how often stalls
happen. A slack measure relative to due time also deals with reordering directly: a reordered packet
is simply one with less slack.

Their floor of acceptable error is far looser than ours. Jamulus allows 0.05 percent of accesses at
128 frames, about 22 errors a minute. JackTrip tolerates about three glitches a second before it adds
headroom. Those fit musicians who choose a tool and tune it. A live singer in a room hears each one.

All three adaptive systems move continuously, and that is the part left out. Packet audio worked
this out in the 1990s: moving inside a talkspurt leaves gaps that hurt quality, so the classic schemes
move between talkspurts
([Moon, Kurose and Towsley](https://www.comp.nus.edu.sg/~cs5248/0405S1/l04/moon98playout.pdf)). This
moves in the quiet too, and rarely.

## Why RFC 3550 jitter cannot size it

[RFC 3550](https://www.rfc-editor.org/rfc/rfc3550.txt)'s `J` is a running mean of how much neighbouring
packets differ, with a gain of 1/16. A rare large spike barely moves a mean, so it reads low exactly
where the delay has to be high. Its memory is 16 packets, 40 ms at 2.5 ms frames and 320 ms at 20 ms,
so it reads whatever just happened. And on loopback at 20 ms frames it reads 1.33 ms from the block
grid alone. On the simulated links below it read 4.1 ms on a link whose spread at 99.9 percent of
packets was 51 ms, and 0.54 ms on one that needed 5.1. `jitter_ms` stays in `stats()` as a health
reading, and nothing chooses from it.

## The numbers

Everything in this section is a simulation of the rule on synthetic links, 40 seeded songs of 3.5
minutes each, because I have no real traces. The links are built from the two things I could source:
"spikes of 10 to 30 ms" from Aeon Karaoke's `performance-audio-path.md`, and
[Sui et al.](https://dl.acm.org/doi/10.1145/2906388.2906393) on 47,000 campus devices, whose round trip
is a median of 3 ms, a 90th percentile of 20 and a 99th of 250, with 5 GHz 35 percent lower on average
than 2.4. I found no published one-way distribution for a phone singing into a LAN. Trust the
comparisons more than the absolute figures.

A stall holds whatever would arrive during it. `clean5` is 1 ms plus 0.4 ms exponential, 3 to 8 ms
stalls every 5 s, a 15 to 30 ms stall a minute. `busy5` is 2 ms plus 1.2, 5 to 15 ms stalls a second,
20 to 40 ms every 20 s. `busy24` is 3 ms plus 3, 10 to 30 ms three times a second, 30 to 80 ms every
10 s. `worsens` starts as `clean5` without the big stalls and adds 5 to 15 ms a second from one minute in.

| Link     | Frame ms | Delay, mean over the song | Late events a minute, chosen and held | Concealed ms a minute, chosen and held | More late packets |
| -------- | -------: | ------------------------: | ------------------------------------: | -------------------------------------: | ----------------: |
| loopback |      2.5 |                       4.2 |                                  0, 0 |                                   0, 0 |                 0 |
| loopback |       20 |                       4.2 |                                  0, 0 |                                   0, 0 |                 0 |
| clean5   |      2.5 |                      10.5 |                            1.56, 0.91 |                              14.7, 7.0 |            0.013% |
| clean5   |       10 |                       8.7 |                            1.83, 0.60 |                              23.9, 6.6 |            0.029% |
| clean5   |       20 |                       7.0 |                            1.39, 0.36 |                              28.3, 7.3 |            0.035% |
| busy5    |      2.5 |                      15.4 |                            6.66, 6.44 |                             59.3, 57.7 |            0.003% |
| busy5    |       20 |                      15.4 |                            3.34, 2.41 |                             70.3, 50.3 |            0.033% |
| busy24   |      2.5 |                      15.0 |                              157, 157 |                             1787, 1787 |                 0 |
| busy24   |       20 |                      15.0 |                                58, 58 |                             1414, 1414 |                 0 |
| worsens  |      2.5 |                      14.6 |                            1.54, 0.21 |                               5.7, 0.5 |            0.009% |
| worsens  |       20 |                      12.0 |                            1.92, 0.13 |                              38.9, 2.6 |            0.060% |

So a clean link pays about one extra short concealment a minute for 4.5 to 8 ms, loopback pays nothing
for 11 ms, and a link that needs 15 ms stays there. A stream's own opening is not 15 ms exact, since
the first packet may itself be held up: the first packet anchors the stream, so a held `busy5` stream
really runs 15 to 21 ms. The 15.4 above is the same luck, reported honestly.

**The window.** Whatever the distribution, the worst of n half seconds covers a share `0.05^(1/n)` of
all half seconds with 95 percent confidence (Wilks), and a later half second beats it on average
every n half seconds.

| Window | Half seconds | Covers, at 95% confidence | Beaten on average every |
| ------ | -----------: | ------------------------: | ----------------------: |
| 10 s   |           20 |                     86.1% |                    10 s |
| 30 s   |           60 |                     95.1% |                    30 s |
| 60 s   |          120 |                     97.5% |                    60 s |
| 150 s  |          299 |                     99.0% |                   150 s |

No window inside a song proves the tail, which is why the rise exists at all. I chose ten seconds
because a song's intro is roughly that long and a singer should have the short delay for the first
verse, and pay for the thin evidence with margin. On `clean5` at 20 ms frames, held at 0.36 late events
a minute and 7.1 concealed ms:

| Window, margin over 1 ms      | Delay, median ms | Late events a minute | Concealed ms a minute |
| ----------------------------- | ---------------: | -------------------: | --------------------: |
| 10 s, nothing                 |              6.3 |                 1.83 |                  37.6 |
| 10 s, a quarter of the spread |              6.0 |                 1.17 |                  23.8 |
| 10 s, half of the spread      |              7.0 |                 0.86 |                  17.3 |
| 30 s, nothing                 |              9.0 |                 1.10 |                  22.7 |
| 30 s, a quarter of the spread |              9.3 |                 0.77 |                  15.3 |

Ten seconds with a margin matches thirty without one, and the singer has it twenty seconds sooner. A
quarter over a half is my call on the balance of milliseconds against clicks. At 2.5 ms frames a half
costs 2 ms more on `clean5` for 0.3 fewer events a minute.

**The floor.** 3.67 ms is a block a packet can wait, 2.67 ms, and a millisecond for what a measured link
with no jitter still has: the network thread's wake and the device's callback. Their real size is
unmeasured, so this number is the first thing the run below can change.

**Worse mid-song.** A fixed delay on a link that worsens has no way out, and the same stall that is
fine at 15 ms is late every time at 3.67. Three late half seconds out of 20 is about nine times what a
stable link shows, so it means something changed. With the rise, `worsens` pays 1.5 to 1.9 late events
a minute over the song and ends near 15 ms. Two would react sooner, at about 0.45 false rises a song on
`clean5` against 0.10 at three, measured with a 30 s window.

## The moving delay

Section 4 used to say the singer notices a moving delay more than a click, so it stays fixed for a
song. I looked for evidence and found none either way. I did find these:

- The engine already moves the delay mid-note. Skew correction drops or conceals a frame whenever the
  clocks have drifted one, with no regard for what is playing, about once every 50 s for 5 ms frames at
  100 parts per million. That rule was never true.
- A delay move is only audible through what it does to the samples, or against the singer's own voice
  once they sing again. The first is the reason for the quiet gate. For the second,
  [Schmid et al.](https://dl.acm.org/doi/10.1145/3678299.3678331) measured the just noticeable difference
  for audio latency at a mean of 27 ms and as low as 5 ms, at a 64 ms base. That is key presses and not
  a voice, so it only suggests a scale: the 5 to 11 ms a fall moves is at the sensitive end, once. And
  the fall makes the delay shorter, which is the thing a singer wants.
- A fall happens once per stream, and intros mean it usually lands before the first note.

So the rule that stays is narrower: no move while a slot is sounding, and no more of them than a song
needs. If a singer can hear it anyway, the run below is where we find out, and pinning the delay
restores the old behaviour with one option.

I rejected stretching the voice to move without a jump. NetEq does it and it would remove the gate, but
it puts a resampler or a WSOLA on the mic path, which section 4 rules out for good reason, to solve
something a breath already solves. If a slot is never quiet it stays where it opened, which is today.

## What it costs

- A 10 ms stream under 12.67 ms, its frame and a block, is no longer sure of in-band FEC. The lost
  frame decodes at the first block that needs it, up to a block before it falls due, and the next
  packet lands about 10 ms after the lost one; below 12.67 ms it is in hand only when the block falls
  late enough. SILK is the
  only layer with FEC ([RFC 6716](https://www.rfc-editor.org/rfc/rfc6716.txt)), so 2.5 and 5 ms frames
  never had it and 20 ms frames miss it at 15 ms anyway. This costs only 10 ms streams on a lossy link
  and I expect few of those: mediasoup sends 20 and aeonentertainment/aeon-karaoke#335's native module will send 2.5 or 5.
- A stream with long frames shows a short stall less often. A 3 ms stall catches one 20 ms frame in
  seven, so ten clean-looking seconds are easy to get and the fall can go too far. In a test at one such
  stall a second, a stream fell to the floor and lost 4 packets in a minute, 0.13 percent, a little over
  the bar, with no rise since they were too thin to trigger one.
- Ten seconds of a 200 part-per-million drift read as 2 ms of spread. The delay falls that much less
  than it could. I left it, since it fails safe.
- If every packet arrives late for a second, the stream ends and opens again at 15 ms, as any gap does.
  At 3.67 ms a smaller step in the link can do that than at 15.
- A song's first ten seconds or so, or longer if the singer never pauses, run at 15 ms.

## What `measure-latency` says

Offline, so no network at all: the chosen rows are the least a slot can hold. On dddb193 plus this
change, in ms. The chosen delay also takes away the block's wait a held stream keeps for good, which
narrows the range, because the fall measures from the earliest packet.

| Path                 | Engine holds at 15 |    Total at 15 | Engine holds, chosen |  Total, chosen |
| -------------------- | -----------------: | -------------: | -------------------: | -------------: |
| rtp 20 ms            |     15.01 to 17.66 | 41.51 to 44.16 |         3.68 to 4.99 | 30.18 to 31.49 |
| rtp 10 ms            |     15.01 to 17.66 | 31.51 to 34.16 |         3.68 to 4.32 | 20.18 to 20.82 |
| rtp 5 ms             |     15.02 to 17.67 | 26.52 to 29.17 |         3.69 to 4.00 | 15.19 to 15.50 |
| rtp 2.5 ms           |     15.02 to 17.67 | 24.02 to 26.67 |         3.69 to 3.83 | 12.69 to 12.83 |
| rtp 5 ms low-delay   |     15.02 to 17.67 | 22.52 to 25.17 |         3.69 to 4.00 | 11.19 to 11.50 |
| rtp 2.5 ms low-delay |     15.02 to 17.67 | 20.02 to 22.67 |         3.69 to 3.83 |   8.69 to 8.83 |
| l16 7.3 ms           |     15.02 to 17.67 | 22.31 to 24.96 |         3.69 to 3.71 | 10.98 to 11.00 |

## The run a person owes

Real Wi-Fi needs a person, a phone and a room. Because the slot counts `lateAtOpening` as it goes, one
song gives both sides of the comparison on the same packets and the same radio, so there is nothing to
run twice and no second song whose Wi-Fi differed.

1. In Aeon Karaoke, on a branch: drop `playoutDelayMs: AEON_PLAYOUT_DELAY_MS` from `AeonEngineHost.ts`
   so slots choose, and add `playoutDelayMs` and `lateAtOpening` to the slots in
   `AeonMixingEngine.sampleStats`, so they reach the stats log. Use this package's build.
2. Start the device service with `MIXER_ENGINE=aeon STATS_LOG_DIR=/tmp/aeon-stats`.
3. Sing a whole song from a phone on a clean 5 GHz network. Then another with someone streaming video
   on the same access point.
4. Read each song's last sample, from the newest file in that directory:

   ```sh
   jq -c 'select(.mixer) | .mixer.slots[] | select(.channelId != null)
     | {pushed, lost, late, lateAtOpening, playoutDelayMs, avgJitterMs}' FILE | tail -1
   ```

It passes if the delay is under 15 on the clean network, if `(late - lateAtOpening) / pushed` is at most
0.001 on both, and if the singer did not hear it move. Put the numbers, the phone and the access point
here. They replace the simulation as the reason for the constants in `delay_chooser.rs`.

## Not measured

- Anything on real Wi-Fi. The simulated links are my guess at a shape, and the constants rest on them.
- Whether -30 dBFS finds a gap on a real phone in a loud room. Backing bleed could hold the gate shut,
  and then the slot stays at 15 ms, losing the gain and costing nothing else.
- Whether a singer hears the fall.
- The real wake jitter of the network thread and the device's callback, which the floor's millisecond
  stands in for.
