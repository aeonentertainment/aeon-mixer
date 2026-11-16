# Security policy

Aeon Mixer will run inside Aeon Karaoke devices on home networks, so we take reports seriously.

**Report a vulnerability privately** through GitHub's
[private vulnerability reporting](https://github.com/aeonentertainment/aeon-mixer/security/advisories/new).
Do not open a public issue or pull request for it.

Include what you found, how to reproduce it, and the impact you expect. We will acknowledge the
report, work with you on a fix, and credit you in the advisory unless you prefer otherwise.

## Scope

- This repository's source and release artifacts.
- The LAN audio transport: RTP over UDP to each slot's port. It is unencrypted and
  unauthenticated by design, the same trust model as the device's WebSocket. A slot plays one
  sender's address at a time, until the host resets it or that sender is quiet for a second
  (decision 0028). A report that shows a practical attack within that model is welcome.

Vulnerabilities in Aeon Karaoke itself belong to that repository's policy.
