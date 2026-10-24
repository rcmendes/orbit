# Orbit: an AI-native issue tracker

**Status:** pre-alpha. v0.2.0 is the latest release. M0 (core CLI), M1 (agent setup), M2 (MCP), M3 (release tooling), and M4 (labels, comments, triage) are implemented. The canonical design is in [SPEC.md](./SPEC.md); vocabulary is in [GLOSSARY.md](./GLOSSARY.md). Requires Git 2.31+ (Rust 1.89+ only when building from source).

### Brief description

Orbit is a zero-daemon, AI-native local issue tracker built in Rust. Tasks are plain Markdown files on a dedicated Git branch, served through a strict `--json` CLI and an MCP server. Labels, append-only comments, atomic claims, and a single locked writer make triage and concurrent agents safe without a database server.

### The goal

Eliminate context-window bloat and pipeline freezes in multi-agent workflows. Orbit gives agents a deterministic ready queue with atomic claims, expiring leases, dependency validation, and a strict error contract — while keeping every task change human-readable and reviewable in Git.

### The pitch

When building micro-SaaS products at maximum velocity, your autonomous toolchain — Cursor, Claude Code, Codex, OpenCode, Superpowers — needs a central nervous system. Standard issue trackers force LLMs to burn thousands of tokens parsing flat files or opaque databases, and database-backed trackers put task state in a binary that code review cannot see.

Orbit keeps the board as Markdown, but compiles it into an in-process graph on every command. Agents ask `orbit ready --claim` for exactly one piece of work, hold a time-bounded lease, and get structured errors when a dependency is unfinished or another agent already owns the task. Writes serialize on one repo-wide lock and publish through temp-file plus rename, so a crash cannot truncate a task and two agents cannot corrupt the queue. Installation is a single binary, with shell/PowerShell installers and an npm wrapper. There is no daemon, no watcher, and no Dolt.

### Architecture

Commands are short-lived processes. A board command resolves one Board, takes a shared or exclusive lock, rebuilds the task graph from Markdown, applies one transition, publishes changed files atomically, and optionally commits to the orphan `orbit` branch. The MCP server is a bridge over the same CLI, so it reuses the same lock, write path, and error contract.

```mermaid
flowchart LR
    subgraph Clients["Clients"]
        DEV["Developer"]
        AGENT["Coding agent"]
        MCPC["MCP client"]
    end

    subgraph Binary["orbit binary"]
        ROUTER["CLI router<br/>clap"]
        MCP["MCP stdio server"]
        CMD["command handlers"]
        GRAPH["in-memory task graph"]
        LOCK["board lock"]
        ATOMIC["atomic writer"]
        GIT["Git adapter"]
        SETUP["setup recipes"]
    end

    subgraph State["Board state"]
        BOARD["Board<br/>hidden worktree or shared path"]
        TASKS["tasks/ID.md"]
        COMMENTS["comments/ID.md"]
        CONFIG["config.toml"]
        BRANCH["orphan branch: orbit"]
    end

    subgraph Harness["Harness files"]
        FILES["CLAUDE.md · AGENTS.md<br/>.mcp.json · opencode.json"]
        HOOK["SessionStart hook"]
    end

    DEV --> ROUTER
    AGENT --> ROUTER
    AGENT --> HOOK
    HOOK -->|prime --hook-json| ROUTER
    MCPC --> MCP
    MCP -->|spawns CLI per call| ROUTER
    ROUTER --> CMD
    CMD --> GRAPH
    GRAPH -->|reads| TASKS
    CMD --> LOCK
    CMD --> ATOMIC
    ATOMIC -->|writes| TASKS
    ATOMIC -->|writes| COMMENTS
    CMD --> GIT
    GIT --> BRANCH
    BOARD --- TASKS
    BOARD --- COMMENTS
    BOARD --- CONFIG
    BRANCH --- BOARD
    CMD --> SETUP
    SETUP --> FILES
    SETUP --> HOOK
```

The full component map, board-resolution flow, domain model, state machine, readiness gate, command map, and sequence diagrams for `init`, the task lifecycle, concurrent claims, and MCP live in [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md).

### Quick start

```bash
# Install once (see below), then in your repository:
orbit init
orbit add "Build the CLI router" -p 1
orbit ready --claim --by claude
orbit heartbeat orb-a7b2c9 --by claude
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified"
```

`orbit init` creates the Board and wires the coding-agent harnesses it detects. In a terminal it shows what it found and asks which ones to wire; without a terminal it wires the detected ones and never prompts.

Board commands accept `--json`, and `orbit <command> --help` explains its semantics with examples. [docs/commands.md](./docs/commands.md) is the full command guide, including when to use each command and end-to-end scenarios.

### Install

Prebuilt binaries are the primary path. v0.2.0 is the latest release.

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.sh | sh

# Windows PowerShell
irm https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.ps1 | iex

