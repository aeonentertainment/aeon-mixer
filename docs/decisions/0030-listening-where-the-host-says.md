# 0030: A slot listens only as far away as the host says

**Status:** accepted, 2026-11-20.

## The problem

An `rtp` slot bound every interface, and decision 0028 has a slot keep the first sender it hears.
So any machine that could reach the engine's address could be a slot's first sender, and hold the
slot until the host reset it or it went quiet for a second. A host whose `rtp` senders are on the
same machine, such as a media server relaying each singer's stream to the engine, had no way to
say so. An `l16` slot has listened on loopback only since 0.8.0, since its sender is always on
this machine.

## Decision

The engine takes audio only from where the host says.

- **An `rtp` slot listens on loopback by default.** `SlotConfig` takes `listen: "loopback"`, the
  same as leaving it out, or `listen: "every-interface"`, which binds every interface, for a
  sender on another machine, such as a phone sending straight to the engine.
- **Checked twice, refused in the engine's words.** `createMixer` refuses an unknown `listen` on an
  `rtp` slot before anything crosses to the native side, and the addon refuses it again for a
  caller that skips the check. On any other kind `listen` is not read, as `device` and `channel`
  are read on a `capture` slot alone, and such a slot binds loopback as it always has.
- **`l16` does not take it here.** Its sender is always on this machine. If a host ever needs an
  `l16` sender on another machine, the same option fits it, as an additive change.
- **The contract suite holds it.** A promise, which needs a socket, sends to an `rtp` slot's port
  on an IPv4 address of this machine that is not loopback: a slot left at its default does not
  hear it, and one whose `listen` is `every-interface` does. A machine with no such address
  leaves it unchecked, with why.

## The cost

It changes a default, so it ships in the next minor (decision 0024). A host that relied on
reaching an `rtp` slot from another machine adds `listen: "every-interface"` to those slots. A
host whose senders are on loopback changes nothing.

## What it does not do

- **It is not authentication.** A slot that listens on every interface takes its first sender
  from anywhere it is reachable, as before, and a process on this machine can always reach a
  slot that listens on loopback. The transport is unauthenticated by design (SECURITY.md).
- **No per-address bind.** A host cannot name one interface or one subnet. Loopback or every
  interface covers a sender on this machine and one on another; a narrower bind can come as
  another value of `listen` if a host needs it.
- **IPv4 only, as before.** Loopback is `127.0.0.1` and every interface is `0.0.0.0`.
