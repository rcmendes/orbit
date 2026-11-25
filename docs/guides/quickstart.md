# Quickstart

Install Orbit, create a Board, file work, and let an agent take it — in about five minutes.

## 1. Install

Orbit's Board requires **Git 2.31+**. The npx trial can check the binary without it, but every persistent route needs Git. Pick one persistent route; the npm and source routes have additional prerequisites.

**macOS / Linux** — Git 2.31+:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.sh | sh
```

**Windows PowerShell** — Git 2.31+:

```powershell
irm https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.ps1 | iex
```

**npm** — Node.js 14.14+, npm 6+, and Git 2.31+:

```bash
npm install -g @rcmendes/orbit
```

The npm package is scoped because the bare name is taken. The command is always `orbit`.

`npx` is a trial path, not a persistent install. It downloads Orbit into npm's prunable `_npx` cache, which is fine for checking the CLI but cannot be recorded by automatic agent setup. Use a persistent install before wiring agents:

```bash
npx @rcmendes/orbit --version
```

**From source** — Rust 1.89+ and Git 2.31+:

```bash
# Without an existing clone
cargo install --git https://github.com/rcmendes/orbit --locked orbit-tracker

# From a local clone
cargo install --path . --locked
```

Verify the installed command is on `PATH`:

```bash
orbit --version
```

If the command is not found, add the install directory to `PATH` and restart your shell. Cargo installs binaries under `$CARGO_HOME/bin` (`%USERPROFILE%\.cargo\bin` by default on Windows). The shell installer prints the directory it added; for global npm, use `npm prefix -g` plus `bin` on macOS/Linux, or the reported prefix itself on Windows.

## 2. Create the Board

```bash
cd your-repository
orbit init
```

In a Git repository, `orbit init` creates the Board at `./.orbit/board` — a standalone Git repository anchored by the `./.orbit` Orbit folder, with the rebuildable cache at `./.orbit/cache`; the Orbit folder is hidden through `.git/info/exclude`. `orbit init .` and a `DIR` that is already a repository root do the same. At a non-Git folder, `orbit init` treats the folder as a Board anchor and stores the Board at `<anchor>/.orbit/board`; member Repositories are discovered below it, and `orbit init` merges any child Boards it finds into the new Board, keeping the children on disk as backups. It derives the Issue-ID slug from the repository or anchor folder and prints a numbered setup report: the Board path, branch, layout, Issue-ID prefix, and Git exclusion; the coding agents it wired, the MCP command, and every file it created or updated; then `orbit doctor` and the next commands, each with a short explanation. In a terminal it asks whether to wire the coding-agent harnesses it detects; without a terminal it writes no agent files and explains how to wire them. Pass `--agents` to decide up front, and `--print` to see the same report in future tense without writing anything.

```bash
orbit init                    # repo ./.orbit/board, or anchor ./.orbit/board
orbit init .                  # same as plain init inside a repository
orbit init --agents all       # wire every supported harness
orbit init --agents none      # Board only, no agent files
orbit init --print            # preview without writing
orbit init ~/src/acme         # anchor Board at ~/src/acme/.orbit/board
(cd ~ && orbit init)         # ordinary Board at ~/.orbit/board
orbit init --visible=tracker  # visible Board folder instead of .orbit/board
```

**Core `init` parameters**

| Parameter | Use it when | Example |
| --- | --- | --- |
| *none* | Start a project-local Board in Git, or an anchor Board outside Git. | `orbit init` |
| `DIR` | Name a Board anchor, an existing repository root (uses its `./.orbit`), or an explicit Board directory. An anchor stores its Board at `<DIR>/.orbit/board`. | `orbit init ~/src/acme` |
| `--slug SLUG` | Choose the Issue-ID prefix instead of deriving it from `DIR`. Explicit input is strict; a populated Board returns `SLUG_LOCKED`. | `orbit init --slug acme` |
| `--agents LIST` | Wire agents without a prompt: `none`, `all`, `auto`, or a comma-separated client list. | `orbit init --agents all` |
| `--print` | Preview the Board, exclusion entry, and generated agent files without writing or prompting. | `orbit init DIR --print` |
| `--visible[=NAME]` | Use a visible Board folder (default name `issues`) instead of the hidden `.orbit/board` layout when the target is a repository or anchor root. | `orbit init --visible=tracker` |
| `--host[=REPO\|URL]` | Set the Board host — a member Repository name/path or git URL whose remote stores the Board's `orbit` branch. Bare `--host` prompts in a terminal. | `orbit init --host=../board-host` |

**Choose by scenario**

- **A normal product repository:** run `orbit init` at the repository root. The Board is `./.orbit/board`, shared by every linked worktree, and IDs use the repository folder name.
- **A multi-repository Board:** run `orbit init ~/src/acme` or `orbit init` at its non-Git root. Repositories inside discover `<anchor>/.orbit/board` automatically; repositories outside it can join with `orbit repo add`.
- **A one-off or CI setup:** use `--print` first, then `--agents none` when the machine should not receive editor configuration. Pass `--slug` when IDs must be predictable.
- **A home Board:** run `orbit init` in `~` to create an ordinary Board there. Normal discovery finds it; discovery never creates it. There is no `--global` flag.
- **Joining an existing Board:** use `orbit repo add DIR` from the repository that should share the Board; do not run `init` again. A second local Board in the same working tree is refused.
- **Moving a supported legacy storage layout:** run `orbit migrate`; `init` is not a migration command. It converts pre-ADR-0019 Boards — worktree-mode Boards, in-repo layouts, and child Boards — into standalone Board folders in one commit.

**Supporting `init` options**

| Parameter | Use it when | Example |
| --- | --- | --- |
| `--dir PATH` / `ORBIT_DIR` | Override discovery with a Board anchor or explicit Board directory. Positional `DIR` wins for `init`. | `orbit --dir ~/src/acme init` |
| `--no-agents` | You want the same result as `--agents none` without spelling the selector. | `orbit init --no-agents` |
| `--hooks` | You also want the Claude or Devin SessionStart hook. | `orbit init --agents claude --hooks` |
| `--portable-mcp` | Generated MCP entries should call bare `orbit` instead of an absolute binary path. | `orbit init --portable-mcp` |
| `--mcp-bin PATH` | You need to record a specific Orbit executable for MCP; conflicts with `--portable-mcp`. | `orbit init --mcp-bin /usr/local/bin/orbit` |

## 3. Put real work on the board

```bash
orbit add "Spec: SSO login across api, web, infra" \
  --type epic -p 1 --label ready-for-agent --body-file spec.md