# npm / npx
npx @rcmendes/orbit --version
npm install -g @rcmendes/orbit
```

The installer adds its directory to `PATH` unless it is already there, and tells you what to source if a restart is needed. The npm package is scoped because the bare name is taken; the command is always `orbit`.

From source, with Rust 1.89 or newer:

```bash
cargo install --path .
```

See [RELEASING.md](./RELEASING.md) for the release process.

### Agent setup

`orbit init` is the one in-repository command. It creates the Board, detects installed harnesses from their commands and project markers, and writes only the integrations that apply. Every file is written with a managed marker, so re-running is safe and `--remove` deletes only what Orbit added.

```bash
orbit init                    # Board + detected harnesses (asks in a terminal)
orbit init --agents all       # wire every supported harness
orbit init --agents none      # Board only; no agent files
orbit init --print            # preview without writing
orbit init --board ~/board    # create a shared Board and link this repository
```

Use `orbit setup` to repair or adjust one harness later:

```bash
orbit setup claude --hooks   # CLAUDE.md + SessionStart hook + MCP
orbit setup cursor           # .cursor/rules/orbit.mdc + skill + MCP
orbit setup codex            # AGENTS.md + skill + MCP
orbit setup opencode         # AGENTS.md + MCP
orbit setup mattpocock       # docs/agents/issue-tracker.md
orbit setup all --check      # installed / missing / stale
orbit setup all --no-mcp     # instruction files only
orbit setup all --remove     # remove only Orbit-managed content
```

A hand-written tracker guide is respected. If `docs/agents/issue-tracker.md` exists without Orbit's managed marker, `orbit setup mattpocock` reports it as `skipped` and leaves it untouched; `--check` treats the custom doc as installed, and `--remove` does not delete it. Tracker files Orbit wrote continue to update and remove normally.

MCP configuration is written by default for Claude, Cursor, Codex, and OpenCode. Entries record the absolute path of the running binary, so editors launched outside a shell do not need `PATH`. The Claude SessionStart hook records the same absolute binary and runs `orbit prime --hook-json --by claude` at session start. Use `--mcp-bin orbit` if you want a portable command that relies on the editor's `PATH`.

`orbit init` and `orbit setup` refuse to record a binary inside npm's prunable `_npx` cache or a temporary directory. Install globally, or pass `--mcp-bin orbit`.

Because the generated MCP command is machine-specific, keep those config files untracked when possible. If one is committed, regenerate it after installing Orbit elsewhere, or use `--mcp-bin orbit`.

### Several related repositories

A Workspace is one Board plus every repository linked to it. Create the Board once, then link the repositories that share it:

```bash
# In the first repository: create the product Board and link to it
orbit init --board ~/src/acme-board

# In each related repository: use the same Board
orbit link ~/src/acme-board
```

Commands in a linked repository find the shared Board without an environment variable, including from Git worktrees. `orbit unlink` returns a repository to its own Board. An explicit override wins for one command:

```bash
orbit --dir ~/src/acme-board list
ORBIT_DIR=~/src/acme-board orbit ready
```

Discovery order is `--dir`, then `ORBIT_DIR`, then the repository link, then the repository's own Board.

### Triage metadata

```bash
orbit label add <id> bug needs-triage
orbit label remove <id> needs-triage
orbit label list
orbit list --label ready-for-agent
orbit list --unlabeled

orbit comment add <id> --by opencode --body "Triage notes..."
orbit comment list <id>
```

The default hold labels are `needs-triage`, `needs-info`, and `ready-for-human`: tasks carrying one are excluded from `ready` and cannot be claimed until the label is removed. `ready-for-agent`, `bug`, and `enhancement` are neutral. Comments are append-only and stored per task under `comments/`.

### MCP

```bash
orbit mcp
```

`orbit mcp` speaks JSON-RPC 2.0 over stdio and exposes 13 tools: `ready`, `list`, `get`, `create`, `update`, `claim`, `release`, `heartbeat`, `finish`, `link`, `delete`, `label`, `comment`. It also serves `orbit://tasks/{id}` as a Markdown resource. Tool calls reuse the CLI's lock, atomic writes, auto-commit, and error envelope. See [SPEC.md](./SPEC.md) section 9.

### Why not Beads or Beans?

- **Beans** proved that Markdown-as-truth is pleasant for humans, but it has no atomic claims, leases, or dependency-aware ready queue.
- **Beads** proved that agents need claims, ready-work detection, priorities, and hash IDs, but its source of truth is a Dolt database and its multi-writer mode needs a server.

Orbit is the narrow overlap: Beans' human-readable storage with Beads' queue semantics, in one static Rust binary with no daemon.

For several related repositories, Orbit links each one to the same Board instead of copying Tasks into a hub, so cross-repository Dependencies share one graph. Beads hydrates copies into a central database; Beans has no multi-repository model. See [ADR 0006](./docs/adr/0006-shared-board-across-repositories.md).
