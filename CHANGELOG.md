# Changelog

## Unreleased

- Added `orbit workspace migrate` to preview and consolidate legacy Workspace and child-repository Boards into `<workspace>/.orbit/board`. It preserves source Boards, Tasks, comments, claims, Checkpoints, and relationships; remaps only colliding IDs; retains source prefixes as accepted legacy prefixes; reports config differences; blocks duplicate GitHub Issue identities; and supports resumable application with validation and rollback guidance.

## 0.9.4 - 2026-10-28

- Treat `admin:org` and `write:org` as satisfying the `read:org` requirement for full GitHub mode. Organization administrators no longer receive a false `GITHUB_SCOPES_MISSING` warning.

## 0.9.3 - 2026-10-28

- Fixed live GitHub GraphQL calls to use `gh api graphql` and a complete query document. Organization Issue Field discovery and creation now work against the real GitHub API; the previous release only worked through the local fixture client.

## 0.9.2 - 2026-10-28

- Fixed live GitHub organization Issue Field discovery. GitHub exposes the `IssueFields` union with concrete field types, so `orbit github setup` and `orbit github setup --create-fields` now query those concrete types instead of the non-existent `IssueField` type. This is required for organization-backed setup against the current GitHub GraphQL API.

## 0.9.1 - 2026-10-28