orbit add "api: OIDC callback" \
  --parent orb-a7b2c9 -p 1 --label ready-for-agent

orbit add "web: login redirect" \
  --parent orb-a7b2c9 -p 2 --label ready-for-agent

orbit add "infra: register OIDC client" \
  --parent orb-a7b2c9 -p 0 --label ready-for-agent
```

A spec or epic is a top-level Issue with `type: epic`; the tickets under it are **sub-issues**. Each sub-issue is claimed and completed independently, and the parent becomes Ready only when every child is done or cancelled.

## 4. Let an agent work

```bash
# Take exactly one issue, atomically. The envelope includes the issue and lease.
orbit ready --claim --by claude --json

# Load the body, acceptance criteria, dependencies, and comments.
orbit show orb-a7b2c9.z21c2w --json

# Extend the lease during a long build or test run.
orbit heartbeat orb-a7b2c9.z21c2w --by claude

# Record a checkpoint before pausing, releasing, or finishing the lease.
orbit checkpoint orb-a7b2c9.z21c2w --by claude \
  --progress "OIDC callback implemented; one regression open" \
  --next "Fix the round-trip test" --blocker "Waiting on review"

# Close the issue and record the note in the same transaction.
orbit done orb-a7b2c9.z21c2w --by claude --comment "Implemented and verified; tests added."
```

That is the entire loop. There is no daemon to start, no database to migrate, and no background worker to supervise.

## 5. See the board

`orbit list` is a fast, human-readable view of the graph. `--group` keeps each Parent Issue and its sub-issues together in one outlined table:

```text
$ orbit list --group
╭───────────────────┬────────┬────┬──────┬───────┬─────────────────────────────┬─────────────────────────────┬────────────╮
│ ID                │ STATUS │ P  │ TYPE │ ISSUE │ TITLE                       │ LABELS                      │ CLAIMED BY │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9  Spec: SSO login across api, web, infra                                   epic · ready · P1 · ready-for-agent │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9.z21c2w │ ready  │ P0 │ issue │ #12   │ infra: register OIDC client │ ready-for-agent │            │
│ orb-a7b2c9.hxdmqt │ ready  │ P1 │ issue │ -     │ api: OIDC callback          │ ready-for-agent   │            │
│ orb-a7b2c9.c5qmed │ ready  │ P2 │ issue │ -     │ web: login redirect         │ ready-for-agent   │            │
╰───────────────────┴────────┴────┴──────┴───────┴─────────────────────────────┴─────────────────────────────┴────────────╯
```

Columns are `ID`, `STATUS`, `P` (priority), `TYPE`, `TITLE`, `LABELS`, and `CLAIMED BY`, plus an `ISSUE` column when the Board has remote material (a configured Remote Project or any Issue link): `ISSUE` is `-` for a Local Issue, `#N` for a Linked Issue, and `~#N` for detached provenance; `--linked` and `--local` filter on it and appear in `--help` only then. Root Issues are ordered by priority (then creation time and ID), and each Parent Issue is followed by its sub-issues, also ordered by priority; `--group` draws that hierarchy and puts issues without a parent in a final `No parent` section. Long titles are shortened to 60 characters in the table; `--json` always returns the full title and every field.

