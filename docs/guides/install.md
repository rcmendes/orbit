# Install details

Every install route, prerequisites, and verification.

Prebuilt binaries are the primary path. `v0.12.0` is the latest release. Every persistent route requires Git 2.31+ at runtime.

| Route | Prerequisites | Command |
| --- | --- | --- |
| macOS / Linux installer | Git 2.31+ | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.sh \| sh` |
| Windows installer | Git 2.31+ | `irm https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.ps1 \| iex` |
| npm (global) | Node.js 14.14+, npm 6+, Git 2.31+ | `npm install -g @rcmendes/orbit` |
| npm (one-off trial) | Node.js 14.14+, npm 6+ | `npx @rcmendes/orbit --version` |
| Source, no clone | Rust 1.89+, Git 2.31+ | `cargo install --git https://github.com/rcmendes/orbit --locked orbit-tracker` |
| Source, local clone | Rust 1.89+, Git 2.31+ | `cargo install --path . --locked` |

After any persistent route, verify the binary before creating a Board:

```bash
orbit --version
```

The installer adds its directory to `PATH` unless it is already there, and tells you what to source if a restart is needed. The npm package is scoped because the bare name is taken; the command is always `orbit`.

npx is only for a temporary trial. `orbit init --agents all` and `orbit setup` refuse to record a binary inside npm's prunable `_npx` cache or a temporary directory. Install globally, or run `orbit setup --portable-mcp` before wiring agents.

See [RELEASING.md](../../docs/RELEASING.md) for the release process, supported targets, and packaging checks.
