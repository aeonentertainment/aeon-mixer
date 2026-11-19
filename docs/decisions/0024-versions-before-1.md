# 0024: Before 1.0, a breaking change takes the next minor version

**Status:** accepted, 2026-11-15, with the developer's agreement: it is how never-compromise rule 4
in AGENTS.md reads until 1.0. A host pins a release's tarball, not an npm range, 2026-11-18, for
#261.

## The problem

Rule 4 says a breaking change to an exported type or behaviour is "a deliberate major bump with a
note in the decision log". The package is at 0.x, and the releases so far read "major" as the
minor number without saying so anywhere: 0007 and 0009 each mention it in passing for one change.
0.8.0 removes `Engine` as a value export (#169), the first release to remove an export, so the
reading needs to be written down before it ships.

## Decision

Until 1.0, the minor number is the major of rule 4.

- **A breaking change** ships in the next minor, 0.x to 0.(x+1), never in a patch, and is listed
  below with the release that carries it.
- **A patch** changes no exported type and no documented behaviour. The release checks this by
  diffing `src/` and the addon's `index.d.ts` against the last tag.
- **1.0** is the developer's call, made when Aeon Karaoke ships this package to people outside the
  team and its API has held still for a while. From then on rule 4 reads as written.

## Why not 1.0 now

A 1.0 tells anyone using the package that the API has stopped moving. It hasn't: the capture slot,
the voice stage and the output fallback all landed in the last week, and the one consumer pins an
exact version. What protects a user before 1.0 is that the number says whether an upgrade can
break them. A host depends on one release's tarball URL (#261), so nothing reaches it until it
moves the URL, and a new minor tells it to read this table first. A package manager reads 0.x the
same way: a caret stops at the minor, so `^0.7.1` takes 0.7.2 but never 0.8.0. A breaking change
in a patch would look safe to take, which is the side effect rule 4 forbids.

## Breaking changes

| Release | Change                                                                                                                                                                                                                                                                                                                                                                                                   | Record                                                                 |
| ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 0.10.0  | `correctorMaxDelayMs` reads 13.5 ms, not 15. Not breaking on its own: it still bounds `correctorDelayMs`, so a budget read from the field shrinks and stays true, and one that hard-coded 15 over-budgets by 1.5 ms. Listed because a documented figure moved.                                                                                                                                           | [#274](https://github.com/aeonentertainment/aeon-mixer/pull/274), 0021 |
| 0.10.0  | `correctorDelayMs` moves down in a rest, to about a quarter period of the singer's voice, where it used to hold until the next note. A host that only shows it, as `SlotStats` asks, sees lower figures between lines; one that held the last value through a rest should read it again.                                                                                                                 | [#274](https://github.com/aeonentertainment/aeon-mixer/pull/274), 0021 |
| 0.10.0  | A `pitch` event's `errorSemitones` is measured against the class nearest that event's own reading, which `src/types.ts` already said, where it came from the corrector's shorter steering reading. Not breaking for a host that took the documented meaning; one case reads differently: a reading under the corrector's bar within 30 ms of a sure one now reads 0, where it kept the sure one's error. | [#274](https://github.com/aeonentertainment/aeon-mixer/pull/274), 0010 |
| 0.10.0  | `playoutDelayMs` is measured from the first packet on: a stream whose first packet came late reads that lateness over the delay it opened at, for good when pinned. Budget on it, not the pin.                                                                                                                                                                                                           | #260, 0007                                                             |
| 0.9.0   | A backing stem is a `BackingStem`, a path or bytes, so `stem.path` read back from a config is `string \| undefined`: check `stem.path !== undefined` before using it.                                                                                                                                                                                                                                    | #207, #234                                                             |
| 0.9.0   | `SlotStats` gains a required `tone` field and `SlotEffect` a `"tone"` kind, so a hand-built fake adds the field and a switch over every kind needs a case.                                                                                                                                                                                                                                               | #205, 0029                                                             |
| 0.9.0   | `DeviceInfo`, what `outputDevices()` and `inputDevices()` list, gains a required `transport` field, which a hand-built fake must add.                                                                                                                                                                                                                                                                    | #255, 0025                                                             |
| 0.9.0   | `outputChanged`'s `gapMs` counts a stall: it runs from the old output's last callback, so after a stall it reads 200 ms or more larger. A host that added the stall itself stops.                                                                                                                                                                                                                        | #253, 0016                                                             |
| 0.8.0   | `Engine` is exported as a type only. `new Engine()` and `instanceof Engine` no longer work; `openEngine` is how an engine is made, as it always was.                                                                                                                                                                                                                                                     | #169, design.md section 7                                              |
| 0.8.0   | An `rtp` slot keeps the sender it is receiving from. A new sender waits for the old one's second of quiet unless the host calls `resetSlot`, so call `resetSlot` whenever a slot's sender changes.                                                                                                                                                                                                       | #203, 0028                                                             |
| 0.8.0   | An `l16` slot listens on `127.0.0.1` only, so an `l16` sender on another machine no longer reaches it.                                                                                                                                                                                                                                                                                                   | #194, #195                                                             |
| 0.7.0   | `OutputInfo` gains a required `refused` field, which a hand-built fake must add.                                                                                                                                                                                                                                                                                                                         | #155, #156                                                             |
| 0.6.0   | `IngestKind` gains `"capture"`, so a switch over every kind needs a case; `close()` releases every mixer the engine still holds.                                                                                                                                                                                                                                                                         | #109                                                                   |
| 0.5.0   | A slot that doesn't pin `playoutDelayMs` chooses its own delay, so the default moves.                                                                                                                                                                                                                                                                                                                    | 0007                                                                   |
| 0.4.0   | `OutputInfo` gains a required `device` field.                                                                                                                                                                                                                                                                                                                                                            | #104                                                                   |
| 0.3.0   | Pitch correction engages when a slot asks for it; until then the engine forced it off.                                                                                                                                                                                                                                                                                                                   | 0009                                                                   |
