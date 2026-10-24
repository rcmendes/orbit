# Changelog

## 0.4.0 - 2026-10-24

- `orbit workspace <folder>` creates an independent shared Board at `<folder>/board` for work that spans repositories. It only creates; each repository joins with `orbit link <folder>`, which now accepts a workspace root as well as a Board root. A workspace inside a Git working tree is rejected with `WORKSPACE_INSIDE_REPO`.
- Added SubTasks: `orbit add --parent <id>`, the `parent` frontmatter field, derived Parent Task readiness, `list --parent`/`--tree`, `ready --parent`, and `SUBTASKS_OPEN` guards on claim, done, cancel, and delete. `done --force` and `cancel --force` cancel open SubTasks first; `delete --force` removes the whole subtree; reopening a SubTask reopens a done parent. `doctor` reports `DANGLING_PARENT` and `NESTED_SUBTASK`. MCP `create`, `list`, and `ready` accept `parent`.
- Task IDs now use the dot hierarchy: `orb-a0b1c2` for a Task and `orb-a0b1c2.d4b3df` for a SubTask. Legacy `orb-xxxxxx` IDs remain valid Tasks. `id.prefix` is limited to lowercase letters, digits, and dashes, and `doctor` reports `PARENT_MISMATCH` when an ID and its `parent` field disagree.
- `orbit list`, `orbit ready`, and `orbit prime` accept repeatable `--label` and `--repo NAME` (shorthand for `repo:NAME`); every filter must match. `orbit export --json` adds a deterministic, schema-versioned snapshot of every task, including terminal work, comments, labels, and dependency/parent edges. MCP `list` and `ready` accept `repo` and `labels`.

## 0.3.0 - 2026-10-24

- Repository documentation now lives under `docs/`: `SPEC.md`, `GLOSSARY.md`, and `RELEASING.md` moved there, while `README.md`, `AGENTS.md`, and `CHANGELOG.md` remain at the root. The changelog stays at the root because cargo-dist discovers release notes there.
- The Wayfinder conventions in [the tracker guide](./docs/agents/issue-tracker.md) now model a map as a `wayfinder:map` Task and its children as `wayfinder:<map-id>` Tasks, reserve `orbit depend` for real Dependencies, and scope the frontier to the map's Ready children instead of the whole Board.
- `orbit setup opencode` now writes the OpenCode V2 `mcp.servers.orbit` entry with `type: local` and no `enabled` flag, and moves a legacy `mcp.orbit` entry to that location without duplicating the server. Removing the integration clears both forms and preserves unrelated servers.
- Added [architecture diagrams](./docs/ARCHITECTURE.md) covering the component map, board resolution, domain relationships, state machine, readiness gate, command flow, workspace initialization, task lifecycle, concurrent claims, and the MCP bridge.
- `orbit <command> --help` now carries a long description and copy-pasteable examples for every command and subcommand. The new [command guide](./docs/commands.md) explains when each command fits, with end-to-end scenarios and a common-errors table.
- `orbit done` and `orbit cancel` accept `--comment`/`--comment-file`, so the terminal status and its note are written and committed together. MCP `finish` accepts `comment` as well.
- `orbit setup` now generates the same `orbit done <id> --by <agent> --comment "..."` completion guidance as the checked-in `AGENTS.md`, and `orbit prime` emits that example too, so `setup --check` no longer reports the managed instruction block as stale. The Matt Pocock tracker instructions use the example as well.
- `orbit setup mattpocock` now leaves a hand-written `docs/agents/issue-tracker.md` without Orbit's managed marker untouched and reports it as `skipped`; `orbit init` reports the same file as skipped when it detects one. `--check` treats the custom doc as installed, `--remove` does not delete it, and agent refresh still recognizes it; tracker files with the marker update and remove normally.
- CI now runs `orbit setup opencode --no-mcp --check`, so the checked-in `AGENTS.md` managed block cannot drift from the generator. `--no-mcp` keeps machine-local MCP configuration out of the check.
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
