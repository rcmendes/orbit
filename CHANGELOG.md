# Changelog

## Unreleased

- The Wayfinder conventions in [the tracker guide](./docs/agents/issue-tracker.md) now model a map as a `wayfinder:map` Task and its children as `wayfinder:<map-id>` Tasks, reserve `orbit depend` for real Dependencies, and scope the frontier to the map's Ready children instead of the whole Board.
- Added [architecture diagrams](./docs/ARCHITECTURE.md) covering the component map, board resolution, domain relationships, state machine, readiness gate, command flow, workspace initialization, task lifecycle, concurrent claims, and the MCP bridge.
- `orbit <command> --help` now carries a long description and copy-pasteable examples for every command and subcommand. The new [command guide](./docs/commands.md) explains when each command fits, with end-to-end scenarios and a common-errors table.
- `orbit done` and `orbit cancel` accept `--comment`/`--comment-file`, so the terminal status and its note are written and committed together. MCP `finish` accepts `comment` as well.
- `orbit setup` now generates the same `orbit done <id> --by <agent> --comment "..."` completion guidance as the checked-in `AGENTS.md`, and `orbit prime` emits that example too, so `setup --check` no longer reports the managed instruction block as stale. The Matt Pocock tracker instructions use the example as well.
- `orbit setup mattpocock` now leaves a hand-written `docs/agents/issue-tracker.md` without Orbit's managed marker untouched and reports it as `skipped`; `orbit init` reports the same file as skipped when it detects one. `--check` treats the custom doc as installed, `--remove` does not delete it, and agent refresh still recognizes it; tracker files with the marker update and remove normally.
- `orbit init` now creates the Board and wires the coding-agent harnesses it detects in one command. In a terminal it asks which harnesses to wire; without one it is deterministic. Running it again reconciles integration instead of returning `ALREADY_INITIALIZED`.
- `orbit init --board PATH` creates a shared Board and links the repository; `orbit link PATH`, `orbit unlink`, and `orbit setup --check` are new.
- A Workspace can span repositories. Discovery order is `--dir`, `ORBIT_DIR`, the repository link, then the repository's own Board; the link is shared by all linked worktrees.
- The Claude SessionStart hook records the absolute Orbit binary, so it works when `orbit` is not on `PATH`.
- `orbit init` and `orbit setup` refuse to record a binary inside npm's prunable `_npx` cache or a temporary directory, with guidance to install globally or pass `--mcp-bin orbit`.
- `orbit doctor` reports whether `orbit` is on `PATH` and warns when a configured Claude hook cannot find it.
- `.github/workflows/smoke.yml` installs the published artifacts on Linux, macOS, and Windows and exercises the onboarding sequence. The release workflow also runs an artifact smoke job before npm publication, so a broken binary blocks the npm release.

## 0.2.0 - 2026-10-23

- `orbit setup` now installs MCP configuration by default for Claude, Cursor, Codex, and OpenCode. MCP entries record the absolute path of the running Orbit binary, so editors launched outside a shell no longer need `~/.cargo/bin` on `PATH`. Use `--no-mcp` to install instruction files only, or `--mcp-bin orbit` for a portable command that relies on the editor's `PATH`.
- Labels on tasks, with configured hold labels excluded from readiness and claims.
- Append-only per-task comments with author and timestamp, available through `orbit comment`.
- `orbit label`, `list --label`/`--unlabeled`, and `doctor` checks for comments.
- MCP `label` and `comment` tools; 13 tools total.
- Triage integration: `docs/agents/triage-labels.md` and updated issue-tracker guidance.

## 0.1.0 - 2026-10-23

- Initial pre-alpha: Markdown board on a dedicated Git branch, atomic claims and leases, strict JSON CLI, agent setup recipes, and the stdio MCP server.
- Release tooling: cargo-dist, shell/PowerShell installers, and the `@rcmendes/orbit` npm package.
