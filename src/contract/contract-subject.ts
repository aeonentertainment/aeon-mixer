import type { OfflineEngine } from "../engine.ts";
import type { Mixer } from "../mixer.ts";
import type { DeviceInfo, MixerConfig, OpenEngineOptions } from "../types.ts";

/** The calls the contract suite makes on a mixer. */
export type ContractMixer = Pick<
  Mixer,
  | "slots"
  | "setSlot"
  | "setSlotEffects"
  | "resetSlot"
  | "push"
  | "start"
  | "destroy"
  | "stats"
  | "on"
>;

/** The calls the contract suite makes on an offline engine. */
export interface ContractEngine extends Pick<OfflineEngine, "output" | "pump" | "on" | "close"> {
  createMixer(config: MixerConfig): Promise<ContractMixer>;
}

/**
 * What a host hands the contract suite: this package's module, or the stand-in its own tests
 * drive in the package's place.
 */
export interface ContractSubject<E extends ContractEngine = ContractEngine> {
  openEngine(options: OpenEngineOptions & { output: { offline: true } }): Promise<E>;
  outputDevices(): Promise<ReadonlyArray<DeviceInfo>>;
}

/**
 * Something a stand-in does not do: one that only records calls renders no audio and opens no
 * socket. The package does both.
 */
export type ContractLimit = "rendersNoAudio" | "opensNoSocket";

export interface ContractOptions<E extends ContractEngine> {
  /**
   * Moves `engine` to another output, as the OS does when the default changes. An offline engine
   * never moves, so without this the move's promise is reported unchecked.
   */
  moveOutput?: (engine: E) => void;
  /**
   * What the subject does not do. Each promise that needs one of these is reported unchecked,
   * with why, and never broken; every other promise is checked whatever this says.
   */
  limits?: ReadonlyArray<ContractLimit>;
}

/**
 * What a promise's check needs beyond the calls every check makes: what the engine plays, a
 * loopback socket on an `l16` slot, or a move to another output.
 */
export type Need = "rendering" | "sockets" | "outputMoves";

/** What one promise's check is given. Every engine it opens is closed after it. */
export interface Trial {
  /** An offline engine, and a way to move it to another output, there when the check needs one. */
  open(
    options?: Omit<OpenEngineOptions, "output">,
  ): Promise<{ engine: ContractEngine; move: () => void }>;
  outputDevices: ContractSubject["outputDevices"];
}

/** A check's outcome: nothing when the promise held, else what it found or why it could not look. */
export type Verdict = { found: string } | { unchecked: string } | undefined;

/** One sentence a host relies on, and the check that proves it on the offline driver. */
export interface ContractPromise {
  promise: string;
  /** What its check needs; where the subject lacks one, the promise is unchecked and not run. */
  needs?: ReadonlyArray<Need>;
  check: (trial: Trial) => Promise<Verdict>;
}