For one feature, `orbit tree` shows the parent, its children, progress, and blockers:

```text
$ orbit tree orb-a7b2c9
orb-a7b2c9  ready  P1  epic  Spec: SSO login across api, web, infra  [ready-for-agent]  0/3 resolved
  orb-a7b2c9.c5qmed  ready  P2  web: login redirect  [ready-for-agent]
  orb-a7b2c9.hxdmqt  ready  P1  api: OIDC callback  [ready-for-agent]
  orb-a7b2c9.z21c2w  ready  P0  infra: register OIDC client  [ready-for-agent]
```

For search, `orbit search` mixes free text with `key:value` qualifiers — `status:`, `assignee:`, `label:`, `type:`, `priority:`, `component:`, `repository:` — and returns a `snippet` per hit:

```text
$ orbit search "status:ready label:backend login"
orb-a7b2c9.hxdmqt  ready  P1  issue  api: OIDC callback  ... login flow ...
```

`orbit list` and `orbit search` paginate with a stable cursor — feed `data.next_cursor` back as `--cursor` and page through without repeats or skips while the Board changes. Both also accept `--filter` (a JSON object of summary-field predicates) and `--fields` (a comma-separated projection, dotted paths allowed); `show --fields` projects the detail object the same way. `orbit digest <id>` is the compact per-Issue read; on an epic or parent it adds a `rollup` with `done`/`cancelled`/`blocked`/`open` counts over the subtree.

### Board view

`orbit board view` serves a kanban of the Board in your browser while the command runs, with two layouts: Columns (drag a card to move it, or walk the grid with the arrow keys) and Queue (a prioritized list with a detail pane). The keyboard covers everything: `/` focuses the filter, `j`/`k` or ↑/↓ move between Issues, ←/→ move between columns in Columns, Enter opens one, `m` opens its move menu, Esc closes, and `?` shows the full shortcut sheet. Every move runs the same `orbit` command you would type, with `--if-version`, so an agent that changed the Issue first wins. Stop it with Ctrl-C.

```bash
orbit board view            # opens http://127.0.0.1:<port>/#token=...
orbit board view --no-open --port 8080 --by maria
```

Security: the server listens on `127.0.0.1` only, and every API call needs the per-run token from the printed URL, so other web pages and sites cannot read or move your Board. The token in the printed URL is the only credential; `--no-open` keeps it out of the browser-launch command line. See [ADR 0023](../../docs/adr/0023-board-view-served-by-a-foreground-local-server.md).

Verify Board health, its branch, and uncommitted changes. If a Claude SessionStart hook is configured, `doctor` also warns when `orbit` is not the binary on `PATH`. From a Board anchor or member Repository it also names the resolved Board, checks Repository discovery and cached Issue freshness, and reports migration remnants, sync conflicts, and agent setup drift with a safe next action:

```bash
orbit doctor
```
