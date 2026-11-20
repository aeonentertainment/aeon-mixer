// `@aeonentertainment/mixer/contract`: the promises a host relies on, checked on the offline
// driver. It imports no addon, so a host's stand-in for the package is checked without one.
export { checkContract, type ContractReport } from "./check-contract.ts";
export type {
  ContractEngine,
  ContractLimit,
  ContractMixer,
  ContractOptions,
  ContractSubject,
} from "./contract-subject.ts";
