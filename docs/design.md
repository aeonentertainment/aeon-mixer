# Engine design

**Status:** design, 31 October 2026. Built against
[what the redesign needs from the mixer](https://ek9v0cptttrf.postplan.dev/), the contract
document, and decisions [0001](decisions/0001-toolchain.md) and
[0002](decisions/0002-native-engine.md). Sections 1 to 7 are shared: read them once. Then read
the module you are changing in section 8 and the workstream that owns it in section 10.

This is the best shape we know today, not a fence. When you find a better one, change this
file: a default in the same PR as the code that needs it, with the reason; a contract, a name
or field or rule another workstream builds against, in its own PR first, so they follow.

## 1. The shape

One process hosts one engine. The engine opens the audio device once, runs the audio thread,
and renders one block at a time for as long as the process lives. A mixer is the graph for one
song: slots for the singers, a backing path for the stems, a master that sums them. The
control thread builds a mixer, hands it to the audio thread, and the audio thread fades it in
at a block boundary. When the song ends the control thread hands over the silent graph and gets
the old one back to drop; the next song's graph comes in the same way when its start is called.
Between songs the engine still renders, so reaction sounds play and the device never reopens.

```
control thread (Node)                                  audio thread (OS callback)
TypeScript API ─► napi ─► Params snapshot ───────────► read once per block, ramp
                       ─► Command queue ─────────────► Swap, Start, Seek, Pause, Key, StopAt
                       ◄─ Event queue ◄──────────────  Ack, Level, Pitch, Position, Ended
                       ◄─ Retired queue ◄────────────  the old Graph, dropped here

network thread                                         audio thread
UDP sockets ─► RTP parse ─► packet ring per slot ────► jitter buffer ─► decode ─► slot chain ─┐
                                                                                             master ─► device
worker thread                                                                                  └─► return ring ─► encode ─► RTP
stem decode, pre-render ─► Stems ────────────────────► playhead ─► stem gains ─► live key ───┘
```

## 2. The two threads

The audio thread is the OS callback. It renders one block and returns. It allocates nothing,
locks nothing, calls nothing in the OS and never panics. Its work per block is bounded by the
graph it holds: at most one Opus frame per slot, one block of every effect, one block of the
backing path.

The control thread is Node's event loop. Every API call runs there. It allocates, opens files
and sockets, builds graphs and drops them.

Three other threads exist and are not special: the network thread owns the UDP sockets, the
worker thread decodes stems and runs the pre-render, the events thread drains the event queue
into Node.

Everything that crosses to or from the audio thread crosses one of five ways. Every ring has one
producer and one consumer.

| Crossing   | Mechanism                              | Direction                                                         | Carries                                                                                                    |
| ---------- | -------------------------------------- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Parameters | triple buffer of a fixed-size `Params` | control to audio                                                  | gains, pans, mute, solo, effect knobs, stem gains, guide offsets                                           |
| Commands   | SPSC ring of `Command`                 | control to audio                                                  | `Swap`, `Start`, `Seek`, `Pause`, `Resume`, `SetKey`, `StopAt` with an id; `SetGuide`, `ResetSlot` without |
| Events     | SPSC ring of `Event`                   | audio to control                                                  | `Ack{id, position}`, `Level`, `Pitch`, `Position`, `BackingEnded`, `Underrun`, `Fault`                     |
| Retired    | SPSC ring of `Retired`                 | audio to control                                                  | graphs and guides the audio thread no longer holds, dropped on the control thread                          |
| Audio      | SPSC rings of stereo f32 frames        | control to audio for push voices, audio to network for the return | PCM; the audio thread pushes or pulls one block at a time                                                  |

Rules that follow:

- `Params` is `Copy` and fixed-size, arrays not vectors, so the triple buffer never allocates.
  The audio thread reads it once at the top of a block and ramps every gain across the block.
- A command with an id is acknowledged in the event ring with the backing position at the block
  it took effect, and the control thread resolves the promise on the ack. Every async method in
  the TypeScript API is this pattern. `SetGuide` and `ResetSlot` carry no id and their methods
  return at once.
- A full ring never blocks, and a producer never touches the consumer's index. What becomes of
  the item depends on who can afford to lose it: an event is dropped and counted, except
  `Fault`, since losing it would leave the room silent with nobody told; a command is handed
  back to the control thread as an error, since dropping a `Swap` would lose a graph; a
  command that carries a box waits in its ring until the retired ring has room for what it
  displaces, so the audio thread is never left holding a box it cannot hand back.
- The events thread drains the event ring every 5 ms and calls one napi threadsafe function per
  batch. Only the `node` crate knows this thread exists. Tests drain the ring directly.
- Render progress is the worker thread's, not the audio thread's, so it stays off the event
  ring: the job writes an atomic fraction the events thread reads on each drain.
- A `Box<Graph>` or `Box<Guide>` moves through a ring as a pointer. The audio thread never drops
  one.
- Moving to another output is not a crossing. The device follower (8.3) holds the audio thread
  only while no stream runs: it stops the old stream, which hands the audio thread back, calls
  `Movable::moved` on it, and starts the new stream with it. As each stream's `start` returns, the
  follower writes the stream's granted period and its output latency into the two `AtomicU32`s that
  `Movable::callback_frames` and `Movable::output_latency_frames` gave it before the first open. The
  audio thread reads both once a block and hands a change to every graph it holds, and a graph
  swapped in takes what it last read, so the new stream needs no command and no round trip through
  the binding.
- A panic inside render is caught at the callback boundary. Every block from there is silence,
  the audio thread pushes `Fault` again each block until the event ring has room for it, and the
  engine refuses further commands until the host closes it.

## 3. The two clocks

Mic slots run on the output clock. A block of output is a block of input from every slot's
jitter buffer, decoded now, or from a capture slot's ring. Nothing on a slot's path may delay a
sample beyond the jitter buffer's playout delay and the decoder's frame, or a capture slot's ring
and the input's period, except the corrector's shifter on a slot whose singer has asked for
correction.

The backing path runs on its own playhead, a frame counter the audio thread advances by one
block per render while playing. Key shift, time stretch, guide vocal mixing, seek and fades
live there and may cost latency, because the engine reports the playhead and the device's
lyric clock follows it.

What the engine reports so the rest of the device can follow:

- `start()` resolves with `startedAt`, the wall-clock moment the listener hears backing frame
  zero; `backingOffsetMs`, the engine's own output latency, from a backing frame rendered to
  that frame leaving the speaker; and `speedFactor`.
- A move to another output (section 5) changes the output latency, and holds a playing backing
  for the gap. `outputChanged` carries the new `latencyMs`, the gap, and `heldMs`, how long the
  move held the playhead: the gap when the backing was playing or fading out to a pause or a seek
  as the old stream stopped, 0 when it was paused, not started, ended or absent. The gap starts
  no later than the old stream's last callback, so after a stall it counts the stall (section 8.3).
  A host that keeps a lyric clock from `startedAt` moves it by `heldMs` and the change in
  latency, or re-anchors it on the next `position` event. A pause or resume still in flight at
  the move counts on the side of it where the audio thread took it, so the host keeps no paused
  state of its own to tell whether a gap counts. The guide clock moves by the new latency itself:
  from the first block after the new stream's `start` returns, every pitch reading and every note
  edge the corrector follows is on the new output's clock.
- Every ack carries the backing position at the block the command landed. `seek`, `resume` and
  `start` return it. A `position` event also fires once a second.
- When a live key shifter is engaged, its latency is subtracted from the reported position, so
  the lyric clock stays on the audio the room hears.
- A pitch reading carries the position on the guide's clock at the reading's centre. The slot's
  guide clock is the playhead less the output latency playing now, `backingOffsetMs` until a move
  and the moved-to `latencyMs` after one, less the slot's mic-path latency, which the
  engine knows, plus a host-set `guideOffsetMs` for what it cannot know: the phone's capture and
  the Wi-Fi.
- An engaged corrector delays its slot's voice by an amount that moves with the voice, and holds
  it through a rest. `stats()` gives each slot `correctorDelayMs`, the delay as it stands, and
  `correctorMaxDelayMs`, its 15 ms ceiling; both read 0 with correction off. A host shows
  the first and budgets with the second. Neither enters a clock: nothing can play a live voice
  early, and the guide clock and a pitch reading both name the corrector's input, ahead of its
  delay. Decision [0009](decisions/0009-reporting-the-corrector-delay.md) has the reasons.
- The echo canceller and the howl guard add no delay, so nothing about them enters a clock. The
  canceller's reference is the master the graph already rendered, which any echo of it trails by at
  least the output's latency, so it works in the slot's own block and waits for nothing. `stats()`
  gives each slot `echo`, what the canceller found: `pathMs` from the master being rendered to its
  echo reaching the slot, `bleedDb` how far under the master the echo arrives, `removedDb` how much
  quieter the slot came out; null while it is off or until it has found the echo and learned it. It
  gives `howlCatches`, how often the guard has ducked the slot. Decision
  [0020](decisions/0020-echo-cancelling.md) has the reasons and the numbers.

## 4. Signal path

The topology is fixed. Adding a feature means adding a stage to a chain, a stem to the backing
path or an output to the master, never a general graph.

**A slot**, in order: source, echo canceller, howl guard, high-pass, gate, pitch corrector,
meter, tone, compressor, voice, reverb, gain and pan into the stereo bus. The echo canceller and the howl
guard are off until a host turns them on, for a house mic near the speakers or a phone that might
howl; off, each hands the slot over bit for bit and does no work. The canceller comes first
because what it models is the room's path from the master to the mic, and it hears that path as
the source hands it over. After the gate, which opens and closes with the singing, the corrector,
whose delay moves with the voice, or the voice, which moves formants, the path it had to learn
would move with the singing. The meter sits after the
corrector and before the tone stage, compressor, voice, reverb and gain, so the level words tell
the singer about their singing, not the tone stage's reshaping, the compressor's evening out, the
effect or the room, and mute never changes them. Pitch readings come from the corrector, ahead of the voice, so the display's trail
and the score judge the singing. The voice moves formants only and adds no sample. It comes after
the corrector, which reads the singer's own voice, and before the reverb, so the room hears the
voice the singer chose. Decision [0014](decisions/0014-the-first-voices.md) has the reasons. The
high-pass, gate and compressor are one effect, cleanup, which a host turns on per slot. None reads
ahead or buffers, so a click leaves on the sample it entered, but the high-pass's phase shift
delays a voice's fundamental near its corner: 1.8 ms at 110 Hz, 0.4 at 220. Decision
[0012](decisions/0012-cleaning-the-voice.md) has the reasons for their order and numbers. The
tone stage makes a phone sound like a good vocal mic: bands a host hands in for a mic it knows or
a blind match for one it does not, a plosive guard, an air shelf and a de-esser. It comes after
the corrector and the meter, so every pitch reading and level word is the same with it on as off,
and before the compressor, so a boomy phone is gone, and a pop that stands far under the voice
taken down, before the compressor would duck a line for them. Its filters are minimum-phase and
read no sample ahead, so a click leaves on the sample it entered. A filter that holds no sample
still delays what it lifts, so the stage caps
that: the bands and the air may leave no frequency from 80 Hz up more than 0.2 ms late, and the
guard adds at most 0.2 ms while it ducks a pop. Decision
[0029](decisions/0029-the-tone-stage.md) has the reasons and the numbers. The
corrector costs a singer who asks for it a delay that moves with the voice,
from 1.2 ms up to 1.125 periods of it: 2.5 ms at most at 880 Hz, 11.2 at 110, 15 at the 82 Hz it
stops at. Decision [0009](decisions/0009-reporting-the-corrector-delay.md) weighs that. Off, it hands
the voice over bit for bit with no delay and still reads pitch. The source is a
jitter buffer and decoder for `rtp` and `l16` slots, a PCM ring for `push` slots, and a PCM ring
a device's capture thread fills for `capture` slots. Slots are mono and pan with a constant-power
law into the stereo bus.

**The backing path**: planar stereo stems in memory, decoded and pre-rendered at the armed key
and speed before start, read by one playhead. Each stem has a gain with a 50 ms ramp. A live
key change engages a stretcher on the summed stems with a 50 ms crossfade. Seek fades out over
20 ms, jumps, fades in over 20 ms. Pause fades out the same way and holds the playhead. `stopAt` ramps the stems to
silence over its `fadeMs` and then raises `BackingEnded`, as reaching the end of the stems does.
A live key change also transposes the melody guide every corrector follows, by the same
semitones, and so does the armed key.

**The master**: the stereo bus plus the backing path plus the engine's reaction voices, through
a soft clipper with no lookahead, metered, then to two outputs: the device, and the return ring
the network thread encodes into 10 ms Opus packets for mediasoup.

| Fixed number         | Value                                        | Why                                                                    |
| -------------------- | -------------------------------------------- | ---------------------------------------------------------------------- |
| Sample rate          | 48 000 Hz                                    | Opus's native rate; no resampling anywhere                             |
| Block                | 128 frames, 2.67 ms                          | the output quantum and every ramp's unit                               |
| Hardware period      | 128 frames, 64 on request                    | 64 ran clean only on macOS; decision 0003 has every OS's numbers       |
| Internal format      | f32, planar                                  | slots mono, buses stereo; the device crate interleaves at the edge     |
| Slots per engine     | fixed at open, default 8, at most 16         | `Params` is fixed-size                                                 |
| Playout delay        | each stream's own, 15 ms down to 3.67 ms     | sized from each stream's arrivals, moved only in quiet; see decision 7 |
| Opus frames accepted | 2.5, 5, 10, 20 ms                            | the phone's native module sends short frames; mediasoup sends 20       |
| Return branch        | 10 ms Opus frames, in-band FEC on            | what the display's jitter buffer is tuned for today                    |
| Level event          | 10 Hz, RMS and peak over 100 ms              | the device maps words at 5 Hz                                          |
| Pitch event          | every 10 ms, batched per events-thread drain | the detector's own cadence                                             |

| Ramp                       | Length                            |
| -------------------------- | --------------------------------- |
| Slot gain, pan, mute, solo | one block                         |
| Stem gain                  | 50 ms                             |
| Live key crossfade         | 50 ms                             |
| Seek and pause fades       | 20 ms each way                    |
| Graph swap fade            | 5 ms, linear                      |
| `stopAt`                   | its `fadeMs`, 3 s from the device |

## 5. Lifecycle

- **Open.** `openEngine` picks the device, or the timer clock where nothing plays locally,
  starts the driver with the silent graph, and returns once the first block has rendered. The
  start runs off Node's event loop, so the host keeps answering through the 3 s each output it
  tries may take. The engine and its audio thread last until `close`; the stream under them moves
  with the output.
  When the output it picks will not start within 3 s, or the OS lists none, it rejects with
  `OutputUnavailableError`, which names that output, and leaves no stream, listener or device
  setting behind. A start refused after 3 s may hold its device, and its rate, until the OS's own
  start returns, on a thread of the engine's that is not the host's. The host may then open
  another output from the module's `outputDevices()`, whose `transport` lets it pass over a
  virtual device that would start and play to nobody (decision
  [0025](decisions/0025-device-transports.md)).
- **Create.** `createMixer` builds the graph on the control thread: decoders, kernels, rings,
  stem buffers, everything pre-allocated. If the backing has an armed key or speed, a render
  job starts on the worker thread and `renderProgress` events report it. The mixer object
  exists but is not live. A second mixer may be created while another plays: this is how the
  next song is armed.
- **Start.** `start` waits for the render job, then sends `Swap` and `Start`. The audio thread
  fades the live graph out and the new one in over one swap fade, begins the playhead, and
  acks. `start` rejects while another mixer is live; destroy that one first.
- **Play.** Parameter calls write the snapshot. Backing calls send commands and resolve on
  their acks. Events flow out.
- **End.** `BackingEnded` fires when the stems run out or `stopAt` completes. The mixer stays
  live, slots still open, until the host destroys it: people talk through the mics after the
  song.
- **Destroy.** `destroy` swaps the silent graph in and resolves when the old graph arrives on
  the retired ring and is dropped. Slot ports close within the network thread's next tick after
  that, since that thread owns the sockets and `destroy` only queues their removal. Nothing
  waits for it: the ports are ephemeral and no caller binds one again.
- **Close.** `close` destroys any live mixer, stops the driver, joins the threads, and releases
  every mixer still held, armed or live on a faulted engine, so no capture input outlives the
  engine.
- **Move.** The engine plays on the output the host pinned with `output.device` while the OS
  lists it, and otherwise on the OS default. When that changes, because the default moved or the
  open output went away, the engine stops the old stream and starts the new one with the same
  audio thread, so the live mixer and its song carry on after a gap. The playhead holds through
  the gap, every slot starts its stream again on the new clock, a capture slot under the new
  output's callback, the first 5 ms fade in, and `outputChanged` fires with the new `output`.
  When the new output will not start within 3 s, the engine starts the one it left again, if the
  OS still lists it, and failing that the default, which only a pinned engine has not tried yet.
  With no pin the default is the output that refused, so the engine faults once the one it left
  fails too. The fallback counts as a move, with the gap the room heard, so the song carries on
  after one gap. `output.refused` names the output that would not start until the rule picks
  another output, as when the default changes or the refused output leaves the list.
  `outputChanged` fires again, with a gap of 0, when the refusal clears with no move. Decision
  [0016](decisions/0016-following-the-output.md) has the reasons and the numbers.
- **Faults.** A caught panic, or a second in which none of the output the engine should play on,
  the one it left and the default will start, emits `fault` on the engine. The second counts from
  the wake, and a start under way is never cut short for it, so with starts that hang the fault
  comes once each has been refused at 3 s. The host closes and reopens; nothing inside tries to
  recover.
- **Mixer errors.** `error` on a mixer fires once, when that mixer can no longer play its song,
  and the host ends the song and destroys it. Three things raise it: a render job that fails,
  at the moment it fails, so a bad next song is known before its turn (`start` still rejects
  with the same message); an ack that times out, because the audio thread drains commands every
  block and a second without one means the room has gone silent; and an engine fault, on every
  mixer the engine holds, so a host can treat one signal as the end of a song. A timeout is not
  itself a fault: a stall can pass, so the song ends and the engine stays open until it faults,
  on a panic or on a second with no output that will start. A lost device moves the engine, and an
  output that will not start leaves it where it was; neither faults it. Trouble a mixer plays
  through is not an error: a slot's bad, late or missing packets are concealed and counted in
  `stats()`, so one phone never ends a song for the room, and a return stream that cannot send
  costs only its remote listener, since the room still hears the mix.

## 6. Module map

```
src/                    TypeScript public API, the only thing Aeon Karaoke imports
tests/                  TypeScript tests through the binding, on the offline driver
native/
  Cargo.toml            the workspace
  engine/               pure core: no device, socket or Node
    src/block.rs        RATE, BLOCK, MAX_SLOTS, planar buffer types
    src/driver.rs       Driver trait, StreamInfo, the offline driver
    src/params.rs       Params and the triple buffer
    src/command.rs      Command, Event and the rings
    src/audio_thread.rs the callback's body: drain commands, render, fade a swap, catch a panic
    src/graph.rs        Graph: render one block
    src/slot/           Slot, its source, the effect chain, the meter
    src/ingest/         packet ring, jitter buffer, Opus and L16 decoding, push input
    src/backing/        Stems, playhead, stem gains, live key, render job
    src/master.rs       sum, soft clipper, master meter, push into the return ring
    src/return_ring.rs  the return branch's ring: whole blocks in, any number of frames out
  kernels/              C++ leaves behind extern "C": corrector, stretcher. Rust leaves: reverb, voice shifter,
                        echo canceller, howl guard
    build.rs            cc compiles cpp/
    cpp/                vendored sources with licences and the commit they came from
  transport/            the network thread: UDP receive into packet rings, RTP return sender
  devices/              Driver implementations: coreaudio, wasapi, alsa, the timer clock;
                        the measure-sink and measure-clock binaries
  tools/measure-corrector/  the corrector's rows and blind-listen set, on the offline driver
  tools/measure-latency/  the mic path's latency: the engine's share offline, the output's on hardware
  tools/replay/         a slot's recording, or a session bundle, through the real chain on the offline driver
  tools/scorecard/      session bundles replayed into numbers, compared against a baseline
  tools/measure-echo/   the echo stages' numbers beside GStreamer's, the bleed run, the live howl check
  node/                 the napi-rs binding and the events thread; the only crate that knows Node
  binding/              where `vp run build:native` writes the addon and its generated loader and types
```

Dependencies point one way: `node` uses `engine`, `transport` and `devices`; `transport` and
`devices` use `engine`; `engine` uses `kernels`; `kernels` uses nothing. `unsafe` lives in
`kernels` and `devices` only. Opus comes into `engine` through `opus` (opus-rs), the safe binding
over `opusic-sys`, which builds its bundled libopus with cmake.

## 7. Contracts

Names and fields here are what other modules build against. A field may be added; one is
removed or renamed only by a design PR.

### 7.1 Rust

```rust
// engine/src/block.rs
pub const RATE: u32 = 48_000;
pub const BLOCK: usize = 128;
pub const MAX_SLOTS: usize = 16;
pub struct Stereo<'a> { pub left: &'a mut [f32], pub right: &'a mut [f32] }   // BLOCK frames each

// engine/src/driver.rs
/// An OS audio output, implemented by the backends in `devices`. `start` hands `render` to the
/// device's callback; dropping the `Running` stops the stream.
pub trait Driver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError>;
}
pub type Render = Box<dyn FnMut(Stereo<'_>) + Send + 'static>;
pub struct StreamInfo { pub sample_rate: u32, pub period_frames: u32, pub output_latency_frames: u32 }
pub struct Running { pub info: StreamInfo, /* stop() on drop */ }
/// devices/src/timer/: the clock for a box with no sound card. `new(Arc<Stats>)`; its own thread
/// renders a block at each deadline on the OS's monotonic clock. Built without the `hardware` feature.
pub struct TimerDriver;
/// What a device follower moves from one output to the next: the engine's `AudioThread`, or a
/// test's render. `moved` is called between streams, never on the audio thread: the next block
/// starts every slot's stream again and fades in over 5 ms. It returns whether the live graph's
/// backing was playing or fading out to a pause or a seek as the stream stopped, so the gap held
/// its playhead.
pub trait Movable: Send + 'static { fn render(&mut self, out: Stereo<'_>); fn moved(&mut self) -> bool; fn callback_frames(&self) -> Arc<AtomicU32>; fn output_latency_frames(&self) -> Arc<AtomicU32>; }   // where the follower writes each stream's StreamInfo.period_frames and StreamInfo.output_latency_frames as its start returns
/// The test clock. Its caller pumps it, so it is not a `Driver`. `pump_into(&mut [f32])` is
/// allocation-free and is what tests guard; `pump(blocks) -> Vec<f32>` is the binding's convenience.
pub struct OfflineDriver;

// engine/src/params.rs
#[derive(Clone, Copy)]
pub struct Params { pub slots: [SlotParams; MAX_SLOTS], pub stems: StemGains }
#[derive(Clone, Copy)]
pub struct SlotParams { pub gain: f32, pub pan: f32, pub mute: bool, pub solo: bool, pub effects: EffectParams, pub guide_offset_ms: f32 }
#[derive(Clone, Copy)]
pub struct EffectParams { pub corrector: CorrectorParams, pub reverb: ReverbParams, pub voice: VoiceParams, pub cleanup: CleanupParams, pub echo_cancel: EchoCancelParams, pub howl_guard: HowlGuardParams, pub tone: ToneParams }
#[derive(Clone, Copy)]
pub struct VoiceParams { pub enabled: bool, pub formant_semitones: f32 }   // -12 to 12; off is the voice itself
#[derive(Clone, Copy)]
pub struct CleanupParams { pub enabled: bool }
#[derive(Clone, Copy)]
pub struct EchoCancelParams { pub enabled: bool }
#[derive(Clone, Copy)]
pub struct HowlGuardParams { pub enabled: bool }
pub const TONE_BANDS: usize = 8;
#[derive(Clone, Copy)]
pub struct ToneParams { pub enabled: bool, pub bands: ToneBands, pub plosive_db: f32, pub air_db: f32, pub de_ess_db: f32 }   // an amount at 0 leaves its part out of the path; check() -> Result<(), ToneRefusal> on the control thread, before the params cross (8.6)
#[derive(Clone, Copy)]
pub enum ToneBands { None, Given { count: u8, bands: [ToneBand; TONE_BANDS] } }   // a Match variant joins with the blind match (8.6)
#[derive(Clone, Copy)]
pub enum ToneBand { Peak { hz: f32, gain_db: f32, q: f32 }, LowShelf { hz: f32, gain_db: f32 }, HighShelf { hz: f32, gain_db: f32 } }
pub enum ToneRefusal { Late { hz: f32, delay_ms: f32 }, Lifted { hz: f32, lift_db: f32 } }   // the frequency the bands and the air treat worst, and by how much

// engine/src/command.rs
pub enum Command {
    Swap { id: u32, graph: Box<Graph> },
    Start { id: u32 },
    Seek { id: u32, to_frame: u64 },
    Pause { id: u32 },
    Resume { id: u32 },
    SetKey { id: u32, semitones: i8 },
    StopAt { id: u32, at_frame: u64, fade_frames: u32 },
    SetGuide { slot: u8, guide: Option<Box<Guide>> },
    ResetSlot { slot: u8 },
}
#[derive(Clone, Copy)]
pub enum Event {
    Ack { id: u32, position_frames: u64 },
    Level { slot: u8, rms_db: f32, peak_db: f32 },
    MasterLevel { rms_db: f32, peak_db: f32 },
    Pitch { slot: u8, reading: Reading },   // the corrector's reading as it made it
    Position { frames: u64 },
    BackingEnded,
    Underrun,
    Fault,
}
pub enum Retired { Graph(Box<Graph>), Guide(Box<Guide>) }

// engine/src/graph.rs
pub struct Graph { /* slots, backing, master; built on the control thread */ }
impl Graph {
    /// One block. Reads `params` once, writes `out`, pushes events. Allocation-free.
    pub fn render(&mut self, params: &Params, out: Stereo<'_>, events: &mut EventProducer);
}

// engine/src/slot/
pub struct Guide { pub notes: Vec<Note>, pub key_mask: u16 }   // built on the control thread, followed on the audio thread
pub struct Note { pub midi: u8, pub start_frame: u64, pub end_frame: u64 }
pub struct Reading { pub position_ms: f64, pub hz: f32, pub confidence: f32, pub note: Option<TargetNote>, pub error_semitones: f32, pub shift_semitones: f32, pub pulling: bool }   // effects/: one every 10 ms, 8.6
pub struct TargetNote { pub midi: i16, pub start_frame: u64 }   // effects/: the guide note pulled toward, after the key shift; a reading's and a `Target`'s
pub struct CorrectorDelay;  // published once a block; Slot::corrector_delay() hands the control thread its Arc
pub struct EchoReport;      // findings() -> Option<EchoFindings>, howl_catches(); published once a block; Slot::echo_report() hands the control thread its Arc
pub struct ToneReport;      // findings() -> Option<ToneFindings>: the match's band gains and counted singing, the de-esser's cut, the guard's catches; published once a block; Slot::tone_report() hands the control thread its Arc
pub struct Shared<'a> { pub any_solo: bool, pub cue: Cue, pub history: &'a EchoHistory }   // what every slot shares for one block: Slot::render(index, params, shared, out, events)

// engine/src/master.rs
impl Master { pub fn history(&self) -> &EchoHistory; }   // what the graph has sent to the device, pushed after the clipper every block

// engine/src/ingest/
pub struct Packet { pub seq: u16, pub timestamp: u32, pub ssrc: u32, pub marker: bool, pub from: SocketAddr, pub len: u16, pub payload: [u8; 1500] }   // from: the sender's address, which the receiver fills; Packet::new leaves it unspecified
pub fn packet_ring(capacity: usize) -> (PacketProducer, PacketConsumer);   // SPSC, fixed-size
pub enum Codec { Opus(OpusDecoder), L16 }   // Codec::opus() creates the decoder, on the control thread
pub struct JitterBuffer;   // new(playout_delay_frames, codec) pins the delay; chosen(codec) lets each stream choose; push(&Packet); pull one block of f32 mono; stats(); shared_stats() for the control thread
pub struct PacketSource;   // the Source of an rtp or l16 slot: new(PacketConsumer, JitterBuffer)
pub struct PushVoice;      // the Source of a push slot: an SPSC f32 stereo ring its PushProducer fills; silence when empty
pub struct CaptureVoice;   // the Source of a capture slot: capture_voice() -> (CaptureProducer, CaptureVoice), an SPSC f32 mono ring a device's capture thread fills; for_input(period_frames, hardware_frames, output_frames) -> Result before it goes live: the input's granted period, what the OS reports for its hardware, and the output's normal callback, StreamInfo.period_frames; after a move the audio thread hands it the new one from Movable::callback_frames
pub struct SlotStats { pub pushed: u32, pub lost: u32, pub late: u32, pub late_at_opening: u32, pub concealed: u32, pub resyncs: u32, pub jitter_ms: f32, pub playout_delay_ms: f32 }   // late_at_opening: of late, those that would be late had the delay stayed where it opened; playout_delay_ms: zero for a push slot, a capture slot's wait from input to mix (8.4)

// transport/src/
/// What the network thread runs: one socket under one token, serviced on readiness and ticked after every poll, at least every 5 ms.
pub trait Task: Send + 'static { fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()>; fn deregister(&mut self, registry: &Registry) -> io::Result<()>; fn ready(&mut self); fn tick(&mut self) {} }
pub struct Network;        // spawn(); add(impl Task) -> TaskHandle, whose drop removes the task; drop stops the thread
pub struct Receiver;       // a Task: bind(ip: Ipv4Addr, PacketProducer) at a port of the OS's choice, putting each datagram's source address on its packet; port(); switch() -> RecordSwitch, taken before the task is added
pub struct RecordSwitch;   // the control thread's hold on a receiver's recording: set(Some(Tap)) starts one, set(None) ends it; 8.4
pub struct Recording;      // create(path, port) -> (Recording, Tap): a new pcap file and the writer thread that appends to it; finish() waits for it to close
pub struct Tap;            // the network thread's end: record(datagram, from, at) copies into a fixed ring and never blocks or wakes anyone; dropping it ends the recording
pub struct Header { pub marker: bool, pub payload_type: u8, pub seq: u16, pub timestamp: u32, pub ssrc: u32 }   // rtp.rs: parse(&[u8]) -> Option<(Header, &[u8])>; write(&mut [u8; 12])

// engine/src/backing/
pub struct Stems { pub instrumental: Planar, pub guide: Option<Planar>, pub frames: u64 }   // 48 kHz, planar f32
pub struct Playhead;       // frames, playing, seek(to), pause(), resume(), stop_at(at, fade)
pub struct RenderJob;      // control thread starts it on the worker thread; yields Stems; progress is an AtomicU32 fraction
pub struct LiveKey;        // two pre-allocated stretchers; bypassed until the first change

// kernels/src/
pub struct PitchCorrector;  // new(rate), set_target(mask, strength, retune_ms), process(in, out), reset(), detected(), latency_samples(), shift_semitones()
pub struct Stretch;         // new(channels, rate, preset), set_transpose_semitones(f32), process(planar in, planar out), latency_frames()
pub struct Reverb;          // new(rate), set(room_size, mix), process(mono in place): a slot is mono until its pan
pub struct YinDetector;     // new(rate, window_samples), analyze(window) -> Pitch: the corrector's detector
pub struct VoiceShifter;    // new(rate), set(semitones, formant_factor), process(in, out), reset(), latency_samples(), latency_bound_samples(rate, semitones, hz): pitch and formants apart, on the mic path
pub struct EchoHistory;     // new(), push(left, right) folds a block to mono, read(lag, out): the frames ending `lag` before the next one
pub struct EchoCanceller;   // new(rate), process(history, own, SourceDelay, block) in place, reset(), findings() -> Option<EchoFindings>; adds no delay
pub struct SourceDelay;     // { latency_frames: u32, spliced_frames: i64, streams: u32 }: where a slot's source has its delay this block, from Source::delay(); the splices are how far the echo has moved beyond what the latency says: every frame the source has spliced in less those it took out, and when a jitter buffer's delay moves, the part of the new delay the echo did not move by, the slack the stream had drifted into; streams counts the streams it has started, each at its latency however far the last had drifted
pub struct EchoFindings { pub path_frames: u32, pub bleed_db: f32, pub removed_db: f32 }
pub struct HowlGuard;       // new(rate), process(block) in place, reset(), catches(); adds no delay
```

### 7.2 TypeScript

```ts
export type Capability =
  | "guideVocal"
  | "key"
  | "speed"
  | "seek"
  | "shortVersion"
  | "pause"
  | "level";
export type IngestKind = "rtp" | "l16" | "push" | "capture";
export type StemKind = "instrumental" | "guideVocals";

export interface OpenEngineOptions {
  // `device` pins an output; left out, the engine follows the OS default (section 5).
  // `timer`: no device; the engine's own thread renders on the OS's clock (decision 0011)
  output: { device?: string; periodFrames?: 64 | 128 } | { timer: true } | { offline: true };
  slots?: number; // fixed for the engine's life, default 8, at most 16
  // pins the ingest jitter buffer; left out, the profile's `ingest.playoutDelayMs` (8.12)
  playoutDelayMs?: number;
  /** The settings nobody chose in the moment (8.12). Left out, `defaultProfile`. The two options above win over it. */
  profile?: SettingsProfile;
}

/** Every setting the engine takes that is not a singer's or a host's choice in the moment (8.12). */
export interface SettingsProfile {
  version: 1;
  output: { periodFrames: 64 | 128 };
  ingest: { playoutDelayMs: number | "auto" }; // "auto": each slot chooses its own (8.4)
  slot: {
    gain: number;
    pan: number;
    effects: {
      "pitch-correction": { strength: number; retuneMs: number };
      reverb: { roomSize: number; mix: number };
      voice: { formantSemitones: number };
    };
  };
  backing: { stemGains: Record<StemKind, number> };
}
export const defaultProfile: SettingsProfile;
/** A profile from JSON. Throws naming the field that is missing, unknown or out of range, or the version it does not know. */
export function parseProfile(value: unknown): SettingsProfile;

export interface OutputInfo {
  sampleRate: 48000;
  periodFrames: number;
  latencyMs: number;
  /** The output playing now, as `outputDevices()` lists it; null on the offline clock. */
  device: { id: string; name: string } | null;
  /** The output the engine should play on, when it would not start, and what the OS said (5). */
  refused: { id: string; name: string; error: string } | null;
}

/**
 * How a device reaches the machine, as its OS or driver reports it; decision 0025 maps each OS.
 * `virtual` has no hardware behind it (BlackHole, ZoomAudioDevice, a virtual cable): it starts
 * and plays to nobody. `network` plays somewhere else, seconds late. `other` is hardware on a bus
 * not named here; `unknown` is a device the OS or driver did not say. A new OS transport maps into
 * one of these, so the union does not grow.
 */
export type Transport =
  | "builtIn"
  | "usb"
  | "bluetooth"
  | "display"
  | "network"
  | "aggregate"
  | "virtual"
  | "other"
  | "unknown";
export interface DeviceInfo {
  id: string;
  name: string;
  isDefault: boolean;
  transport: Transport;
}

/** The OS's devices with no engine open. Listing opens no stream and changes no setting. */
export function outputDevices(): Promise<ReadonlyArray<DeviceInfo>>;
export function inputDevices(): Promise<ReadonlyArray<DeviceInfo>>;

/**
 * `openEngine` rejects with this when the output it picks will not start. `device` is that output,
 * the pinned one or the default, or null when the OS lists none. Every other failure is a plain
 * `Error`. It carries no list: `outputDevices()` gives one with no engine open.
 */
export class OutputUnavailableError extends Error {
  readonly code: "output-unavailable";
  readonly device: { id: string; name: string } | null;
}

export interface Engine {
  readonly capabilities: ReadonlySet<Capability>;
  /** The output playing now: what it was opened on, and after a move, what it moved to. */
  readonly output: OutputInfo;
  /** The profile in force, with `periodFrames` and `playoutDelayMs` folded in where they were given. */
  readonly profile: SettingsProfile;
  /** The module's `outputDevices()` on a device engine; none on the timer and offline clocks. */
  outputDevices(): Promise<ReadonlyArray<DeviceInfo>>;
  /** The module's `inputDevices()`: the inputs a `capture` slot may name. None offline. */
  inputDevices(): Promise<ReadonlyArray<DeviceInfo>>;
  createMixer(config: MixerConfig): Promise<Mixer>;
  /** Room-lifetime reaction voices, four at once. False when a fifth is dropped while four play. */
  reactions: { play(pcm: Float32Array, channels: 1 | 2): boolean };
  on(event: "fault", listener: (error: Error) => void): () => void;
  /** Whenever `output` changes: after every move, the fallback from a refused output included,
   * with `gapMs` how long the room heard nothing and `heldMs` how long that held the backing's
   * playhead, `gapMs` when it was playing or fading out to a pause or a seek and 0 when not
   * (section 3); both 0 when a refusal clears alone. */
  on(
    event: "outputChanged",
    listener: (output: OutputInfo, gapMs: number, heldMs: number) => void,
  ): () => void;
  /** The mix after the clipper, reactions included, between songs too. */
  on(event: "masterLevel", listener: (rmsDb: number, peakDb: number) => void): () => void;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  pump?(blocks: number): Float32Array;
  close(): Promise<void>;
}

/** A `capture` slot's `device` is an id from `inputDevices()`, or the default input; `channel` is the device's, default 0. */
export type SlotConfig =
  | { kind: Exclude<IngestKind, "capture"> }
  | { kind: "capture"; device?: string; channel?: number };

/**
 * A stem is the encoded file at `path`, or the whole encoded file in `bytes`, decoded the same way
 * (8.5). `createMixer` reads `bytes` until it settles, so the host leaves them unchanged until then.
 * Either keeps every capability: seek, key and speed work on the decoded stem, never on the file.
 */
export type BackingStem =
  | { kind: StemKind; path: string; bytes?: never }
  | { kind: StemKind; bytes: Uint8Array; path?: never };

export interface MixerConfig {
  slots: ReadonlyArray<SlotConfig>;
  backing?: {
    stems: ReadonlyArray<BackingStem>;
    key: number; // semitones, rendered before start
    speed: number; // factor, rendered before start
    stemGains?: Partial<Record<StemKind, number>>; // a stem left out takes the profile's
    stopAtMs?: number;
  };
  /** `host` is an IP literal, never a name; `payloadType` is dynamic, 96 to 127. */
  rtpReturn?: { host: string; port: number; payloadType: number; ssrc: number };
  /**
   * Writes this mixer's session bundle into a new directory at `dir`, from now until `destroy`
   * (8.11). `createMixer` rejects when `dir` exists. A bundle holds every singer's voice.
   */
  bundle?: { dir: string };
}

export interface SlotSettings {
  gain: number;
  pan: number;
  mute: boolean;
  solo: boolean;
}
/** A parameter left out takes the profile's value (8.12). */
export type SlotEffect =
  | { kind: "pitch-correction"; enabled: boolean; strength?: number; retuneMs?: number }
  | { kind: "reverb"; enabled: boolean; roomSize?: number; mix?: number }
  | { kind: "voice"; enabled: boolean; formantSemitones?: number }
  | { kind: "cleanup"; enabled: boolean }
  | { kind: "echo-cancel"; enabled: boolean }
  | { kind: "howl-guard"; enabled: boolean }
  /**
   * Makes a phone sound like a good vocal mic and holds no sample (8.6). `bands` is the host's
   * correction for the mic it knows, at most 8, or "match" for the stage to find broad ones from
   * the singing. `plosiveDb` 0 to 12, `airDb` 0 to 6 and `deEssDb` 0 to 12 are amounts. They are
   * not in the profile: left out, each is 0 and its part is out of the path. Throws when the
   * bands and the air together leave a frequency over 0.2 ms late or lift one over 6 dB.
   * "match" throws until the blind match has its target (decision 0029).
   */
  | {
      kind: "tone";
      enabled: boolean;
      bands?: ReadonlyArray<ToneBand> | "match";
      plosiveDb?: number;
      airDb?: number;
      deEssDb?: number;
    };
/**
 * One filter of a tone stage's `bands`: the Audio EQ Cookbook's peaking or shelf biquad at
 * 48 kHz, a shelf at slope 1. `hz` 80 to 16 000, `gainDb` -12 to 6, `q` 0.3 to 8.
 */
export type ToneBand =
  | { kind: "peak"; hz: number; gainDb: number; q: number }
  | { kind: "low-shelf" | "high-shelf"; hz: number; gainDb: number };
export interface MelodyGuide {
  notes: ReadonlyArray<{ midi: number; startMs: number; endMs: number }>;
  keyMask: number; // the key's pitch classes as bits, C in bit 0
}

/** The ingest counters are section 7.1's; section 3 says what a host may do with the delays. */
export interface SlotStats {
  pushed: number;
  lost: number;
  late: number;
  concealed: number;
  resyncs: number;
  jitterMs: number;
  correctorDelayMs: number;
  correctorMaxDelayMs: number;
  echo: { pathMs: number; bleedDb: number; removedDb: number } | null;
  howlCatches: number;
  /**
   * What the tone stage did; null while it is off. `match` is where the blind match's filters
   * stand and how much singing it has counted since it last started over; null unless `bands` is
   * "match", so null until the match has its target. `deEssDb` is the most the de-esser took off
   * in the last second. `plosives` counts the pops the guard has ducked.
   */
  tone: {
    match: { bands: ReadonlyArray<{ hz: number; gainDb: number }>; countedMs: number } | null;
    deEssDb: number;
    plosives: number;
  } | null;
}

export interface Mixer {
  /** `port` is null for `push` and `capture` slots. */
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind; port: number | null }>;
  setSlot(index: number, settings: Partial<SlotSettings>): void;
  setSlotEffects(index: number, effects: ReadonlyArray<SlotEffect>): void;
  setSlotGuide(index: number, guide: MelodyGuide | null, guideOffsetMs?: number): void;
  /**
   * Forget the stream a slot was hearing; the next packets are a new singer. Call it whenever a
   * slot's sender changes: the sender it released is turned away until it has been quiet a second
   * (8.4). On a `capture` slot it resets the corrector only: the input is the same device, and its
   * ring keeps its fill.
   */
  resetSlot(index: number): void;
  /**
   * Writes every datagram that reaches an `rtp` or `l16` slot's port to a new pcap file at `path`,
   * with the moment the network thread read it, until `null`, another path or `destroy`. Throws
   * for a `push` or `capture` slot, a path that exists, or a mixer writing a session bundle,
   * whose slots already record (8.11). A recording holds a singer's voice: section 8.4.
   */
  recordSlot(index: number, path: string | null): void;
  /**
   * Interleaved stereo for a `push` slot, folded to mono. Returns the frames taken; the rest did
   * not fit. Throws on a slot of another kind or a buffer that is not whole frames.
   */
  push(index: number, pcm: Float32Array): number;
  start(): Promise<{ startedAt: number; backingOffsetMs: number; speedFactor: number }>;
  seek(toMs: number): Promise<{ positionMs: number }>;
  pause(): Promise<void>;
  resume(): Promise<{ positionMs: number }>;
  setBackingKey(semitones: number): Promise<void>;
  setStemGain(kind: StemKind, gain: number): void;
  on(event: "level", listener: (slot: number, rmsDb: number, peakDb: number) => void): () => void;
  /**
   * Every 10 ms, on the guide's clock (section 3). `targetMidi` is the guide note the corrector
   * pulls toward, after the backing's key shift: null with correction off, with no guide, or
   * between notes, where it pulls toward the key. `errorSemitones` is the class it pulls toward
   * less the reading, in the singer's own octave, so positive is flat: 0 with correction off or
   * while unvoiced.
   */
  on(
    event: "pitch",
    listener: (
      slot: number,
      positionMs: number,
      hz: number,
      confidence: number,
      targetMidi: number | null,
      errorSemitones: number,
    ) => void,
  ): () => void;
  on(event: "position", listener: (positionMs: number) => void): () => void;
  on(event: "renderProgress", listener: (fraction: number) => void): () => void;
  on(event: "backingEnded", listener: () => void): () => void;
  /** This mixer can no longer play its song; end the song and destroy it. Section 5. */
  on(event: "error", listener: (error: Error) => void): () => void;
  stats(): {
    slots: ReadonlyArray<SlotStats>;
    underruns: number;
    /** Blocks the full return ring dropped; null without `rtpReturn`. */
    rtpReturn: { droppedBlocks: number } | null;
  };
  destroy(): Promise<void>;
}
```

The binding's generated types are re-exported through `src/native.ts` and never widened. The
public types above are inferred from the implementation wherever TypeScript can reach.

`Engine` is exported as a type only, and only `openEngine` builds one. Its constructor was never
part of the contract: until now the package also exported the class, so `new Engine(options)`
compiled, but nothing documented or used it. Before 1.0, removing that export takes no major
bump, as decisions 0007 and 0009 set for other pre-1.0 changes. Exporting it as a type only
means no later refactor can change a public constructor by accident.

## 8. Modules

Each module names its purpose, the rules beyond section 2 that bind it, and what done means.

### 8.0 Skeleton: `native/Cargo.toml`, `native/engine/src/{lib,block,driver}.rs`, `native/node`, `native/binding`, `package.json`, `vite.config.ts`, `.github/workflows`, `src/native.ts`

The workspace, the binding that loads, the offline driver, and CI. It is what every other
module plugs into. It also writes the first `src/engine.ts` and `tests/engine.test.ts`, which
8.8 inherits.

Done when: `vp check`, `vp test` and `cargo test` pass on macOS, Windows and Linux in CI; a
TypeScript test pumps one silent block through the offline engine; a release carries the
prebuilt addon for every platform in one tarball, which installs with no toolchain and no
install script. The tarball is 2 MB a platform, so splitting it into optional packages per
platform waits until its size costs someone something.

### 8.1 Graph: `engine/src/{graph,params,command,master,audio_thread}.rs`, `engine/src/slot/`

The audio thread's whole job. `render` reads the snapshot, pulls one block from every slot's
source, runs the chain, sums with the backing path and the reaction voices, clips, meters,
writes the device buffer and the return ring, and pushes events.

- Solo: when any slot is soloed, every unsoloed slot's effective gain is zero. Mute wins over
  solo. The effective gain ramps across the block like any other gain change.
- Meters publish RMS and peak over the last 100 ms at 10 Hz, from after the corrector and before
  the tone stage and the compressor.
- The reaction voices are the engine's, not a graph's. The audio thread renders them once a
  block, and every graph rendering in that block sums them ahead of its master through
  `Graph::render_with`, so a reaction plays straight through a swap fade and is clipped and
  metered with the mix.
- The master pushes every block it sends to the device into its echo history, and each slot pushes
  its own share of the bus into one of its own, so its canceller can adapt without the singer's
  voice (decision 0020). The graph hands the master's to every slot in `Shared`.
- The soft clipper is transparent below its knee and never hard-clips. Its curve and knee are
  the implementer's, tested by feeding a sum above full scale and asserting no sample exceeds
  1.0 and a sum at half scale passes unchanged within 0.01 dB.

Done when: tests on the offline driver, under the allocation guard, show gain, pan law, mute
reaching silence inside one block, solo, meters within 0.1 dB of a known signal, the clipper
bounds, and a graph swap with no discontinuity above the fade's expected slope.

### 8.2 Driver and offline driver: `engine/src/driver.rs`

The clock boundary. The offline driver calls `render` on the caller's thread for as many
blocks as asked and returns the output. It is how every engine test and every TypeScript test
runs.

Done when: `OfflineDriver::pump` renders a known graph deterministically, twice the same, and
`openEngine({ offline: true })` exposes `pump` to TypeScript.

### 8.3 Devices: `devices/`, `tools/measure-sink`

One `Driver` per OS. CoreAudio with an explicit buffer frame size. WASAPI event-driven on
IAudioClient3's shared stream at the shortest period the shared engine allows; exclusive only
when asked or when shared cannot open, because it silences every other app and ran in bursts on
the laptop decision 0003 measured. ALSA direct on the hardware device with an explicit period,
and a realtime audio thread where the host grants one. Each reports what it got in
`StreamInfo`.

- `measure-sink` opens the device, runs for a minute, and prints the period, the reported
  latency, the callback timing distribution and the underrun count. Those numbers go into
  decision 0003 per OS before any crate is final.
- The timer clock is the `Driver` for a box with no sound card. A thread of ours sleeps to each
  block's absolute deadline on the monotonic clock and renders every block that has fallen due,
  at most a second's worth, so the output clock keeps to wall time. Its period is one block and
  its latency zero. The wait is the clock, between renders, as a backend's thread waits on its
  device; render itself keeps section 2's rules. `measure-clock` prints its wakes in
  `measure-sink`'s form. Decision 0011 has the numbers and what was rejected.
- Device enumeration lists outputs and inputs, marks the default and says each one's transport,
  read as decision [0025](decisions/0025-device-transports.md) maps it for each OS. Reading it
  opens no stream. A transport that cannot be read is `Unknown`, and the device stays listed.

```rust
// devices/src/output.rs
pub struct Output { pub id: String, pub name: String, pub is_default: bool, pub transport: Transport }
pub enum Transport { BuiltIn, Usb, Bluetooth, Display, Network, Aggregate, Virtual, Other, Unknown }   // the binding sends each as its TypeScript name
```

The follower keeps the engine on the room's output. It owns the running stream on a thread of its
own, so no OS listener and no Node call ever starts or stops a device.

```rust
// devices/src/
pub struct Follower;       // start(request, impl Movable, impl System, Arc<Stats>) -> Result<Self, StartError>; current(); drop stops it
pub enum StartError { Unavailable { output: Option<Output>, error: DriverError }, Backend(DriverError) }
pub struct Current;        // the follower's Playing, shared: get() on any thread but the audio thread
pub struct Playing { pub info: StreamInfo, pub output: Output, pub moves: u32, pub gap: Duration, pub held: Duration, pub refused: Option<Refused> }   // held: the gap when `moved` said it held a playhead, else zero
pub struct Refused { pub output: Output, pub error: String }   // the output the rule picks that would not start
pub trait System: Send + 'static {   // the OS behind the follower; a test passes a fake one
    fn outputs(&self) -> Result<Vec<Output>, DriverError>;
    fn driver(&self, request: Request, stats: Arc<Stats>) -> Box<dyn Driver + Send>;
    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError>;   // wakes on a default or device-list change until dropped
}
pub struct Wake;           // raise(): never blocks; wakes already pending fold into one
```

- On every wake the follower lists the outputs and works out where to play: the pinned output if
  it is listed, else the default. It moves only when that is not where it plays, so a burst of
  notifications costs one move. A stream that is lost plays nowhere, so the follower reopens even
  the same output. A list that fails to read leaves a stream that still plays alone.
- A stream that sees its device go calls `Stats::lose`, which raises the follower's wake; `fail` is
  kept for a render that panicked. A device can also stay listed and raise nothing while it stops
  asking for audio, as a CoreAudio IOProc does on a Mac asleep, and by one Mac's logs in DarkWake.
  So on every backend a playing stream with no callback for 200 ms is lost too, and the follower
  reopens it. A reopen that stays silent doubles the wait for the next, up to 6.4 s, until a
  callback comes. A stall never faults the engine.
- A move stops the old stream, so the `Movable` comes back, calls `moved`, and starts the new
  stream with it on the same `Stats`. What `moved` returns sets the move's `held`: the gap when
  it held a playhead, zero when not. As every stream's `start` returns, the first one's too, the
  follower writes its `StreamInfo.period_frames` into the movable's `callback_frames` cell and its
  `StreamInfo.output_latency_frames` into its `output_latency_frames` cell.
- The gap runs from the earlier of the wake and the old stream's last callback to the new stream
  running. The engine renders nothing after that callback, so the room hears silence from there
  and the playhead stands still. For a default change the two are within a period of each other.
  For a stall the callback comes first by the stall's 200 ms or more, and for a loss by however
  late the OS reports it. A callback counts only from the old stream's start, which ended the
  last move's gap, so a stream that never called back counts from its start and no stretch of
  silence lands in two gaps. Nothing else bounds it: a stream with no callback rendered nothing,
  so a move after a 6.4 s stall reports 6.4 s and more, because the room heard that much
  silence. Every backend the follower runs marks its callbacks with `Stats::callback`, so
  `Stats::last_callback` and the wake are read from one clock, `std::time::Instant`, the one the
  stall wait reads. Whether a gap counts time the machine slept is that clock's call on each OS.
  The second the follower waits for an output before it gives up still counts from the wake.
- A move whose output will not start starts the output it left again, if the OS still lists it,
  and failing that the default, which only a pinned engine has not tried yet, and sets
  `Playing.refused`. No output is tried twice in one attempt. The fallback counts as a move:
  `moves` goes up and `gap` is the one the room heard. The follower does not try the refused
  output again while the rule still picks it, so a refusal costs one gap, not one on every wake.
  Once the rule picks another, as when the default changes or the refused output leaves the list,
  the refusal clears, and the next time the rule picks that output it is tried afresh.
- A start that has not returned after 3 s is refused as one that failed, at open and on a move,
  on every backend. Each start runs on a thread of its own. When it is refused, the follower takes
  the `Movable` back from the render it lent, which renders silence from then on, and the start
  finishes alone: a stream it starts late is stopped there, and one that fails drops what it held.
  A stream that called back before the 3 s were up is running, so it keeps the `Movable`, and the
  follower waits for its start to return with no bound. A start refused at 3 s may hold its device
  until the OS's own start returns.
- With nothing that will start, it waits for the next wake and tries again every 100 ms until a
  second has passed since the wake, then fails the stats, which faults the engine. A try under way
  always finishes first, so with starts that hang the fault comes once each has been refused.
- `start` fails with `StartError::Unavailable` when the output the rule picks will not start, or
  the OS lists none, and `Backend` when the list, the watch or the thread fails. A failed `start`
  leaves no stream and no watch, and drops the `Movable`. The binding throws `Unavailable` as
  `OutputUnavailableError`.
- The binding reads `Current` for `engine.output`, `refused` included. Its events thread sends
  `outputChanged` whenever `moves` goes up, with the move's gap and `held`, and with both 0 when
  `refused` clears with no move. A refusal is only ever set by the fallback's move.
- The watches: CoreAudio listens on the system object for the default output and the device list,
  with `kAudioHardwarePropertyRunLoop` set to null so the HAL calls it on its own thread. WASAPI
  registers an `IMMNotificationClient` from a COM thread of its own. ALSA has nothing to listen
  to, and moves only on a loss.

`Capture` is `Driver`'s other half, and it is here before any backend has one. #40 has to record at
a real input to measure a real output, and #52 will build its house-mic input on the same thing, so
both follow this rather than each other.

```rust
// devices/src/
pub trait Capture { fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError>; }
pub type Sink = Box<dyn FnMut(Captured<'_>) + Send + 'static>;
pub struct Captured<'a> { pub samples: &'a [f32], pub at: Instant }
pub struct CaptureRequest { pub device: Option<String>, pub period_frames: u32, pub channel: u16 }
pub struct CaptureInfo { pub sample_rate: u32, pub period_frames: u32, pub channels: u16, pub input_latency_frames: Option<u32> }
pub struct Capturing { pub info: CaptureInfo, /* stop() on drop */ }
pub fn capture(request: CaptureRequest, stats: Arc<Stats>) -> Box<dyn Capture + Send>;
pub fn inputs() -> Result<Vec<Input>, DriverError>;   // Input has Output's fields
```

- `start` hands `sink` to the device's capture thread and returns once the stream runs. Dropping the
  `Capturing` stops it. The thread is realtime, so a sink allocates nothing, locks nothing and
  returns at once.
- `samples` is one channel as f32 at 48 kHz, the channel asked for. A device that cannot give 48 kHz
  fails to open, because the engine never resamples.
- `at` is when the thread woke to receive them. The samples were on the hardware before `at`, however
  late the wake was. It lets a caller compare the input with an output on the host's clock, the only
  clock two devices share.
- `input_latency_frames` is what the OS says lies between the hardware and a delivery, beyond the
  period, and none when it says nothing. No software can check it.
- `stats` records the stream as an output's does. Its underruns are frames the device lost because
  nobody took them in time, after which the count of frames delivered no longer says the time.
- It holds no ring, drift correction or slot. Those are the `capture` slot's (8.4), and a sink is
  where they attach.
- `measure-capture` opens an input and an output and pulls the input through a capture slot's ring
  and drift correction on every output block, printing once a minute how long the ring held the
  mic. It plays silence. An hour of it with the mic on another clock is decision 0018's evidence.

Done when: decision 0003 holds the measured period, latency and glitch count on a Mac, a
Windows laptop and a Linux box, and the backends pass `cargo test` with the device code behind
a feature so CI without hardware still compiles them.

### 8.4 Ingest: `engine/src/ingest/`, `transport/src/receive.rs`

The network thread owns one UDP socket per `rtp` and `l16` slot, so today's grant flow and the
mediasoup loopback keep working unchanged, polls them with a timeout, parses the RTP header and
pushes fixed-size packets into the slot's ring. The RTCP mediasoup multiplexes onto the same
port is dropped by its payload type, RFC 5761's reserved range. The audio thread drains the
ring into the jitter buffer at the top of each block.

An `rtp` slot listens on every interface, since phones send over the LAN. An `l16` slot listens
on loopback only: its sender is the house-mic sender on the device, and nothing else on the LAN
should reach that slot.

- The jitter buffer anchors the first packet's RTP timestamp to the output clock plus the
  playout delay, and a single late packet never moves that anchor. A missing packet at its due
  time is concealed by the decoder, with in-band FEC from the next packet when that is already
  in hand. A packet that arrives after its time is counted late and dropped; it never silences
  or resets the stream.
- A pause is not a loss. A phone's Opus runs with DTX, and through a pause its sequence number
  holds while its timestamp runs on, as RFC 3551 4.1 has a sender that suppresses silence do.
  Until a later packet arrives a pause and a loss look the same, so the buffer conceals the time
  and keeps waiting for the same sequence number. The first packet after a pause carries it and
  plays on time; a later one arriving first counts the awaited one lost. Lateness is judged on
  the timestamp: a packet whose time the buffer has concealed is late. The marker bit a sender
  sets after a pause is a SHOULD, so nothing relies on it.
- A slot chooses its own delay under the profile's `"auto"` (8.12), unless the host pins one
  there or with `playoutDelayMs`. A stream opens at
  15 ms. Every half second the buffer notes the slack of the half second's latest and earliest
  packets against their due times. Once 20 notes, ten seconds, leave at least a block of room,
  the anchor moves earlier, once, until the latest of them would have a millisecond and a quarter
  of the spread over the block to spare. After that fall, three late notes out of the last 20 move
  it later to cover them. No move leaves a delay under 3.67 ms, a block a packet can wait to be
  drained and a millisecond, nor over 15 ms, the delay it opened at, with no exception: a fall goes
  at least to 15 ms even when some of what it saw is then late, and a rise stops there. A move
  waits for the slot to be under -30 dBFS for 21 ms, happens at the head of what waits to play,
  drops only samples that are quiet too, and counts as a resync. The echo moves by what the move
  drops or delays, but the new delay also takes up the wait the anchor's slack had not counted, a
  late first packet's and drift's, so the part of the new delay the echo did not move by is a
  splice in the source's `delay()`, and the latency and splices together move by what the echo
  did. Decision [0007](decisions/0007-choosing-the-playout-delay.md) has the reasons and the
  numbers.
- A delay is how long the stream's earliest packets wait, from the block that drains them to their
  due time, and the buffer measures it on every stream, pinned or chosen: the most slack of any
  packet in the notes a move is chosen from, the last 20 and the half second being noted. `stats()`
  reports that as `playoutDelayMs`, and a fall and a rise move from it, so the number a host reads
  is the one the moves bound. The first packet reads as the opening delay. When it came late, the
  packets after it arrive that much earlier against their due times, and from the first of them the
  delay reads the opening delay plus that lateness: for good on a pinned stream, until the fall on a
  chosen one. Drift moves it between skew steps, as it moves the earliest packets. A packet can also
  wait up to a block before its drain, which no slack sees. Measuring moves nothing: the anchor
  stays where its first packet put it. The anchor's slack, the opening delay until a move and the
  move's delay after it, is what skew correction holds the stream at and what the source's latency
  counts, so the guide clock and the echo canceller follow the anchor as before and do not count a
  first packet's lateness.
- Skew moves the anchor one step at a time, holding the stream at the anchor's slack. Once a second
  the buffer looks at how early the last second's packets arrived against their due times, next to
  that slack. A step is a frame, or as many whole 2.5 ms as the slack holds when a frame is longer.
  When every packet came at least a step early, or the earliest did once the delay has fallen, the
  anchor moves a step earlier and that step is dropped; when even the earliest came a step late, or
  half the slack's room over a block late when that is less, it moves a step later and that step is
  concealed. Either counts as a resync, and either is a splice in the source's `delay()`, since it
  moves the echo and not the latency. These and the wait a delay move takes up are a jitter
  buffer's two kinds of splice.
  Each stream it anchors counts in `delay()`'s `streams`, so the echo canceller starts its drift
  over with it.
  Arrival slack is the measure rather than the decoded FIFO's fill, because decoding on demand
  keeps the FIFO at one frame.
- A slot keeps its sender. The receiver puts each datagram's source address on its packet, and
  the address of the first packet a slot takes holds it: a packet from another address is
  dropped before it touches the stream, counted nowhere. `resetSlot` or a second without a packet
  from the sender frees the slot, and the next packet from any address takes it, except the
  sender `resetSlot` released, which is refused until it has been quiet a second. A move to
  another output ends the stream and keeps the sender. Decision
  [0028](decisions/0028-keeping-the-sender.md) has the reasons and what it leaves open.
- A new SSRC from the slot's sender, a gap over a second on either clock, or `resetSlot` starts
  a new stream: the buffer re-anchors and the slot's corrector resets. A gap is a second without
  a packet, or a timestamp a second away from where the stream is; the second one is a sender
  that paused.
  A stream quiet for a second stops concealing, so an idle slot costs no decodes.
- Decoding happens in the callback, bounded to the packets due this block. The decoder is
  created on the control thread; libopus allocates nothing after that.
- A slot's first pull discards what its ring already holds. A mixer is armed, and its ports
  handed to the phones, before it starts; a stream anchored on that backlog would play behind
  real time for good.
- In-band FEC is SILK's, so of the frames accepted only 10 and 20 ms can carry it, and it needs
  the next packet in hand, a delay over the frame and a block. At the 15 ms a stream opens at, 10 ms
  frames get it. The packet after a lost 20 ms frame arrives 5 ms after that frame falls due, so
  mediasoup's 20 ms streams get the decoder's own concealment. A 10 ms stream whose delay has fallen
  under 12.67 ms, the frame and a block, is no longer sure of it: whether the next packet is in hand
  depends on where the block falls.
- `l16` slots take stereo 16-bit big-endian at 48 kHz, as the house-mic sender sends today, and
  fold to mono. `push` slots take interleaved stereo f32 from the control thread and fold to
  mono too. The ring holds 100 ms, and `push` returns how many frames fit; the rest is the
  host's to hand in again. The slot plays at most 30 ms behind the newest frame it holds: a pull
  that finds more drops the oldest down to 30 ms and counts a resync, so a host on another
  clock, or one catching up after a stall, is pulled back inside the budget instead of drifting
  out of it. 30 ms is three 10 ms chunks, room for a host two chunks late, and is a default
  until `measure-latency` (8.9) gives the push path a number.
- A `capture` slot is a mic on this machine, with no socket and no jitter buffer. `createMixer`
  starts its input through `devices::capture` (8.3) at the output's period, never over 1024
  frames, when the ring holds it beside that callback, or else the longest whole-block period it
  can, 896 frames under 2048, or as near as the device allows, and rejects if it cannot, or if the
  input's period is over 1024 frames, or the ring cannot hold the output's callback beside it: the
  target's ceiling and the headroom, twice the period, the callback in whole blocks and 16 frames,
  over its 4096 frames. So a 2048-frame callback, which some Bluetooth outputs grant, plays under
  any input of 1016 frames or less, and a 128-frame mic under any callback up to 3712. `destroy`
  stops it. The capture thread's sink pushes each delivery into an SPSC ring of 4096 frames, fixed
  so the audio thread never sizes it, and the slot pulls a block from it. Its headroom is what the
  fill holds over the trough just after a delivery: an input period, and the blocks an output
  callback pulls back to back after the first, three more under a 480-frame callback, since the
  input delivers nothing between them. Its first pull drops what arrived while armed. It plays
  silence until the ring holds its target and its headroom, then for one more window of 128 blocks
  while it reads the trough, and drops the excess at once, so its first sample waits as long as
  every one after it: a third of a second of silence when a song starts. An offline engine opens
  no input, so it refuses a capture slot; the timer clock opens one, since a box that plays
  nothing may still have a mic. ALSA opens a `hw:` device once, so on Linux an input feeds one
  mixer at a time, armed or live.
- The ring's fill drifts when the input runs on another clock than the output. The slot holds the
  lowest fill it sees before a pull over 128 blocks, its trough, at a target: a block to pull and
  16 frames for the wakes of two threads. A trough off target moves the delay one sample a block,
  each move spliced across the block by linear interpolation, so nothing steps. A pull the ring
  cannot fill plays what it holds and silence after. When full pulls come back within the blocks
  the granted callback takes at a wake, and never fewer than eight, the phase of two clocks or a late wake ran the ring short, and the target rises by all
  those pulls lacked and 16 frames, at most one input period over where it opened. A longer run is
  an input that stopped, and raises it only by what its first pull lacked. A trough that stays a
  whole window more than a period over the target is a stall's backlog, dropped at once to the
  target. A fill high for one wake is no backlog: an output that catches up after a late wake, as
  WASAPI shared and the timer clock do, pulls more than one callback's blocks at a wake, past the
  headroom, and drains what the input delivered in the meantime at once. The slot drops none of
  it, conceals nothing and counts no move for it. On a move to another output, the slot primes and
  settles again, silent for a third of a second, under the new callback, which the audio thread
  reads from the follower's cell at its next block. The new stream may render before its `start`
  returns and the cell is written, but nothing plays under the old headroom: a slot that re-primes
  is silent through one whole settling window, 0.34 s, before it plays a sample, and a change of
  callback while it is silent starts it over under the new one. A move to a callback the ring
  cannot hold beside the input's period cannot be refused as `createMixer` refuses one: the slot
  stays silent until the next move, drops what it captures, counts one resync and reads 0 as
  `playoutDelayMs`, and never primes under it. A move back to a callback it can hold primes again.
  On the output's clock the trough never moves and holds the target exactly, 3 ms for a
  128-frame input. Decision
  [0018](decisions/0018-capturing-a-house-mic.md) has the reasons and the numbers.
- The fill before a pull is not how long a sample waited: a pull just after a delivery finds more
  in the ring than one just before it, and its samples waited as much less. The wait holds still:
  the trough and its headroom, less where in the input's period the pulls fall, which the fill
  cannot see. Under an output that takes a block a wake the pulls fall on a step of
  `gcd(period, 128)`. Under a longer callback, one that takes three blocks on some wakes and four
  on others at 480 frames, the step is the whole input period. The slot reports the middle of that
  step, within half a step of the truth: 4.33 ms for a 128-frame input at its opening target, and
  12.33 ms under a 480-frame callback. `latency_frames`, which the guide clock subtracts, is that
  wait and what the OS reports for the input's hardware, and it moves only when the target does.
  `delay()` gives the echo canceller that latency and the frames the slot has moved its delay by
  since it opened, less the target's own moves: each splice, a gap's silence once full pulls come
  back, each backlog dropped. After an input that stopped it counts nothing until the trough is
  back on the target, since what a stopped input hands over says nothing of how far the delay
  moved, and the latency the target sets is where it ends up. `stats()` gives the wait as `playoutDelayMs`, zero until the
  slot first plays, and zero from a move to a callback its ring cannot hold until the next move.
  `concealed` counts pulls the ring could not fill, `resyncs` each move of the delay, each dropped
  backlog and each move to a callback the ring cannot hold, and the rest stay 0. While the slot is
  silent under such a callback, `latency_frames` keeps the hardware's share and the wait its target
  sets; nothing plays, so the guide clock and the canceller have nothing to follow.
- The echo canceller works on a capture slot on either clock. On the output's, as a mic on the
  same interface is, it finds the path to the frame and takes 20 dB or more out, as on any slot.
  A mic on its own clock slides the echo a few samples a second against the master; the canceller
  learns the rate and follows it, and slides its reference a sample with each splice the slot
  reports. From the fifth second it takes out within 1 dB of the same mic on the output's clock at
  a 128-frame input period, and within 3.3 dB at 480, in the second that holds a correction. A
  128-frame input's whole-period correction, every 27 s at 100 ppm, costs its second 6 to 7 dB. A
  lag the delay finder names is judged only once the source's recent splices, halving every three
  seconds, come to less than 64 frames: a 128-frame correction holds it about three seconds, and a
  32-frame one or a finer step's few frames never do.
  Decision [0022](decisions/0022-following-a-mic-on-its-own-clock.md) has the numbers.
- A slot can be recorded, so a problem heard in a real room can be replayed offline (8.9). With
  `recordSlot` on, the slot's receiver hands every datagram it reads, RTCP and all, to a `Tap`
  with the moment it read it, after the packet is in the ring. The tap copies it into a ring of
  its own and returns; a writer thread per recording drains that ring every 20 ms into the file.
  So the audio thread never knows, the jitter buffer sees the same packets in the same blocks, and
  the network thread makes no system call for it and never waits on a disk. A full ring drops the
  record, never the packet, and the recording shows a loss the slot did not have; it holds 256
  datagrams, five seconds of 20 ms frames. The file is classic pcap with microsecond times, each datagram behind made-up Ethernet,
  IPv4 and UDP headers that carry the sender's address and the slot's port, so Wireshark,
  tcpdump and libwebrtc's `neteq_rtpplay` open it. Decision
  [0017](decisions/0017-recording-a-slot.md) has the reasons.
- The control thread creates the file, never over an existing one, and writes its header, so a
  bad path fails the call. Each record is written whole, so a process that dies leaves a file
  that reads up to its last packet. A host turns recording on after `start`, so a replay begins
  where the live slot did. A `push` or `capture` slot records nothing yet.
- A recording holds a singer's voice, and `recordSlot`'s holds their phone's LAN address too. One
  is made only when the host asks for it, and nothing in this package sends it anywhere
  (never-compromise 3). The host chooses the path, owns the file from then on, and answers for
  the singer's consent. Nothing here deletes it.
- `MixerConfig.bundle` records every `rtp` and `l16` slot this way from `createMixer`, beside the
  rest of the song (8.11).

Done when: a test feeds synthetic RTP with reordering, 30 ms jitter spikes and 5 percent loss
through the offline driver and the output matches the source within the concealment's expected
error; frames of 2.5, 5, 10 and 20 ms all pass; a packet arriving late never produces a gap longer
than the frame it missed; a stream that pauses for two seconds resumes without a growing delay; a
pause under a second with its sequence numbers contiguous loses nothing and every packet after it
plays at the stream's delay; a slot left to choose falls to the floor on a clean link on every
frame size, stays exactly where a pinned slot plays on a link that needs all 15 ms, moves only in a
quiet moment, and, when its first packet came on time, loses at most 0.1 percent more of its
packets to lateness than the same song at the opening delay; and a stream whose first packet came
late reports what its earliest packets wait, pinned or chosen, while one whose first packet came
on time reports the opening delay.

### 8.5 Backing: `engine/src/backing/`

Stems in memory, one playhead, gains, fades, the live key, and the render job.

- The worker thread decodes stems with Symphonia, a pure Rust decoder, to planar f32 at 48 kHz,
  resampling any other rate with Rubato's FFT resampler. With an armed key or speed it renders
  them through one stretcher at signalsmith's default preset, both stems together so they stay
  aligned, trimming the stretcher's lead and flushing its tail. `createMixer` resolves when the
  graph is built; the backing goes in with `Graph::set_backing` as soon as the render finishes,
  so a render that fails raises `error` then, and `start` waits for a render still running.
- A stem given as `bytes` is the same file held in memory, in any format Symphonia probes from its
  content. The binding copies the bytes once, on the control thread as the render job starts, and
  the worker reads its copy as it would a file and frees it the moment that stem is decoded. So
  while the render runs the engine holds the encoded stem beside the decoded one, and once it is
  done a song costs its decoded stems, as with a path; the host's own buffer is the host's. The
  engine copies rather than borrows because the worker reads the stem off the control thread, and
  a buffer the host can still write is not safe to read there. No file is opened, read or written
  for a stem given as bytes. A stem that will not decode fails the render naming its path, or
  for bytes its kind: `the instrumental stem's bytes: <reason>`. An empty buffer is refused at
  `createMixer`.
- The playhead advances a block per render while playing, holds on pause, jumps on seek with
  its fades, and stops at `stopAt` with its fade. Both raise `BackingEnded` once. A seek or
  pause acks when it takes effect, once its fade out is done, so the ack carries the frame it
  landed on; a pause fades because a hard stop clicks. The `stopAt` fade is applied before the
  live key, so it is silent at its frame on the reported clock.
- The live key holds two pre-allocated stretchers. Until the first live change the stems pass
  through dry. A change resets the idle stretcher at the new key and feeds it for its latency,
  so the crossfade never fades into its silent start, then crossfades to it; the next change
  does the same with the other. Reported position subtracts the engaged stretcher's latency.
- The armed key plus the live key is the total transposition the correctors' guides follow.

Done when: a seek lands within one block of the asked frame and its ack reports it; a track
rendered at speed 1.09 ends within 50 ms of its expected duration; `stopAt` with a 3 s fade is
silent at the asked frame and raises `BackingEnded` once; a guide-vocal gain change ramps over
50 ms with no step; a live key change of two semitones moves a test tone by two semitones
within 5 cents after the crossfade.

### 8.6 Kernels and slot effects: `kernels/`, `engine/src/slot/effects/`

The C++ leaves and the Rust stages that drive them. The corrector's sources come from Aeon
Karaoke's `lib/dsp/pitch-corrector` at a named commit, with its vendored FFT. The stretcher is
signalsmith-stretch. The reverb is a Rust port of Freeverb. The voice shifter is Rust of our own,
[decision 0004](decisions/0004-voice-shifter.md) has why and its delay bound. Each C++ kernel gets
an `extern "C"` shim and a safe Rust wrapper; the shim is the only `unsafe`.

- The guide follower is the Rust port of what the GStreamer element did: it splits a block at
  every note edge so the target changes on the sample the guide names, and posts a reading
  every 10 ms. Position comes from the playhead as section 3 describes, which makes it exact.
- At strength zero the corrector is bypassed with no delay and still reads pitch. Its readings
  then carry no target and no error, since nothing pulls.
- A reading carries the note the follower set for the run it closes and the kernel's error at
  that reading, so the target and the error a host shows are the ones the shifter acts on.
- A reading also carries `shift_semitones`, the transposition the voice at its centre came out
  of the shifter with, and `pulling`, whether the kernel was pulling as it read: a voice it
  hears, a class to pull it to, and a strength over 0. The corrected voice sat
  `error_semitones - shift_semitones` from its class. The follower keeps the shift of its last
  32 runs by the input sample each began on, and looks up the reading's centre plus the
  shifter's delay, where that sample left. It costs a store a run and a scan of 32 a reading, in
  a fixed array. The note it pulls toward is one `note: Option<TargetNote>`, its `midi` and its
  `start_frame` on the guide's clock together, since a pitch alone cannot tell two notes in a row
  on one pitch apart, and a pitch with no start cannot be written. The slot pushes the reading
  whole as `Event::Pitch { slot, reading }`, so a new field is written once. The binding maps
  `reading.*` to the `pitch` event, `targetMidi` from `note`'s `midi`, and carries none of the
  shift, the pull or the start yet; the scorecard reads them (8.9).
- The echo canceller is SpeexDSP's MDF ported to Rust, in the slot's 128-frame block, behind a delay
  finder after AEC3's; the howl guard ducks the slot 12 dB when a narrow tone with no harmonic keeps
  growing, following it through a corrector's move and a moment unseen. Decision
  [0020](decisions/0020-echo-cancelling.md) has both. The port carries mdf.c's
  BSD licence in its file. A delay-locked loop in front of the filter follows an echo that slides
  against the master, as a mic on its own clock's does, reading the master between frames, and a
  source's splice slides the filter's windows by a sample (decision 0022).
- The slot drives its stages from `slot::effects`, each in place on its mono block.
  `EchoCanceller::process(params, history, own, source, block)` and
  `HowlGuard::process(params, block)` run first, after the source.
  `Corrector::process(params, guide, frame, semitones, block, on_reading)` borrows the slot's
  guide, takes the guide-clock frame of the block's first sample and the backing's key shift, and
  calls `on_reading` per reading for the slot to push as `Event::Pitch`.
  `Voice::process(params, block)` runs the voice shifter at the voice's formant move and no
  transposition, so it adds no sample. `Reverb::process(params, block)` takes the reverb knobs.
  `HighPass`, `Gate` and `Compressor` each take `process(&CleanupParams, block)`.
- A voice is a formant move in semitones from the singer's own. Voice names and their settings
  are presets and live with the host. Off, or on at 0, the stage hands the voice over bit for
  bit.
- Cleanup's stages are causal, with no lookahead and no frame. The high-pass is a 12 dB per
  octave Butterworth at 80 Hz, whose phase shift delays a fundamental by 1.8 ms at 110 Hz and
  0.4 at 220. The gate follows the room's own level, so it needs no threshold per phone, and
  takes up to 18 dB off between lines. The compressor is 3:1 over -30 dBFS with at most 6 dB of
  makeup, so it never brings a slot more than 6 dB closer to feedback. Off, each fades to unity
  over a block and then costs nothing.
- The high-pass and a tone section switch a biquad two ways, on purpose. The high-pass crossfades
  from dry to filtered over one block from a zeroed state, and is dry from the block after it
  turns off. A tone section glides its coefficients from flat, and after it turns off runs as flat
  until its ring settles. Side by side on 110 Hz harmonics over rumble, the glide's samples sit up
  to 0.25 of full scale from the crossfade's in the block it turns on and the six after, up to 0.26
  from dry for 16 blocks (43 ms) after off, and within 3.0e-8 of the crossfade's once settled. Moving the high-pass to the glide would remove
  about ten lines and change what a singer hears at the switch, which only a listen and a
  `measure-latency` run could clear. The crossfade stays.
- The tone stage is `slot::effects::tone`, Rust biquads of its own with no kernel, run after the
  meter: `Tone::process(params, block)`. In order it is the plosive guard, the bands, the air
  shelf and the de-esser, so the de-esser judges the sibilance that leaves the stage. The match,
  when it runs, takes the bands' place, after the guard so it never averages a pop. Decision
  [0029](decisions/0029-the-tone-stage.md) has each part's reasons and its starting numbers. In
  8.1's `params.rs` and `slot/mod.rs`, and in 8.8's `src/` and `node/`, it adds its own lines and
  changes nothing else: the params, its place after the meter, its report, the effect's
  validation and stats, and the binding's tone settings, `checkTone` and tone stats. The one
  existing line it changes is the binding's `SlotParamsInput` conversion, which becomes fallible,
  since a band count over eight has to fail there.
- Bands are a capture profile: what a host knows of one mic, as up to eight filters that undo its
  colour and move it to the curve of a good vocal mic. The engine knows no phone model. The host
  owns the table and whatever keys it. Each band is the Audio EQ Cookbook's peaking or shelf
  biquad at 48 kHz, a shelf at slope 1, so a host's fitting tool and the engine draw the same
  curve from the same numbers. A profile is true for audio as the mic heard it: a phone that
  sends its own voice processing or automatic gain has a colour that moves, and no band undoes
  that.
- A minimum-phase filter holds no sample, but it delays what it lifts and the skirts of what it
  cuts: a 12 dB lift of Q 4 at 100 Hz leaves 100 Hz 19 ms late. So `ToneParams::check` judges the
  bands and the air shelf as one cascade from 80 Hz to 16 kHz, and `setSlotEffects` throws when
  any frequency comes out over 0.2 ms late, the phase the voice stage already costs, or lifted
  over 6 dB, the most cleanup's makeup may bring a slot toward feedback. The error names the
  frequency and the figure. The check is the engine's, so the filters judged are the filters
  that run.
- The match is for a mic the host has no bands for. It keeps the level of the singing in a few
  broad bands, each against the mean of them all so that loudness cancels, and moves three broad
  filters toward the curve of a good vocal mic, an engine constant. It cannot tell a phone's
  colour from a singer's own, so everything about it is bounded:
  - It counts a block only while the corrector's last reading is a voice it is sure of,
    confidence over 0.9, and the slot stands 12 dB over the room's level, which a follower like
    the gate's keeps on the stage's own input. A quiet room and the backing in the mic between
    lines count for nothing, and time that does not count moves nothing: the filters hold.
  - It moves nothing until it has counted 5 s. After that each filter moves at most 0.5 dB for
    each second counted, to at most 6 dB from flat, and never to a setting `check` would refuse.
  - It reads the voice ahead of its own filters, so what it does is a function of what it heard
    and it cannot chase itself.
  - It starts over, its count at zero and its filters ramping to flat, only on `resetSlot`, which
    a host calls when a slot's sender changes. A pause, a gap or a new SSRC from the same phone
    keeps what it learned.
  - Until it has its target (decision 0029), it does not run: `ToneBands` has no `Match`
    variant, `Tone::process` takes no `voiced`, the corrector's word that its last reading was a
    voice it is sure of, and `setSlotEffects` refuses `bands: "match"` by name. All three join
    when the match does.
- The plosive guard is a first-order low shelf that ducks by `plosiveDb` while the band under
  60 Hz stands 21 dB over the voice above 300 Hz, and lets go over 50 ms. It ducks only a pop
  that stands far under the voice, and never a sung note: a low note's first period, before its
  harmonics over 300 Hz arrive, is little but low end, so a guard that caught more would duck
  its onset. At rest it is out of the path. While it ducks it leaves the voice just over its
  corner at most 0.2 ms late. The de-esser is a high shelf whose cut follows how far the voice
  over 5 kHz stands above the rest of it, to at most `deEssDb`, so it needs no threshold per
  phone; while it cuts, nothing is over 0.05 ms late. Neither reads ahead, so the first
  millisecond of an s passes, and the first 7 ms or so of a pop, which the guard's band under
  60 Hz takes to rise. Air is a high shelf at 10 kHz that lifts by `airDb`, hiss included.
- Off, the tone stage crosses to the untouched voice over a block and then does no work: the slot
  is bit for bit what it is with no stage. On with no bands and every amount at 0 it is the same.
  A change of bands, of an amount or of on and off ramps and never steps.
- `stats()` gives each slot `tone`: where the match's filters stand and how much singing it has
  counted, the most the de-esser took off in the last second, and how many pops the guard has
  ducked. A session bundle (8.11) holds each `setSlotEffects` call with its bands and `stats()`
  once a second, so a replay runs the profile the song ran and a scorecard reads what the stage
  did.

Done when: the corrector through its shim pulls harmonic tones from 82 to 880 Hz onto the asked
pitch class within the tolerances the C++ tests use; the stretcher moves a tone by the asked
semitones; the follower changes target within one block of each edge of a known guide; the
pitch event cadence is 10 ms; each first voice in decision 0014 moves a known vowel's formants
within the tolerances its test names and leaves its pitch, an off voice is bit-identical to none,
and a click and a tone through a voice leave on the sample they entered; with cleanup on, a click
leaves the slot on the frame it entered, `measure-latency` reads the mic path against the budget,
the gate takes at least 15 dB off a room between lines without moving a soft note by 1 dB, and a
12 dB level swing comes out 5 dB wide; on the offline driver, the canceller takes a room's echo
out of a house mic by the tolerances in decision 0020, the singer in the master and singing, and
the guard catches a loop running away; the tone stage off, or on with no bands and every amount
at 0, is bit-identical to no stage; a click leaves on the sample it entered with bands at the
cap, the match moved once it joins, the plosive guard ducking and the de-esser cutting; tones
from 80 Hz to 16 kHz through bands and air at the cap come out at most 0.2 ms late, and 0.4 with
the guard held
at its deepest; each band is within 0.1 dB of the cookbook's curve; `check` refuses a cascade
over either cap by the frequency it breaks at; every `Pitch` and `Level` event is the same with
the stage on as off; at `plosiveDb` 12 the guard makes no pop louder and ducks the pops that
stand furthest under the voice (twelve pops of noise under 80 Hz lose 0.0 to 4.8 dB under
100 Hz, a median of 0.5), and a held E2 or D2, bright or with nothing over 600 Hz, moves by under
1 dB in any 10 ms, its onset included, and counts as no pop; at `deEssDb` 6 an s 10 dB hot loses
at least 4 dB over 5 kHz and a vowel moves by under 0.5 dB; once the match joins, given a voice
through a known tilt it comes back toward its
target inside its rate and range, holds through silence, noise and a backing track alone, moves
nothing in its first 5 s and starts over on `resetSlot`; all of it runs under the allocation
guard; and on the bench's paired recordings the rows decision 0029 names show each test phone
nearer the reference mic with the stage on, by an amount the PR states, and no other row worse.

### 8.7 Return branch: `engine/src/return_ring.rs`, the push in `engine/src/master.rs`, `transport/src/{packetizer,send}.rs`

The master pushes every rendered block into a stereo f32 ring. The network thread pops, gathers
480 frames, encodes one Opus frame with in-band FEC, wraps it in RTP with the configured SSRC
and payload type, and sends it. Pacing is the output clock, because blocks arrive at the
device's rate. If the ring is full, the audio thread drops the block it would have pushed and
counts it. The packetizer, from ring to RTP bytes, is separate from the socket that sends them.

The ring holds frames, not blocks, because 480 is not a multiple of 128: the master writes a
whole block or drops it, and the packetizer takes 480 frames once they are there. The graph
owns its master, so this workstream also owns `Graph::set_return` in `graph.rs`, the one way a
return ring reaches a master before the graph goes live.

Done when: on the offline driver, the packetizer fed from the ring produces one packet per 480
rendered frames, with monotonic sequence numbers and timestamps advancing 480 per packet. The
socket send is the one step the test leaves out.

### 8.8 Binding and API: `node/`, `src/`, `tests/`, `engine/src/reactions.rs`

The `node` crate exposes the engine to Node through napi-rs: open, create, the parameter
setters, the commands as promises, the events thread. `src/` turns that into the API in
section 7.2 with typed events and validation of ranges before anything crosses.

- The binding converts units once, at the edge: milliseconds to frames, decibels stay decibels.
- Promises resolve on acks. A command whose ack has not arrived within a second rejects with
  the engine's fault state.
- `rtpReturn` gets its ring and send task in `createMixer` and loses them in `destroy`, so the
  stream runs while the graph is live. `stats().rtpReturn.droppedBlocks` is there because the
  packetizer hides a drop as a timestamp jump: without the count, a stalled network thread
  looks to the host like a quiet room.

Done when: every method and event in 7.2 exists, `vp check` passes with no `any`, and the
TypeScript suite exercises them on the offline engine.

### 8.9 Tools: `native/tools/`

Tools live in the Rust workspace, because what they measure is the engine. `measure-sink` is in
`devices`. `measure-latency` gives the mic-path number in two parts: the engine's share of each
ingest kind on the offline driver, which opens nothing and which a test holds, and the output's
share on real hardware, which a person runs with `device --open-device`. Decision
[0006](decisions/0006-measuring-latency.md) has how it works and what its figures cannot say. Their
output is the evidence a PR quotes.

A new ingest kind is a variant of `Ingest`, and a new stage on the mic path that holds samples,
such as correction, is a new row in the offline table with its own guard. A stage that holds none,
such as the voice, the tone stage, the echo canceller or the howl guard, proves it instead with a
test that a click leaves on the sample it entered while the stage is engaged: shaping a voice,
cancelling an echo it has learned, or ducking. The tone stage also shifts phase where it changes
level, so its test measures tones against its cap as well (8.6). The offline table's chirp would not engage them, so a row would show
nothing the test does not.

`native/tools/measure-corrector` opens no device. It runs the corrector through the real slot
chain on the offline driver and prints each row beside the GStreamer baseline, and it renders the
blind-listen set. Decision [0005](decisions/0005-corrector-measurement.md) has the reasons.

`native/tools/measure-echo` replays Aeon Karaoke's `aec:measure` files through the canceller and
scores them by its own formulas, times both echo stages per slot, writes and reads the bleed run's
programme, and, built with `hardware` and run with `--open-device`, is the live howl check.

`native/tools/replay` opens no device or socket. It reads a slot's recording, or any classic pcap
of one RTP stream, parses each datagram as the receiver does, and feeds it through the real
packet ring, jitter buffer and slot on the offline driver, each packet in the block after its
recorded arrival. The same file replays to the same samples every time. It prints the slot's
level beside libopus's own decode of the same packets, placed on their RTP timestamps, and the
slot's counters as `stats()` gives them.

Given a session bundle's directory in place of a file, it replays the whole song (8.11).

`native/tools/scorecard` opens no device or socket. It replays each session bundle it is given,
or each in a directory of them, as `replay` does, under `--profile` and with `--backing` as there,
and turns each song into numbers. Every number is read from what the replay's engine reported, as
`stats()`, `level` and `pitch` report it live, so it measures nothing a host could not see, except
the two that read a reading's shift, pull and note start, which the binding does not carry yet
(8.6). It
reads each slot's `stats()` after every block. Per recorded slot: `late`, `concealed` and
`playoutDelayMs` as the song ended; `correctorDelayMs`, the mean of `correctorDelayMs` over the
blocks it was above 0; `pitchErrorCents`, the median of `|errorSemitones|` in cents over the
`pitch` readings with a `targetMidi`, a `confidence` of at least 0.9 and an `hz` of at least 82,
the corrector's own bar, so a reading between notes, with no guide or with correction off counts
for nothing; `correctedErrorCents`, the same median of `|errorSemitones - shift_semitones|`, where
the corrected voice sat; `letGoes`, each gap in `pulling` with a pulled reading either side, within
a run of readings on one guide note, told apart by its `start_frame`, so a voice that stops as its
note ends lets go of nothing, even when the next note is on the same pitch;
`echoRemovedDb` and `bleedDb`, the medians of `echo.removedDb` and `echo.bleedDb`
over the blocks where `echo` was not null; `howlCatches`, every rise in it over the song, so a
reset that starts it from 0 again loses none; and `levelTopTenthDb` and `levelMedianDb`, the
90th percentile and the median of every `rmsDb` the slot's `level` reported in the song, as
`replay`'s table reads them, so a slot that sang half the song has a median that says so. The
master has the two levels, from
`masterLevel`. A number with nothing to read, such as pitch error with correction off, is left out.

- **A scorecard file** holds `format`, 1, and each bundle by its directory's name, each number
  with its `value` to a hundredth and a `tolerance`. The tool writes each tolerance as the
  baseline had it, or 0, and a person sets them: which movement fails a run is the file's to
  say, never the tool's.
- **Against a baseline**, an earlier scorecard, it prints every number that differs and fails
  when one moved further than its tolerance or is gone. A number the baseline lacks is new and
  fails nothing.
- **The fixtures.** Its tests score bundles written on the offline driver, with no real voice in
  them, against a committed baseline, so a change that moves one of their numbers past its
  tolerance fails `cargo test`: a short duet for the mic path and the corrector, and a house mic
  in a room for the echo canceller. The canceller's is a house mic's because a phone's Opus
  fills noise in where the backing's waveform was, which leaves the canceller little to take
  out and its numbers moving with the delay.
- **Not yet.** A fixture that holds `howlCatches` still: over a singer the guard catches the
  synthetic voice at some delays and not others (#245). Of the tone stage's rows (decision
  0029), the five that compare a phone with a reference mic need that recording stored beside a
  bundle, and `tone_late` and `tone_readings` come with the stage.

`native/tools/bench` is the mixer alone on a real clock, with no host application around it. A
developer at a desk asks it "did my change make it worse" in one command, and the runs in #116
that need only an output and a cable become a script. Like the tools above, it opens a device only
when built with `hardware` and run with `--open-device`. Without that flag it runs on the timer
clock (decision 0011), which opens nothing. Its tests run on the timer clock's synthetic clock,
with fake outputs and inputs of their own. Device ids are the ones `measure-latency list` prints.

```
bench play BUNDLE [--backing STEMS] [--profile FILE] [--out DIR] [--baseline FILE] [--write FILE]
                  [--open-device [--output ID] [--period 64|128]]
bench loopback --open-device --rig cable|mic [--air-cm CM] [--output ID] [--input ID]
               [--input-channel N] [--period 64|128] [--level-db DB] [--baseline FILE] [--write FILE]
```

- **`play` is `replay` on a real clock.** It builds the mixer a bundle's manifest describes, under
  the bundle's profile or `--profile`, with `--backing` as `replay` takes it. It reads the bundle
  and resolves each call with `replay`'s own code, so the two cannot drift. Before the first block
  it prepares the whole song: each call as the command it sends, each packet in its slot's order.
  The render then hands each one to the engine in the block `replay` would, counted from the
  manifest's frame. So on any clock the bench renders what `replay` renders of the same bundle,
  sample for sample, and only the clock differs: the output's own, its period, and whatever else
  the machine is doing. The scorecard's duet is the first bundle it plays.
- **It follows no output.** An output that stops or changes ends the run and is named,
  since a move would make the song differ from `replay`'s (8.11).
- **What it measures** is what only a real clock can say: whether the mix kept up.
  `output.underruns` counts the periods the output was not handed in time. `output.renderP99Ms`
  and `output.renderMaxMs` time a block's render against the output's period, which is all the
  time a render has. The song's
  own numbers are the scorecard's: scoring the bundle the bench writes gives the scorecard's
  numbers for the bundle it played, so the bench does not repeat them.
- **It writes a bundle** to `--out`, as a mixer writes one (8.11), with the real output in the
  manifest and `stats.jsonl` from the run. It keeps time by the output's frames: every `us` it
  writes, a packet's or a line's, is the manifest's `us` plus the frames since, at 48 frames a
  millisecond, and no packet is stamped later than the block it was handed to. Its pairs never
  drift, so the bundle replays to the samples the bench rendered. The bench is a second writer of
  format 1, held to it by `replay` reading what it writes in the bench's tests.
- **`loopback` measures a house mic's mouth to speaker without a mouth.** It plays decision
  0006's chirp into the output's left channel from the render, at frames it knows. A `capture`
  slot (decision 0018) reads it back from the input the rig reaches: a cable from the output's left
  channel, or a mic `--air-cm` from the speaker, as `measure-latency`'s rigs are. The slot is out
  of the mix and its output is read before the master, so the loop never feeds itself. Both ends
  are frames of the output's clock: the chirp's start as rendered and its arrival in the slot's
  output. The gap between them is the output, the cable or the air, the input and what the
  capture slot holds, which is the path a house mic's voice takes to the room. So the figure needs
  no clock line and splits nothing (decision 0006 says why a loopback cannot be split).
- **One detector.** It finds each chirp with `measure-latency`'s matched filter and onset rule,
  taken from that crate as a library, and refuses where that tool does: an input that is silent
  or stands under 30 dB over its noise, or chirps that disagree by more than 0.75 ms. Its numbers:
  `loopback.roundTripMs`, the median chirp's gap less the air; `loopback.spreadMs`, how far the
  chirps disagree; `loopback.gainDb`, the chirp's level as read against its level as played; and
  `loopback.snrDb`, the first arrival over the input's noise.
- **Its result is a scorecard file.** `play` names its numbers by the bundle's directory, as the
  scorecard does, and `loopback` by `loopback`. `--write` and `--baseline` follow the scorecard's
  rules, so two bench runs compare as two scorecards do. A bench figure belongs to its machine,
  and a baseline taken on another machine says nothing about this one.
- **What it prints**, for someone who has never seen it: what it played (the bundle, its slots,
  its length, and whether the stems were found); on what (the output's name, rate, period and the
  latency it reports, or the timer clock and that nothing was heard); each slot's `stats()` once a
  second as it plays; and then what it measured, with a verdict in words. A run kept up, or fell
  behind by so many periods. A round trip is set against decision 0008's 20 to 30 ms. Against a
  baseline, each number that moved is printed with whether it moved past its tolerance. A refusal
  prints why in place of a number.

Done when: on the timer clock's synthetic clock and a fake output, `bench play` of the scorecard's
duet renders what `replay` renders, sample for sample, under the allocation guard, and the bundle
it writes replays to those samples; a fake loop of a known delay reads back as that delay plus
what the capture slot reports holding, within a frame; and each #116 run it can now do is written
there as its command.

### 8.10 Adapter: Aeon Karaoke's `lib/device/infrastructure/src/mixer/impl/aeon/`

The class behind Aeon Karaoke's mixing-engine port that drives this package, beside the
GStreamer one, chosen by a flag.

Done when: the engine runs a session behind the flag; the latency e2e passes at the new number;
GStreamer and the `aeonpitch` element are deleted in a later PR.

### 8.11 Session bundle: `src/bundle/`, `tools/replay`

A recording of one slot says what reached it and nothing about the song around it. A session
bundle is one song whole: a directory `replay` can play back with nothing else, and that two
songs can be compared by. Decision [0026](decisions/0026-the-session-bundle.md) has the reasons.

It is the Binding and API workstream's, with 8.8 and 8.9 (section 10). In files it does not own it
adds these and changes nothing else: in 8.1's `engine/src/audio_thread.rs`, the frame count,
an atomic the block adds to; in 8.4's `transport/src/record/`, the sender's stand-in, a key the
recording draws and the hash of each sender under it, and the `WallClock` a recording is given
to stamp arrivals on, so a bundle's pcaps and lines share one. 8.4 in turn passes each datagram's source to
`packet_from` at its call sites in `tools/replay`, and `replay` says when a capture has more
than one sender, since the slot plays one at a time and the libopus reference decodes them all
(decision 0028). In 8.8's `src/mixer.ts`, 8.4 keeps `resetSlot`'s comment saying what the
released sender meets, as 7.2 does.

```
<dir>/
  manifest.json    written once, as the mixer is created
  profile.json     the engine's profile as the mixer is created (8.12)
  slots/<n>.pcap   one per rtp or l16 slot, the file recordSlot writes (8.4)
  events.jsonl     every call that changes what is heard, and what the engine reported
  stats.jsonl      stats() once a second
  host/            the host's own files; the engine makes it and never looks inside
```

- **The clock.** The audio thread adds each block to a count of output frames, an atomic it
  stores once a block and nothing waits on. Every line of `events.jsonl` and `stats.jsonl`
  carries `frame`, that count as the control thread read it, and `us`, the wall clock the pcaps
  stamp arrivals with, read in the same call: the mixer holds one `WallClock` and gives it to
  each of the bundle's recordings. Every line's pair ties the two clocks together, and the
  stats line each second shows them drift. A setting changed at `frame` is taken up by the
  block that starts at or after it, live and in a replay.
- **`manifest.json`** holds `format`, 1; the package's version and the platform; the output's
  rate, period, latency and name; `frame` and `us` at creation; each slot's index and kind and
  whether it is recorded; and the backing: each stem's kind and SHA-256, with key, speed, stem
  gains and `stopAtMs`. A stem is named
  by its hash and never copied, since a song is large and may not be the host's to copy. No path
  on the host's disk and no device id is written. A stem given as `bytes` is hashed from them, in
  slices that yield the control thread between them as a path is read as a stream, so it has the
  hash its file would. `replay --backing` finds it by that hash as any file of the same bytes, and
  a bytes stem changes nothing in `replay`: a host that holds a song only in memory has no file to
  give a replay, which then plays the mic path alone.
- **`events.jsonl`** holds, each with its arguments: `setSlot`, `setSlotEffects`, `setSlotGuide`
  with the whole guide, `resetSlot`, `start`, `seek`, `pause`, `resume`, `setBackingKey`,
  `setStemGain`, and the engine's `outputChanged`, `error`, `backingEnded` and `ack`. The mixer
  adds `backingRendered` as it moves the rendered stems into the graph, with their length in
  frames at the armed speed, which the binding's `installBacking` returns: the length is known
  only once the render is done, after the manifest is written, and a replay that renders the
  stems again checks its own against it. A call's line is stamped before its change is made and
  written in the same turn of the control thread, once the change is made or its command sent,
  so the file holds the calls in the order they were made and no line's `frame` is less than the
  one before it. A command's ack writes a line of its own as it arrives: `ack`, with `call`
  naming the call that sent it and `positionMs` the backing position it gave. `destroy` writes
  `{"type":"end"}` last, so a bundle without one was cut short. `level` and `pitch` are left out:
  a replay makes them again.
- **Order against `start`.** `start` makes its graph live as it sends the swap, before its acks
  arrive. A call made just after it reaches the live graph; one made just before it is held for
  `start` to write, or, for `resetSlot`, dropped. Both can carry `start`'s `frame`, so only the
  file's order tells them apart. While `start` waits on its acks, the parameter calls and
  `setSlotGuide` end the same on either side of it, since `start` writes what was held.
  `resetSlot` does not: after `start` it reaches a slot that has pulled nothing yet, which changes
  nothing on an `rtp`, `l16` or `capture` slot and drops what a `push` slot was handed while
  armed. The backing calls are refused before `start` and write their own lines as sent, and
  `destroy` closes the file, which would lose a line still waiting on an ack. Writing each line in
  its call's turn orders all of them.
- **`stats.jsonl`** holds `stats()` and the backing's position, from a timer on the control
  thread that keeps no process alive.
- **A sender is a port, never an address.** A bundle's pcaps write each sender as 0.0.0.0 at a
  port of its own, a hash of its address and port under a key the recording draws at random and
  never writes, where `recordSlot` writes the phone's address. A slot keeps the sender it is
  receiving from (decision 0028), so a replay must tell senders apart to refuse the ones the live
  slot refused; it never needs to know who they were. Two senders to one slot share a port one
  time in 65,535.
- **`recordSlot` throws on a mixer with a bundle.** Its slots already record there, and a slot has
  one recording, so a second would end the bundle's.
- **A `push` or `capture` slot is listed with `recorded: false`** and replays as silence. Its PCM
  is format 2's.
- **Nothing here touches the audio thread but that one store.** The pcaps cost the network thread
  what decision 0017 measured, once per slot. Events and stats are written on the control thread,
  each line whole, so a process that dies leaves a bundle that reads to its last line.
- **Privacy.** A bundle holds every singer's voice in the song. It is made only when the host
  asks for one, nothing in this package sends it anywhere (never-compromise 3), and the host
  owns it and answers for each singer's consent (8.4).
- **Replay.** `replay DIR [--profile FILE] [--backing STEMS] [--wav OUT] [--json]` builds the mixer
  the manifest describes on the offline driver, under the bundle's profile or the one `--profile`
  names (8.12), with the manifest's `frame` as its first block. Its graph is swapped in at the
  `start` line, with the swap's fade, as it was live: until then a call only builds what `start`
  writes, a `resetSlot` does nothing, a packet waits in its slot's ring, and the mix is silence
  where live it was whatever played before. Every line's pair ties the clocks, the manifest's too,
  so it converts each arrival to a frame through the pairs either side of it, or at 48 frames a
  millisecond from the nearest pair outside them, and pushes it before the first block that starts
  at or after that frame, the block a setting at that frame would reach. The pcaps start before the
  manifest's pair is read, so an arrival can come before it and goes before the first block. It
  applies each line at its frame, in the file's order, filling what a call left out from the
  profile as `src/mixer.ts` does: a change to how a call is resolved there changes `replay` in the
  same PR. It plays to `end`. `--backing` names a directory it searches for stems by hash; without
  it the mic path replays alone. `--wav` writes the mix, and `--json` prints the replay's own
  `stats()` and levels. A format it does not know, or a bundle with no `end`, it names; the first
  it refuses, the second it plays to its last line or packet.
- **An output move replays as none.** The offline driver has one clock, so a slot's stream does
  not end at an `outputChanged` line as it did live, and the replay can differ from the song
  after one.
- The same bundle replays to the same samples every time. Against the live song it can differ by
  a block: the pairs place an arrival to within the callback's own jitter, so a packet read at a
  block's edge can land either side.

Done when: a bundle written during a song on the offline driver replays to the same samples
twice, under the allocation guard, with an effect change and a seek mid-song landing on the same
blocks; a bundle with no `end` line replays to its last line or packet; a `format` the tool does
not know is refused by name; and `tap_cost` with eight slots recording is quoted in the PR.

### 8.12 Settings profile: `src/profile/`

Settings had no one home: a host named every effect value on every call, pinned the playout delay
in an option and kept its own defaults for the rest. A profile is the one object that holds what
the engine does when nobody chose, so a session says which settings it ran under and another can
be tried against the same singing. Decision [0027](decisions/0027-the-settings-profile.md) has the
reasons.

- **A choice is not a default.** A value a call names is someone's choice: the singer turning
  correction to full, the host muting a slot. The profile holds the rest. `mute`, `solo`, an
  effect's `enabled` and a melody guide are always choices and have no place in it.
- **`defaultProfile`** is a period of 128 frames; a playout delay of `"auto"`; a slot at gain 1,
  pan 0; correction at strength 0.6 with a 40 ms retune; reverb at room size 0.5, mix 0.25; a
  voice at 0 semitones; the instrumental at gain 1 and the guide vocals at 0. The effect values
  are the ones Aeon Karaoke has been sung with, so a host that moves to the profile hears no
  change.
- **Where it applies.** `openEngine` takes it for the engine's life. `periodFrames` and
  `playoutDelayMs`, given, win over it, and `Engine.profile` reads back the result. A slot starts
  at the profile's gain and pan with every effect off. `setSlotEffects` fills each parameter a
  call leaves out from the profile, and `createMixer` each stem gain. The tone stage's amounts
  have no value in it yet: left out, each is 0 (8.6).
- **Nothing a host does today changes.** Every field that becomes optional was required, and a
  call that names it means what it meant.
- **`parseProfile`** checks each value against the range its setter does, and refuses a missing
  field, an unknown field and an unknown `version` by name. An unknown field is refused, not
  ignored, so a misspelt setting never passes as one that did nothing.
- **The engine's own constants are not in it.** The gate, the compressor and the delay chooser
  hold some thirty. A constant joins the profile under a `tuning` key, documented as free to
  change in any minor version, in the PR that first needs to sweep it.
- **In a bundle and a replay.** A bundle's `profile.json` is `Engine.profile` as the mixer was
  created, and `events.jsonl` holds each call as the host made it, with what it left out left
  out. So `replay --profile` changes exactly what nobody chose, and keeps every choice.
- It is the Binding and API workstream's (section 10). The profile is resolved on the control
  thread into the `Params` and options the engine already takes, so nothing below the binding
  changes.

Done when: `openEngine` with no profile behaves as before for a host that names every value;
`setSlotEffects` with a parameter left out takes the profile's, proven by output on the offline
driver; `parseProfile` refuses a missing field, an unknown field, a value out of range and an
unknown version, each by name; and one bundle replayed under a second profile differs only in the
slots whose calls left that value out.

## 9. Verification

- The offline driver is the clock in every engine test. No test opens a device. A test that
  creates an `rtp` or `l16` slot binds that slot's ephemeral port, because the port is what it
  checks, and may bind it again to see it held; no test sends a datagram, since nothing marks
  one's arrival to await, and the packet path is proven in Rust on the offline driver.
- Every realtime `process` and `render` runs under the `assert_no_alloc` guard in tests. Its
  allocator panics on an allocation inside a guarded region, and the crate carries the `unsafe`
  so the engine does not.
- DSP tests feed known signals and assert within tolerances named in the test.
- Timing claims carry a number from `measure-latency` or `measure-sink`, and the number goes in the PR
  and, when it decides something, in `docs/decisions`.
- CI builds and tests the workspace on macOS, Windows and Linux. Device backends compile behind
  a feature so a runner without hardware still builds them.

## 10. Workstreams and ownership

Work is split so that agents run in parallel without touching the same files. A workstream
owns the paths named in the module sections in its row. Shared files are `native/Cargo.toml`,
`native/engine/src/lib.rs` and `src/index.ts`: add your own line, leave the rest. The rows were
the first build's workstreams, each an issue now closed. The table stays as the map of which
module sections a piece of work owns: a new issue names the rows whose paths it touches.

A measuring tool may add a read-only accessor to a kernel or engine type another workstream owns:
one function that reports state, with no change to behaviour. That is how a tool reads state
without owning the file, as `Slot::corrector_correcting` does for `measure-corrector`.

| #   | Workstream          | Owns                 |
| --- | ------------------- | -------------------- |
| 0   | Skeleton            | 8.0                  |
| 1   | Graph               | 8.1, 8.2             |
| 2   | Devices             | 8.3, decision 0003   |
| 3   | Ingest              | 8.4                  |
| 4   | Backing             | 8.5                  |
| 5   | Kernels and effects | 8.6                  |
| 6   | Return branch       | 8.7                  |
| 7   | Binding and API     | 8.8, 8.9, 8.11, 8.12 |
| 8   | Adapter             | 8.10                 |

## 11. Divergences from the contract document

Each is deliberate and here so no one reverts it by accident.

- **The engine outlives the song.** The contract has one mixer per song created when ready and
  destroyed at the end. The service still sees that. Underneath, the device stays open and
  graphs swap, which removes the reopen, the pop and the reason reactions had to live outside.
- **Reactions play through the engine.** The contract keeps them on a separate bus because the
  mixer is gone between songs. The engine is not, so reaction voices are inputs on it, in the
  echo canceller's reference by construction.
- **A soft clipper on the master.** Not in the contract. Four singers over a backing track
  exceed full scale, and hard clipping is the ugliest sound the box can make. Zero lookahead,
  zero latency.
- **One live mixer.** The device runs one room, so the engine hosts one live graph and any
  number of mixers being armed.
- **No `relinkSlot`.** A slot's port is stable. A phone that rejoins sends a new SSRC, which
  re-anchors the jitter buffer and resets the corrector, at once from the same address and after
  the old stream's second of quiet from a new one. `resetSlot` frees the slot at once.
- **Decode in the callback.** Decoding on the network thread would need one more buffer between
  the two threads. Decoding a due packet in the callback costs tens of microseconds and no
  latency.
- **A mixer error is narrower than a pipeline error.** The GStreamer engine ends the song on any
  error any element posts, even one bad phone stream. Here `error` means the mixer can no longer
  play its song (section 5), so a singer's failure stays that singer's.

## 12. Decided later

- Whether a phone at the mouth needs the echo canceller as well as the howl guard: the bleed run in
  decision 0020 decides. Then a stereo reference and a residual echo suppressor, if a real room
  shows the linear filter leaves too much.
- The offline stretcher: signalsmith at a quality setting first, an A/B against Rubber Band on
  three tracks before paying for a licence.
- A stem the host feeds as a stream, so a song can start before all of it has arrived. It could
  not pre-render a key or speed or seek ahead of what has arrived, so the backing would report
  what it lacks, and the host would offer only the rest. A whole buffer is enough until a catalog
  needs a song to start sooner.
- Stem storage on a LAN box: planar f32 is about 23 MB per stem minute; int16 storage or
  streaming decode if a box cannot hold two stems.
- Whether a capture profile may warm a phone's low end past the tone stage's cap: a 6 dB low shelf
  at 200 Hz leaves 95 Hz 0.53 ms late. That is a cost on the mic path, so it takes a listen on the
  bench's pairs that says the warmth is worth it. The same pairs decide whether the match is good
  enough to be a host's default, and whether a capture with no top octave needs an exciter.
- Choosing a slot's delay before `start`, from the packets a phone already sends to an armed mixer,
  so the first ten seconds of a song have the chosen delay too. Nothing here measures how long a
  mixer sits armed yet.
- Higher and lower voices: an octave up with formants +3 semitones, and an octave down with -3,
  the settings and research in decision 0014. They come through the corrector's shifter, which
  reads its delay line at the product of the correction and the octave, so a voice with
  correction costs the corrector's one delay and `correctorDelayMs` reports it. Through the voice
  stage's own shifter they would cost a second delay, up to 31.5 ms at 82 Hz with correction on.
  The pitch knob joins the contract with that work, and where it lives (with the corrector's
  knobs, or only with a guide) is that work's to decide. It starts once the corrector's shifter
  has settled, since it changes the same kernel, and is about four pull requests: the contract
  with decision 0014, the kernel, the slot and binding, and a listen set. It is done when tones
  an octave up and down land within a stated number of cents, `correctorMaxDelayMs` reads the
  same with a voice and correction as with correction alone, a click leaves where it entered, and
  a listen keeps them. The faults to listen for are known: an octave down on a 440 Hz voice keeps
  more of the singer's own formants than the asked ones, and shifting down can drop a short
  sound. With correction off, an octave voice pays the shifter's delay where today the slot pays
  none: a cost on the mic path the listen has to justify. Deeper and brighter shipped without
  them, and the beta's list does not name them.
- Harmony: up to two voices per slot taken from the corrected voice, each a `VoiceShifter`
  (decision 0004) at the diatonic interval the guide's note and `keyMask` give, panned off centre.
  They run beside the dry voice, so the dry voice keeps its path and its delay, and each trails it
  by decision 0004's bound, up to 15.0 ms at 82 Hz a tritone up and 16.5 ms an octave up; whether
  that reads as fuller or as late is a listen's to say. No guide, no harmony, and the stage drops
  a voice on unvoiced sound itself, since the kernel keeps shifting through consonants. On test
  tones the kernel lands within 5 cents across an octave either way; its formants miss by under
  2 dB RMS at 6 semitones down and 3 to 5 dB at 12 down on low voices. One voice is about 1.0 %
  of a core on an M2 Pro, two on four slots about 8 %, of which voices on one slot could share
  their pitch reading, about 0.2 of each 1 %, if the kernel could take one in. It was parked on
  15 November 2026, and is best started after the corrector's listen, since it shifts the
  corrected voice and would carry any warble into two more. Done when each voice lands within
  10 cents of its interval on a test tone, the dry voice's click row does not move, four slots run
  under the allocation guard with their CPU quoted, and a blind listen rates it at or above no
  harmony.
- `measureSlot(index)`: one call on a mixer that plays a test sound in the mix, finds it in one
  slot's input, and resolves with the delay to the sample on the engine's clock and a confidence,
  or with not heard and what was heard (the level, and how much of the sound), so a quiet phone
  can be told from the wrong microphone. The sound goes out and back the way a voice does, so the
  loop is the delay a singer hears and no clock is synced between the phone and the engine. The
  detector is `measure-latency`'s, its chirp and matched filter moved where the engine can use
  them, not a new one. Where the echo canceller is on, its delay finder and this share one
  estimate, and the engine could follow the delay through a song from the backing in the mic. The
  engine measures and reports; when to measure is the host's, which today plays a chime through
  a short mixer and reads it back from `pitch` events, so a missed chime says nothing. It is a
  new public method and about four pull requests: the contract in 7.2 and 8.8, the detector
  shared out of `measure-latency`, the test sound and detection in the engine, and the binding
  with its tests. Done when, on the offline driver, a slot fed the mix back at a known delay reads
  it within a block, and one fed silence or the sound 40 dB down resolves as not heard and says
  what it heard. Two things only hardware answers: whether a phone's noise suppression mangles
  the sweep, and how the loop splits between the output and the phone, which a `capture` slot
  with no network in its path can measure. It is not on the beta's list.
