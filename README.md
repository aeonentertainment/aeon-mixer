# Aeon Mixer

A low-latency audio mixer for [Aeon Karaoke](https://github.com/aeonentertainment/aeon-karaoke).
Phones on the LAN, wired mics and wireless mics are the microphones. The mixer takes those mic
streams and a backing track, and plays one mix to the room.

**Status: design stage.** The package exports a stub. The design is in
[docs/design.md](docs/design.md); the reasons are in [docs/decisions](docs/decisions).

## Development

```bash
vp install   # dependencies (installs Vite+ if needed: npm i -g vite-plus)
vp check     # format, lint, type-check
vp test      # unit tests
vp pack      # build dist/
```

Node 24 and [Vite+](https://viteplus.dev) are the only requirements. A pre-commit hook runs
`vp check --fix` on staged files; `vp hooks disable` turns it off for your clone.

Agents start at [AGENTS.md](AGENTS.md). Contributors start at [CONTRIBUTING.md](CONTRIBUTING.md).
Security reports go through [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
