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
- A panic inside render is caught at the callback boundary. Every block from there is silence,
  the audio thread pushes `Fault` again each block until the event ring has room for it, and the
  engine refuses further commands until the host closes it.

## 3. The two clocks

Mic slots run on the output clock. A block of output is a block of input from every slot's
jitter buffer, decoded now. Nothing on a slot's path may delay a sample beyond the jitter
buffer's fixed playout delay and the decoder's frame.

The backing path runs on its own playhead, a frame counter the audio thread advances by one
block per render while playing. Key shift, time stretch, guide vocal mixing, seek and fades
live there and may cost latency, because the engine reports the playhead and the device's
lyric clock follows it.

What the engine reports so the rest of the device can follow:

- `start()` resolves with `startedAt`, the wall-clock moment the listener hears backing frame
  zero; `backingOffsetMs`, the engine's own output latency, from a backing frame rendered to
  that frame leaving the speaker; and `speedFactor`.
- Every ack carries the backing position at the block the command landed. `seek`, `resume` and
  `start` return it. A `position` event also fires once a second.
- When a live key shifter is engaged, its latency is subtracted from the reported position, so
  the lyric clock stays on the audio the room hears.
- A pitch reading carries the position on the guide's clock at the reading's centre. The slot's
  guide clock is the playhead less `backingOffsetMs` less the slot's mic-path latency, which the
  engine knows, plus a host-set `guideOffsetMs` for what it cannot know: the phone's capture and
  the Wi-Fi.
- An engaged corrector delays its slot's voice by an amount that moves with the voice, and holds
  it through a rest. `stats()` gives each slot `correctorDelayMs`, the delay as it stands, and
  `correctorMaxDelayMs`, its 15 ms ceiling; both read 0 with correction off. A host shows
  the first and budgets with the second. Neither enters a clock: nothing can play a live voice
  early, and the guide clock and a pitch reading both name the corrector's input, ahead of its
  delay. Decision [0009](decisions/0009-reporting-the-corrector-delay.md) has the reasons.

## 4. Signal path

The topology is fixed. Adding a feature means adding a stage to a chain, a stem to the backing
path or an output to the master, never a general graph.

**A slot**, in order: source, echo cancel when compiled in, pitch corrector, meter, reverb,
gain and pan into the stereo bus. The meter sits after the corrector and before the reverb and
gain, so the level words tell the singer about their singing, not the room, and mute never
changes them. The corrector runs bypassed for now: engaged, its shifter delays the voice by up
to 15 ms for a low voice, which breaks the latency budget on slower devices. It reads pitch
with no delay until `measure-latency` shows engaged correction fits the budget. The source is a
jitter buffer and decoder for `rtp` and `l16` slots, a PCM ring for `push` slots. Slots are
mono and pan with a constant-power law into the stereo bus.

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
| Playout delay        | 15 ms, set at open                           | fixed and shallow; the singer notices a moving delay more than a click |
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

- **Open.** `openEngine` picks the device, starts the driver with the silent graph, and returns
  once the first block has rendered. The stream stays open until `close`.
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
- **Close.** `close` destroys any live mixer, stops the driver, joins the threads.
- **Faults.** A device loss or a caught panic emits `fault` on the engine. The host closes and
  reopens; nothing inside tries to recover.
- **Mixer errors.** `error` on a mixer fires once, when that mixer can no longer play its song,
  and the host ends the song and destroys it. Three things raise it: a render job that fails,
  at the moment it fails, so a bad next song is known before its turn (`start` still rejects
  with the same message); an ack that times out, because the audio thread drains commands every
  block and a second without one means the room has gone silent; and an engine fault, on every
  mixer the engine holds, so a host can treat one signal as the end of a song. A timeout is not
  itself a fault: a stall can pass, so the song ends and the engine stays open until a device
  loss or a panic says it is gone. Trouble a mixer plays through is not an error: a slot's bad,
  late or missing packets are concealed and counted in `stats()`, so one phone never ends a song
  for the room, and a return stream that cannot send costs only its remote listener, since the
  room still hears the mix.

## 6. Module map

