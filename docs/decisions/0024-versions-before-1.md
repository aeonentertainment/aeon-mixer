# 0024: Before 1.0, a breaking change takes the next minor version

**Status:** proposed, 2026-11-14. Needs the developer's agreement: it is how never-compromise rule
4 in AGENTS.md reads until 1.0.

## The problem

Rule 4 says a breaking change to an exported type or behaviour is "a deliberate major bump with a
note in the decision log". The package is at 0.x, and the releases so far read "major" as the
minor number without saying so anywhere: 0007 and 0009 each mention it in passing for one change.
0.8.0 removes `Engine` as a value export (#169), the first change that breaks code using the API as
it was, so the reading needs to be written down before it ships.

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
exact version. What protects a user before 1.0 is how npm reads a range: a caret on 0.x stops at
the minor, so `^0.7.1` takes 0.7.2 but never 0.8.0. A breaking change in a patch would reach every
such range by itself, which is the side effect rule 4 forbids.

## Breaking changes

| Release | Change                                                                                                                                               | Record                    |
| ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| 0.8.0   | `Engine` is exported as a type only. `new Engine()` and `instanceof Engine` no longer work; `openEngine` is how an engine is made, as it always was. | #169, design.md section 7 |
| 0.7.0   | `OutputInfo` gains a required `refused` field, which a hand-built fake must add.                                                                     | #155, #156                |
| 0.6.0   | `IngestKind` gains `"capture"`, so a switch over every kind needs a case; `close()` releases every mixer the engine still holds.                     | #109                      |
| 0.5.0   | A slot that doesn't pin `playoutDelayMs` chooses its own delay, so the default moves.                                                                | 0007                      |
| 0.4.0   | `OutputInfo` gains a required `device` field.                                                                                                        | #104                      |
| 0.3.0   | Pitch correction engages when a slot asks for it; until then the engine forced it off.                                                               | 0009                      |
