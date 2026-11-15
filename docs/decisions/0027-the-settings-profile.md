# 0027: The settings profile, one home for what nobody chose

**Status:** accepted, 2026-11-14, ahead of the code in #180.

## The problem

A session bundle (decision 0026) records every call a host made. That is not enough to say what
settings a song ran under, or to try other ones against it, because the engine has no defaults
worth the name. A slot's effects start at zero, so a host names every value on every call: Aeon
Karaoke keeps correction's strength and reverb's mix in its own model and passes them each time.
The playout delay is an option on `openEngine`, and the stem gains are required at `createMixer`.

So every value in a bundle looks like a choice. A replay cannot tell the singer who asked for full
correction from the host's standing 0.6, and has nothing to swap.

## A choice and a default are different things

The profile holds what the engine does when nobody chose. A call that names a value is a choice,
and a call that leaves it out takes the profile's. That one rule is the whole design:

- A bundle's events keep what each call left out as left out, so the split survives recording.
- A replay under another profile changes the defaults and keeps every choice.
- A host moves its own defaults here by ceasing to pass them, one call at a time.

## One object, at `openEngine`

Rejected:

- **A profile per mixer.** The period and the playout delay are the engine's, fixed for its life.
  A profile split across two calls is two homes again.
- **Changing it on a running engine.** The period cannot change without reopening the output, and
  nothing yet needs the rest to. A host that wants new defaults opens a new engine.
- **Leaving `periodFrames` and `playoutDelayMs` out of it**, since options exist for them. A
  bundle then needs the options as well as the profile to say what ran. They stay as options
  that win, so no host breaks, and `Engine.profile` reads back the one resolved object.

## The defaults are Aeon Karaoke's

Strength 0.6, retune 40 ms, room size 0.5, mix 0.25. They are the only values the engine has been
sung with, so a host that stops passing them hears no change. Whether they are the best values is
exactly what replaying bundles under other profiles is for.

## The engine's constants stay out

The gate, the compressor and the delay chooser hold about thirty constants a tuner would want.
An exported type is the public API (never-compromise 4), and thirty fields nobody has swept yet
would be thirty promises. A constant joins under `tuning`, marked as free to change in a minor
version, in the PR that first sweeps it, with the measurement that made it worth exposing.

## Strict parsing

`parseProfile` refuses a field it does not know. A profile is written by hand or by a tuning run
and read months later by another version. Ignoring an unknown field would let a misspelt or
retired setting pass as one that was tried and did nothing, which is the worst result a
comparison can have.

## What it costs the mic path

Nothing. The profile is resolved on the control thread into the parameters the engine already
takes.