```
src/                    TypeScript public API, the only thing Aeon Karaoke imports
tests/                  TypeScript tests through the binding, on the offline driver
tools/                  measurement scripts a person runs: measure-sink, measure-latency
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
  kernels/              C++ leaves behind extern "C": corrector, stretcher. Rust leaves: reverb, voice shifter
    build.rs            cc compiles cpp/
    cpp/                vendored sources with licences and the commit they came from
  transport/            the network thread: UDP receive into packet rings, RTP return sender
  devices/              Driver implementations: coreaudio, wasapi, alsa; the measure-sink binary
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
/// The test clock. Its caller pumps it, so it is not a `Driver`. `pump_into(&mut [f32])` is
/// allocation-free and is what tests guard; `pump(blocks) -> Vec<f32>` is the binding's convenience.
pub struct OfflineDriver;

// engine/src/params.rs
#[derive(Clone, Copy)]
pub struct Params { pub slots: [SlotParams; MAX_SLOTS], pub stems: StemGains }
#[derive(Clone, Copy)]
pub struct SlotParams { pub gain: f32, pub pan: f32, pub mute: bool, pub solo: bool, pub effects: EffectParams, pub guide_offset_ms: f32 }
#[derive(Clone, Copy)]
pub struct EffectParams { pub corrector: CorrectorParams, pub reverb: ReverbParams }

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
    Pitch { slot: u8, position_ms: f64, hz: f32, confidence: f32 },
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
pub struct CorrectorDelay;  // published once a block; Slot::corrector_delay() hands the control thread its Arc

// engine/src/ingest/
pub struct Packet { pub seq: u16, pub timestamp: u32, pub ssrc: u32, pub marker: bool, pub len: u16, pub payload: [u8; 1500] }
pub fn packet_ring(capacity: usize) -> (PacketProducer, PacketConsumer);   // SPSC, fixed-size
pub enum Codec { Opus(OpusDecoder), L16 }   // Codec::opus() creates the decoder, on the control thread
pub struct JitterBuffer;   // new(playout_delay_frames, codec); push(&Packet); pull one block of f32 mono; stats(); shared_stats() for the control thread
pub struct PacketSource;   // the Source of an rtp or l16 slot: new(PacketConsumer, JitterBuffer)
pub struct PushVoice;      // the Source of a push slot: an SPSC f32 stereo ring its PushProducer fills; silence when empty
pub struct SlotStats { pub pushed: u32, pub lost: u32, pub late: u32, pub concealed: u32, pub resyncs: u32, pub jitter_ms: f32 }

// transport/src/
/// What the network thread runs: one socket under one token, serviced on readiness and ticked after every poll, at least every 5 ms.
pub trait Task: Send + 'static { fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()>; fn deregister(&mut self, registry: &Registry) -> io::Result<()>; fn ready(&mut self); fn tick(&mut self) {} }
pub struct Network;        // spawn(); add(impl Task) -> TaskHandle, whose drop removes the task; drop stops the thread
pub struct Receiver;       // a Task: bind(port: Option<u16>, PacketProducer); port()
pub struct Header { pub marker: bool, pub payload_type: u8, pub seq: u16, pub timestamp: u32, pub ssrc: u32 }   // rtp.rs: parse(&[u8]) -> Option<(Header, &[u8])>; write(&mut [u8; 12])

// engine/src/backing/
pub struct Stems { pub instrumental: Planar, pub guide: Option<Planar>, pub frames: u64 }   // 48 kHz, planar f32
pub struct Playhead;       // frames, playing, seek(to), pause(), resume(), stop_at(at, fade)
pub struct RenderJob;      // control thread starts it on the worker thread; yields Stems; progress is an AtomicU32 fraction
pub struct LiveKey;        // two pre-allocated stretchers; bypassed until the first change

// kernels/src/
pub struct PitchCorrector;  // new(rate), set_target(mask, strength, retune_ms), process(in, out), reset(), detected(), latency_samples()
pub struct Stretch;         // new(channels, rate, preset), set_transpose_semitones(f32), process(planar in, planar out), latency_frames()
pub struct Reverb;          // new(rate), set(room_size, mix), process(mono in place): a slot is mono until its pan
pub struct YinDetector;     // new(rate, window_samples), analyze(window) -> Pitch: the corrector's detector
pub struct VoiceShifter;    // new(rate), set(semitones, formant_factor), process(in, out), reset(), latency_samples(), latency_bound_samples(rate, semitones, hz): pitch and formants apart, on the mic path
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
export type IngestKind = "rtp" | "l16" | "push";
export type StemKind = "instrumental" | "guideVocals";

export interface OpenEngineOptions {
  output: { device?: string; periodFrames?: 64 | 128 } | { offline: true };
  slots?: number; // fixed for the engine's life, default 8, at most 16
  playoutDelayMs?: number; // the ingest jitter buffer, default 15
}

export interface Engine {
  readonly capabilities: ReadonlySet<Capability>;
  readonly output: { sampleRate: 48000; periodFrames: number; latencyMs: number };
  outputDevices(): Promise<ReadonlyArray<{ id: string; name: string; isDefault: boolean }>>;
  createMixer(config: MixerConfig): Promise<Mixer>;
  /** Room-lifetime reaction voices, four at once. False when a fifth is dropped while four play. */
  reactions: { play(pcm: Float32Array, channels: 1 | 2): boolean };
  on(event: "fault", listener: (error: Error) => void): () => void;
  /** The mix after the clipper, reactions included, between songs too. */
  on(event: "masterLevel", listener: (rmsDb: number, peakDb: number) => void): () => void;
  /** Offline engines only: render blocks and return them as interleaved stereo. */
  pump?(blocks: number): Float32Array;
  close(): Promise<void>;
}

export interface MixerConfig {
  slots: ReadonlyArray<{ kind: IngestKind }>;
  backing?: {
    stems: ReadonlyArray<{ kind: StemKind; path: string }>;
    key: number; // semitones, rendered before start
    speed: number; // factor, rendered before start
    stemGains: Record<StemKind, number>;
    stopAtMs?: number;
  };
  /** `host` is an IP literal, never a name; `payloadType` is dynamic, 96 to 127. */
  rtpReturn?: { host: string; port: number; payloadType: number; ssrc: number };
}

export interface SlotSettings {
  gain: number;
  pan: number;
  mute: boolean;
  solo: boolean;
}
export type SlotEffect =
  | { kind: "pitch-correction"; enabled: boolean; strength: number; retuneMs: number }
  | { kind: "reverb"; enabled: boolean; roomSize: number; mix: number };
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
}

export interface Mixer {
  readonly slots: ReadonlyArray<{ index: number; kind: IngestKind; port: number | null }>;
  setSlot(index: number, settings: Partial<SlotSettings>): void;
  setSlotEffects(index: number, effects: ReadonlyArray<SlotEffect>): void;
  setSlotGuide(index: number, guide: MelodyGuide | null, guideOffsetMs?: number): void;
  /** Forget the stream a slot was hearing; the next packets are a new singer. */
  resetSlot(index: number): void;
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
  on(
    event: "pitch",
    listener: (slot: number, positionMs: number, hz: number, confidence: number) => void,
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

## 8. Modules

Each module names its purpose, the rules beyond section 2 that bind it, and what done means.

### 8.0 Skeleton: `native/Cargo.toml`, `native/engine/src/{lib,block,driver}.rs`, `native/node`, `native/binding`, `package.json`, `vite.config.ts`, `.github/workflows`, `src/native.ts`

The workspace, the binding that loads, the offline driver, and CI. It is what every other
module plugs into. It also writes the first `src/engine.ts` and `tests/engine.test.ts`, which
8.8 inherits.

Done when: `vp check`, `vp test` and `cargo test` pass on macOS, Windows and Linux in CI; a
TypeScript test pumps one silent block through the offline engine; prebuilt addons publish per
platform as optional dependencies.

### 8.1 Graph: `engine/src/{graph,params,command,master,audio_thread}.rs`, `engine/src/slot/`

The audio thread's whole job. `render` reads the snapshot, pulls one block from every slot's
source, runs the chain, sums with the backing path and the reaction voices, clips, meters,
writes the device buffer and the return ring, and pushes events.

- Solo: when any slot is soloed, every unsoloed slot's effective gain is zero. Mute wins over
  solo. The effective gain ramps across the block like any other gain change.
- Meters publish RMS and peak over the last 100 ms at 10 Hz, from after the corrector.
- The reaction voices are the engine's, not a graph's. The audio thread renders them once a
  block, and every graph rendering in that block sums them ahead of its master through
  `Graph::render_with`, so a reaction plays straight through a swap fade and is clipped and
  metered with the mix.
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
- Device enumeration lists outputs and marks the default. Hotplug is decided later.

Done when: decision 0003 holds the measured period, latency and glitch count on a Mac, a
Windows laptop and a Linux box, and the backends pass `cargo test` with the device code behind
a feature so CI without hardware still compiles them.

### 8.4 Ingest: `engine/src/ingest/`, `transport/src/receive.rs`

The network thread owns one UDP socket per `rtp` and `l16` slot, so today's grant flow and the
mediasoup loopback keep working unchanged, polls them with a timeout, parses the RTP header and
pushes fixed-size packets into the slot's ring. The RTCP mediasoup multiplexes onto the same
port is dropped by its payload type, RFC 5761's reserved range. The audio thread drains the
ring into the jitter buffer at the top of each block.

- The jitter buffer anchors the first packet's RTP timestamp to the output clock plus the
  playout delay, and jitter never moves that anchor. A missing packet at its due time is
  concealed by the decoder, with in-band FEC from the next packet when that is already in
  hand. A packet that arrives after its time is counted late and dropped; it never silences or
  resets the stream.
- Skew moves the anchor one frame at a time. Once a second the buffer looks at how early the
  last second's packets arrived against their due times. When every one came at least a frame
  early, the anchor moves a frame earlier and that frame is dropped; when every one came a
  frame late, or half the delay late when a frame is longer than that, it moves a frame later
  and that frame is concealed. Either counts as a resync. Arrival slack is the measure rather
  than the decoded FIFO's fill, because decoding on demand keeps the FIFO at one frame.
- A new SSRC, a gap over a second on either clock, or `resetSlot` starts a new stream: the
  buffer re-anchors and the slot's corrector resets. A gap is a second without a packet, or a
  timestamp a second away from where the stream is; the second one is a sender that paused.
  A stream quiet for a second stops concealing, so an idle slot costs no decodes.
- Decoding happens in the callback, bounded to the packets due this block. The decoder is
  created on the control thread; libopus allocates nothing after that.
- A slot's first pull discards what its ring already holds. A mixer is armed, and its ports
  handed to the phones, before it starts; a stream anchored on that backlog would play behind
  real time for good.
- At the 15 ms playout delay, in-band FEC helps frames of 10 ms and shorter. The packet after
  a lost 20 ms frame arrives 5 ms after that frame falls due, so mediasoup's 20 ms streams get
  the decoder's own concealment.
- `l16` slots take stereo 16-bit big-endian at 48 kHz, as the house-mic sender sends today, and
  fold to mono. `push` slots take interleaved stereo f32 from the control thread and fold to
  mono too. The ring holds 100 ms, and `push` returns how many frames fit; the rest is the
  host's to hand in again. The slot plays at most 30 ms behind the newest frame it holds: a pull
  that finds more drops the oldest down to 30 ms and counts a resync, so a host on another
  clock, or one catching up after a stall, is pulled back inside the budget instead of drifting
  out of it. 30 ms is three 10 ms chunks, room for a host two chunks late, and is a default
  until `measure-latency` (8.9) gives the push path a number.

Done when: a test feeds synthetic RTP with reordering, 30 ms jitter spikes and 5 percent loss
through the offline driver and the output matches the source within the concealment's expected
error; frames of 2.5, 5, 10 and 20 ms all pass; a packet arriving late never produces a gap
longer than the frame it missed; a stream that pauses for two seconds resumes without a
growing delay.

### 8.5 Backing: `engine/src/backing/`

Stems in memory, one playhead, gains, fades, the live key, and the render job.

- The worker thread decodes stems with Symphonia, a pure Rust decoder, to planar f32 at 48 kHz,
  resampling any other rate with Rubato's FFT resampler. With an armed key or speed it renders
  them through one stretcher at signalsmith's default preset, both stems together so they stay
  aligned, trimming the stretcher's lead and flushing its tail. `createMixer` resolves when the
  graph is built; the backing goes in with `Graph::set_backing` as soon as the render finishes,
  so a render that fails raises `error` then, and `start` waits for a render still running.
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
- At strength zero the corrector is bypassed with no delay and still reads pitch.
- The slot drives two stages from `slot::effects`, each in place on its mono block.
  `Corrector::process(params, guide, frame, semitones, block, on_reading)` borrows the slot's
  guide, takes the guide-clock frame of the block's first sample and the backing's key shift, and
  calls `on_reading` per reading for the slot to push as `Event::Pitch`.
  `Reverb::process(params, block)` takes the reverb knobs.

Done when: the corrector through its shim pulls harmonic tones from 82 to 880 Hz onto the asked
pitch class within the tolerances the C++ tests use; the stretcher moves a tone by the asked
semitones; the follower changes target within one block of each edge of a known guide; the
pitch event cadence is 10 ms.

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

### 8.9 Tools: `tools/`

Scripts a person runs on real hardware. `measure-sink` is in `devices`. `measure-latency`
plays a click through a slot and records it at the output to give the mic-path number. Their
output is the evidence a PR quotes.

### 8.10 Adapter: Aeon Karaoke's `lib/device/infrastructure/src/mixer/impl/aeon/`

The class behind Aeon Karaoke's mixing-engine port that drives this package, beside the
GStreamer one, chosen by a flag.

Done when: the engine runs a session behind the flag; the latency e2e passes at the new number;
GStreamer and the `aeonpitch` element are deleted in a later PR.

## 9. Verification

- The offline driver is the clock in every engine test. No test opens a device. A test that
  creates an `rtp` or `l16` slot binds that slot's ephemeral port, because the port is what it
  checks, and may bind it again to see it held; no test sends a datagram, since nothing marks
  one's arrival to await, and the packet path is proven in Rust on the offline driver.
- Every realtime `process` and `render` runs under the `assert_no_alloc` guard in tests. Its
  allocator panics on an allocation inside a guarded region, and the crate carries the `unsafe`
  so the engine does not.
- DSP tests feed known signals and assert within tolerances named in the test.
- Timing claims carry a number from `tools/` or `measure-sink`, and the number goes in the PR
  and, when it decides something, in `docs/decisions`.
- CI builds and tests the workspace on macOS, Windows and Linux. Device backends compile behind
  a feature so a runner without hardware still builds them.

## 10. Workstreams and ownership

Work is split so that agents run in parallel without touching the same files. A workstream
owns the paths named in the module sections in its row. Shared files are `native/Cargo.toml`,
`native/engine/src/lib.rs` and `src/index.ts`: add your own line, leave the rest. Each row is a
GitHub issue, which carries what it is blocked by; each issue is one or more PRs.

| #   | Workstream          | Owns               |
| --- | ------------------- | ------------------ |
| 0   | Skeleton            | 8.0                |
| 1   | Graph               | 8.1, 8.2           |
| 2   | Devices             | 8.3, decision 0003 |
| 3   | Ingest              | 8.4                |
| 4   | Backing             | 8.5                |
| 5   | Kernels and effects | 8.6                |
| 6   | Return branch       | 8.7                |
| 7   | Binding and API     | 8.8, 8.9           |
| 8   | Adapter             | 8.10               |

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
- **No `relinkSlot`.** A slot's port is stable. A new sender is a new SSRC, which re-anchors
  the jitter buffer and resets the corrector. `resetSlot` exists for the host that wants to
  force it.
- **Decode in the callback.** Decoding on the network thread would need one more buffer between
  the two threads. Decoding a due packet in the callback costs tens of microseconds and no
  latency.
- **A mixer error is narrower than a pipeline error.** The GStreamer engine ends the song on any
  error any element posts, even one bad phone stream. Here `error` means the mixer can no longer
  play its song (section 5), so a singer's failure stays that singer's.

## 12. Decided later

- Echo cancellation: the stage exists in the chain; the calibration probe decides whether a
  phone at the mouth needs it before the WebRTC canceller is linked.
- The offline stretcher: signalsmith at a quality setting first, an A/B against Rubber Band on
  three tracks before paying for a licence.
- Stem storage on a LAN box: planar f32 is about 23 MB per stem minute; int16 storage or
  streaming decode if a box cannot hold two stems.
- House mics as a native capture input with drift correction, instead of the loopback sender.
- Device hotplug and the default-device change.
