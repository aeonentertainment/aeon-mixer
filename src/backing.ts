import type {
  BackingInput,
  NativeBackingRender,
  NativeEngine,
  NativeMixer,
  StemGainsInput,
} from "./native.ts";
import type { MixerConfig, StemKind } from "./types.ts";
import { msToFrames } from "./units.ts";
import { finiteIn, integerIn } from "./validate.ts";

export type BackingConfig = NonNullable<MixerConfig["backing"]>;

/** `stopAtMs` fades the stems out over this before falling silent: docs/design.md section 4. */
export const STOP_FADE_MS = 3000;

/** What the backing path needs from its engine: word of its render job, as the events thread reports it. */
export interface RenderHost {
  readonly native: NativeEngine;
  /**
   * Reports the render job's progress until it finishes, then resolves; rejects when the engine
   * stops first.
   */
  watchRender(id: number, onProgress: (fraction: number) => void): Promise<void>;
}

/**
 * One mixer's backing path while it is armed: the render job on the worker thread and the
 * settings held for start. `install` moves the rendered stems into the mixer's graph.
 */
export class Backing {
  readonly speed: number;
  readonly stopAtFrames: number | undefined;
  /** Resolves when the render job is done, with stems or an error that `install` reports. */
  readonly rendered: Promise<void>;
  #rendering = true;
  #installed = false;
  #fraction = 0;
  #stemGains: StemGainsInput;
  readonly #render: NativeBackingRender;

  /** Checks `config` and starts its render on the worker thread. */
  constructor(host: RenderHost, config: BackingConfig, onProgress: (fraction: number) => void) {
    const input = toInput(config);
    this.speed = input.speed;
    this.stopAtFrames =
      config.stopAtMs === undefined
        ? undefined
        : msToFrames(finiteIn("stopAtMs", config.stopAtMs, 0, Number.MAX_SAFE_INTEGER));
    this.#stemGains = {
      instrumental: finiteIn("stemGains.instrumental", config.stemGains.instrumental, 0, 4),
      guide: finiteIn("stemGains.guideVocals", config.stemGains.guideVocals, 0, 4),
    };
    this.#render = host.native.startRender(input);
    // The final 1 is announced once `rendered` has settled, so a host that starts the mixer from
    // its renderProgress listener finds the stems ready
    this.rendered = host.watchRender(this.#render.id, (fraction) => {
      this.#fraction = fraction;
      if (fraction < 1) onProgress(fraction);
    });
    this.rendered.then(
      () => {
        this.#rendering = false;
        if (this.#fraction === 1) onProgress(1);
      },
      () => {},
    );
  }

  /** Whether `install` would still wait on the worker thread. */
  get rendering() {
    return this.#rendering;
  }

  get stemGains() {
    return this.#stemGains;
  }

  /**
   * Moves the rendered stems into `graph`, once; the graph keeps them through a refused swap.
   * Throws the engine's message when the render failed.
   */
  install(graph: NativeMixer) {
    if (this.#installed) return;
    graph.installBacking(this.#render);
    this.#installed = true;
  }

  setStemGain(kind: StemKind, gain: number) {
    const checked = finiteIn(`${kind} gain`, gain, 0, 4);
    this.#stemGains =
      kind === "instrumental"
        ? { ...this.#stemGains, instrumental: checked }
        : { ...this.#stemGains, guide: checked };
    return this.#stemGains;
  }
}

/** Half to double speed: what a speed control offers a singer. */
const SPEED_RANGE = [0.5, 2] as const;

function toInput(config: BackingConfig): BackingInput {
  let instrumental: string | undefined;
  let guide: string | undefined;
  for (const { kind, path } of config.stems) {
    if (typeof path !== "string" || path.length === 0) {
      throw new TypeError(`backing.stems: the ${kind} stem needs a path`);
    }
    switch (kind) {
      case "instrumental":
        if (instrumental !== undefined) {
          throw new RangeError("backing.stems holds two instrumental stems");
        }
        instrumental = path;
        break;
      case "guideVocals":
        if (guide !== undefined) throw new RangeError("backing.stems holds two guideVocals stems");
        guide = path;
        break;
    }
  }
  if (instrumental === undefined) throw new RangeError("backing.stems needs an instrumental stem");
  return {
    instrumental,
    ...(guide === undefined ? {} : { guide }),
    semitones: integerIn("key", config.key, -12, 12),
    speed: finiteIn("speed", config.speed, ...SPEED_RANGE),
  };
}
