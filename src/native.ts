// The addon and its loader, as `vp run build:native` writes them. Loaded with `require`, since the
// loader is CommonJS and a `.node` file can only be required. Re-exported here and never widened.
import { createRequire } from "node:module";
import type * as Binding from "../native/binding/index.js";

const binding: typeof Binding = createRequire(import.meta.url)("../native/binding/index.js");

export const NativeEngine = binding.Engine;
export type NativeEngine = Binding.Engine;
export type NativeMixer = Binding.Mixer;
export type NativeBackingRender = Binding.BackingRender;
export type {
  BackingInput,
  GuideInput,
  NativeEvent,
  OutputEntry,
  OutputInfo as NativeOutputInfo,
  SlotParamsInput,
  StemGainsInput,
} from "../native/binding/index.js";