- Wayfinder children are SubTasks labelled `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task`. Map membership is the parent edge (`orbit list --parent` / `orbit ready --parent`). Claims stay Orbit leases; GitHub assignees are not Wayfinder ownership.
- `orbit triage` rejects a proposal whose resulting labels would contain more than one state role (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`) or more than one category role (`bug`, `enhancement`). The error is `TRIAGE_INVALID_PROPOSAL` and names the conflicting roles. Proposals that do not add or remove labels still apply, and a Task may have neither role.
- Documented the boundary between Orbit and Matt Pocock's engineering skills in the tracker guide and triage workflow. Orbit tracks work; `orbit triage` only enriches `area`, verification text, and canonical labels. The skills own context gathering, claim verification, grilling, agent briefs, and `.out-of-scope/`. Added [the agent-brief template](./docs/agents/agent-brief.md).

## 0.9.0 - 2026-10-28

- This release includes `orbit github` and `orbit triage` in the default binaries, shell/PowerShell installers, and npm package. GitHub sync requires Orbit v0.9.0 or newer; personal-account repositories use basic mode, which syncs labels and GitHub-owned fields but does not sync `area` or `verification`.
- Expanded the artifact onboarding smoke on Linux, macOS, and Windows to exercise GitHub connect/status, adopt/create, sync preview/apply, and Triage review/apply with the local fixture client and no network access. Added a clear upgrade error for configured GitHub Boards loaded by builds without the default `github-provider` capability.
- Added claim checkpoints. `orbit checkpoint <id> --progress ... [--next ...] [--blocker ...]` records the claim holder's latest handoff state in the Task's frontmatter; later calls can update one field or clear the next action/blocker. The record survives `release`, `claim --steal`, and `reopen`, appears in `show`/`ready`/`list`/`prime`, MCP `get`, and `export`, and never changes readiness or claim eligibility. A done or cancelled Task keeps it as archived history (`checkpoint_archived: true`) but refuses new checkpoints. Exposed over MCP as `checkpoint`; not synchronized to external issue trackers.
- Updated the README, command guide, specification, architecture, glossary, tracker guide, ADR 0013, CLI/MCP help, generated agent guidance, and integration tests for checkpoints.

## 0.8.2 - 2026-10-26

- `orbit init` now prints a numbered setup report instead of a terse status line. It states the Board path, branch, storage mode, Task-ID prefix, and Git exclusion; the harnesses wired, MCP command mode, and every managed file; then `orbit doctor` and the next commands, each annotated with what it does. `--print` uses future tense (`would create`, `would list`), states that nothing was written, and keeps the generated file contents. A standalone Board always includes `orbit link DIR` in its next steps, and re-runs report unchanged MCP configs instead of omitting them.
- The `init` harness and MCP prompts now explain what each choice writes and what it requires, and the installer success message names `orbit init` as the next step for creating a Board and wiring agents.
- Orbit's description is now consistent in the README, Cargo metadata, CLI help, and generated agent instructions: a Git-native issue tracker for humans and coding agents, with an atomic work queue.

## 0.8.1 - 2026-10-26

Supersedes 0.8.0, which remains published and deprecated on npm after failing Windows CI. This release contains the Orbit-folder feature set from 0.8.0 plus:

- Hardened `doctor --migrate`: it refuses to remove a legacy Board when task or comment Markdown diverges, never removes a dirty worktree, re-attaches an orphan `orbit` worktree from any command, honors `--dir` in `doctor --fix`, warns when other repositories still link to a moved workspace, escapes Git exclude patterns, and prints the discovery trace when a repair fails.
- Legacy worktree moves now fall back to a directory rename that updates the worktree's `gitdir` pointer directly when Git for Windows refuses `git worktree move`, so migration works on Windows.
- Fixed Windows CI coverage for absolute Board paths and user-scope setup preview output. The suite now passes on macOS, Linux, and Windows.

## 0.8.0 - 2026-10-26

Superseded by 0.8.1 for Windows compatibility.

- `orbit init [DIR] [--slug SLUG] [--global]` is now the only Board-creation command. `orbit workspace`, `init --board`, and `init --local` are removed. `init` defaults to `./.orbit`, derives the slug from `DIR` (or its parent when named `.orbit`), refuses a second repository-local Board, writes the worktree exclusion entry when applicable, and prints the next step. `--global` targets `~/.orbit` in repository mode and conflicts with a positional `DIR` or `--dir`/`ORBIT_DIR`. `init` no longer writes a link pointer; use `orbit link DIR` to join a shared Board.
- `orbit link DIR` treats `DIR` as the Orbit folder, accepts a legacy workspace or board root with a `LEGACY_LAYOUT` warning that continues on later commands, and writes an absolute pointer in the Git common directory so every worktree shares the same Board and lock. It refreshes managed agent files without changing the configured MCP command mode and reports every shadowed local Board, including a legacy `<git-common-dir>/orbit/board`. A target that holds no Board returns `LINK_NOT_FOUND`. `orbit unlink` always refreshes managed files and falls back to the main checkout's `.orbit` when it exists.
- The Orbit folder resolver now treats `DIR` as the Board root. `DIR` defaults to `./.orbit`; a Board inside a Git working tree is a worktree on the `orbit` branch, `board.storage` records `worktree` or `repository`, and the data lock lives at `<DIR>/.lock`. Bare discovery walks the nearest `.orbit`, the main checkout, legacy layouts, and the existing global `~/.orbit`; `LEGACY_LAYOUT` and `AMBIGUOUS_BOARD` are reported as warnings. Init also prunes and re-attaches an orphaned worktree without losing Tasks.
- `orbit init` now offers the first-run MCP choice between the absolute Orbit binary path (default, for GUI-launched editors) and the portable `orbit` command, which requires Orbit on the editor's `PATH`. Both `init` and `setup` accept `--portable-mcp` for non-interactive and repair use; re-running with the flag is idempotent, an unflagged repair returns to the absolute-path default, and `link`/`unlink` refresh preserves the configured mode. Help text and docs explain the trade-off.
- `orbit setup` accepts `--scope project|user` (default `project`). Project scope writes repository files; user scope writes the harness's user-level instruction and MCP locations, and `--print`, `--check`, and `--remove` honor the chosen scope. Generated configs no longer record the Board path; they rely on discovery or `--dir`/`ORBIT_DIR`, so moving the Orbit folder does not stale them. Without `--agents`, `init` prompts only in a TTY; a non-TTY run writes no agent files and prints the `orbit setup` hint, while `--agents` and `--print` never prompt.
- `orbit doctor` now runs without a Board, prints the discovery trace, and reports `LEGACY_LAYOUT`, `AMBIGUOUS_BOARD`, `ORPHAN_WORKTREE`, `STORAGE_MISMATCH`, and `ABSOLUTE_BOARD_PATH`. `--fix` removes stale temp files and re-attaches an orphaned `orbit` worktree; `--migrate` moves a legacy hidden or workspace Board to the Orbit folder explicitly and resumably.
- Documented the Orbit folder design in [ADR 0012](./docs/adr/0012-orbit-folder.md) and updated the README, command guide, specification, architecture, glossary, tracker guide, releasing guide, and the amended ADRs 0001/0002/0006 and replaced ADR 0008. A Board lives in one delegated `DIR` (default `.orbit`); `orbit init [DIR]` becomes the only creation command; `orbit workspace`, `--board`, and `--local` are removed; discovery, path-free agent setup, and legacy migration are specified. Unified init, the resolver, scoped setup, and `doctor` migration have landed; removing legacy discovery remains the next release.
- Expanded the README and command guide with parameter-by-parameter `init` and `setup` guidance and concrete scenarios for local projects, shared workspaces, explicit DIR, global Boards, CI setup, joining existing Boards, and legacy migration.
- The packaged-install smoke now runs in an isolated project and home directory, wires harnesses detected by `orbit init --agents auto`, verifies the generated MCP command matches the Orbit binary under test, and completes an MCP initialize/tools-list handshake through that command.
- `orbit init --print` previews without writing or prompting. `--agents` selects integrations for automation; without a flag, a TTY asks and a non-TTY run only prints the next step.

## 0.7.0 - 2026-10-25

- Added `orbit slug show` and `orbit slug migrate <SLUG>`. `show` reports the bare slug, literal prefix, task/SubTask counts, and comment-file count. `migrate` keeps every random suffix and rewrites Task and SubTask IDs, `parent`/`depends_on`, comment filenames and Orbit headers, and `id.prefix` as one atomic board commit. It supports `--dry-run`, `--json`, and `--force`; refuses an unhealthy Board, prefix collisions, and live claims without `--force`; and is intentionally not exposed over MCP. Task bodies, comment text, Git history, and external links are not rewritten. `SLUG_LOCKED` now points to `orbit slug migrate`.
- Updated the README, command guide, specification, architecture, glossary, ADR 0011, CLI help, and integration tests for the slug migration workflow.

## 0.6.0 - 2026-10-25

- Added per-board Task-ID slugs. `orbit init` adopts the main checkout folder name, `orbit workspace` adopts the workspace folder name, and `init --board` defaults to `orb`; `--slug` overrides on both commands. Re-runs preserve the prefix; empty Boards can change it, while populated Boards return `SLUG_LOCKED`. A hand-edited new-style prefix returns one `PREFIX_MISMATCH` instead of one error per file, and `export` carries `data.board.prefix` (schema 2).
- `orbit list` now renders an aligned table with `ID`, `STATUS`, `P`, `TYPE`, `TITLE`, `LABELS`, and `CLAIMED BY` columns. Root Tasks are ordered by priority, then creation time and ID; each Parent Task is followed by its SubTasks, also ordered by priority. `--group` renders one outlined table with a banner for each Parent Task, its SubTasks underneath, and a final `No parent` section. Titles are shortened to 60 characters in the table; `--json` remains unchanged and carries the full title and every field. The `--tree` flag was removed; use `--group` for the grouped view.
- Updated the README, command guide, architecture, specification, glossary, agent tracker guide, CLI help, generated agent instructions, and this changelog to describe the table view and the current command surface.

## 0.5.0 - 2026-10-24

- Added a first-class `type` field: `task` (default), `epic`, `spike`, `chore`, or `decision`. Type is metadata and does not affect readiness; `kind` remains `task`/`subtask`. `epic` is top-level only. Every save writes `type`, including `type: task`; pre-type files read as `task`.
- Added `--type` to `add`/`update`, repeatable `--type`/`--exclude-type` to `list`/`ready`, MCP `type`/`types`/`exclude_types`, JSON `type`, and non-task prefixes in human output. The `epic` label is now legacy: no backfill or inference, and `--exclude-label epic` still works.
- `orbit list` and `orbit ready` accept repeatable `--exclude-label`; MCP `list` and `ready` accept `exclude_labels`. A task carrying any excluded label is hidden, so agents can skip `epic`/`spec` containers without changing the board's hold-label configuration.
- Task JSON now carries `subtask_count`, `resolved_subtask_count`, `open_subtask_count`, and `eligible_for_close` in `show`, `list`, `ready`, and `export`. Export moves to `schema_version` 2.
- Added `orbit tree <id>`, a scoped view of one Parent Task, its SubTasks, resolved/total progress, and blocking edges. `--json` returns `data.root`, `data.tasks` with the root first, and `data.count`.
- `orbit prime` now reports `close_candidates`: non-terminal Parent Tasks with at least one SubTask and no open SubTasks. The human output marks each as ready to close; nothing is auto-closed.
- Documented the Beads and Matt Pocock compatibility mapping: Issue = Task, Sub-issue = SubTask, Epic/spec = top-level Task with `type: epic`. Added ADR 0009 and ADR 0010, the one-level hierarchy limit, the frontier command, and the `done --force`/`cancel --force` child-cancellation warning to the README, SPEC, glossary, tracker guide, setup templates, and CLI help.

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
