<div align="center">

# 🪐 Orbit

### The Git-native issue tracker for humans and coding agents.

**One binary. One Markdown board. Zero daemons.**

[![CI](https://github.com/rcmendes/orbit/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/rcmendes/orbit/actions/workflows/ci.yml)
[![Release](https://github.com/rcmendes/orbit/actions/workflows/release.yml/badge.svg)](https://github.com/rcmendes/orbit/actions/workflows/release.yml)
[![npm](https://img.shields.io/npm/v/@rcmendes/orbit?logo=npm)](https://www.npmjs.com/package/@rcmendes/orbit)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![Status](https://img.shields.io/badge/status-pre--alpha-8a2be2)](https://github.com/rcmendes/orbit/releases)

[Quick start](#quick-start) · [Why Orbit](#why-orbit) · [How it works](#how-it-works) · [Command map](#command-map) · [Documentation](#documentation)

</div>

---

**Orbit is a local issue tracker whose tasks are Markdown files in your Git repository.** Every task and comment lives on a dedicated orphan branch, so people and agents work from the same reviewable source of truth. Mutations are serialized by one lock and published with `fsync` + rename; meaningful changes are committed to Git. There is no database, daemon, server, or context-window tax.

Claude Code, Cursor, Codex, OpenCode, and any shell-capable agent get a deterministic, dependency-aware ready queue through a strict JSON CLI or an MCP server. Every claim is atomic, so two agents never take the same task. Humans get readable Git diffs and exactly the same board.

**1 binary · 0 daemons · 4 statuses · 14 MCP tools · 100% Markdown tasks**

```text
$ orbit ready --claim --by claude
claimed orb-a7b2c9 by claude until 2026-10-24T22:30:00Z
```

> **Status:** pre-alpha. [`v0.9.4`](https://github.com/rcmendes/orbit/releases/tag/v0.9.4) is the latest release. It includes the GitHub Issue provider (`orbit github`), Triage review (`orbit triage`), and the Matt Pocock compatibility hardening. GitHub sync requires Orbit v0.9.0 or newer. The **Orbit folder** design from v0.8.1 is unchanged: one delegated `DIR` (default `.orbit`), a single `orbit init [DIR]`, scoped path-free agent setup, and a resumable legacy migration. The canonical design is [SPEC.md](./docs/SPEC.md) and vocabulary is [GLOSSARY.md](./docs/GLOSSARY.md). Orbit requires Git 2.31+ (Rust 1.89+ only when building from source).

<details>
<summary><strong>Contents</strong></summary>

- [Why Orbit](#why-orbit)
- [Quick start](#quick-start)
- [Core concepts](#core-concepts)
- [How it works](#how-it-works)
- [The agent loop](#the-agent-loop)
- [Features and SubTasks](#features-and-subtasks)
- [Task-ID slugs](#task-id-slugs)
- [Multi-repository workspaces](#multi-repository-workspaces)
- [Triage, labels, and comments](#triage-labels-and-comments)
- [Two front doors: JSON CLI and MCP](#two-front-doors-json-cli-and-mcp)
- [Export the board](#export-the-board)
- [Agent setup](#agent-setup)
- [Command map](#command-map)
- [Recovery and maintenance](#recovery-and-maintenance)
- [Why not Beads or Beans?](#why-not-beads-or-beans)
- [Compatibility vocabulary](#compatibility-vocabulary)
- [Install details](#install-details)
- [Documentation](#documentation)
- [Development](#development)
- [Contributing](#contributing)

</details>

## Why Orbit

Most trackers were designed for humans clicking through a web UI. Agents need something different: a stable contract, a small context footprint, and a queue that cannot hand the same ticket to two workers.

| The usual problem | Orbit's answer |
| --- | --- |
| Task state lives in a database nobody can review | Tasks are Markdown files on an orphan Git branch, diffable like code |
| Two agents grab the same ticket | `ready --claim` is atomic; the loser gets a typed error |
| An agent dies mid-task and the ticket is lost | Claims expire after a lease; recover with `claim --steal` |
| Agents burn tokens parsing the whole backlog | A dependency-aware ready queue returns one actionable task |
| Writes race and truncate files | One board lock, temp-file + `fsync` + rename, serialize every mutation |
| Work spans repositories but the board does not | One shared Board for a whole workspace, including every Git worktree |
| The tracker requires infrastructure to run | One binary, no daemon, no database, no hosted service |
| Status is prose an agent has to interpret | Stable JSON envelope, typed error codes, and retryability |

Orbit is deliberately narrow. It is not a project-management suite, a time tracker, or a hosted sync service. It is the smallest reliable queue that lets several coding agents work on one product without stepping on each other.

### Built for

- **Solo builders** who want a fast local backlog that lives beside the code and never needs a server.
- **Agent squads** running Claude Code, Cursor, Codex, and OpenCode in parallel against one product.
- **Multi-repository products** where the API, web app, infrastructure, and agent tooling all need one dependency graph.

## Quick start

### 1. Install

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

### 2. Create the Board

```bash
cd your-repository
orbit init
```

In a Git repository, `orbit init` creates the Board in `./.orbit`. At a non-Git Workspace root, it creates `<workspace>/.orbit/board` and reserves `<workspace>/.orbit/cache`; child repositories discover the Board without `orbit link`. It derives the Task-ID slug from the repository or Workspace folder and prints a numbered setup report: the Board path, branch, storage mode, Task-ID prefix, and Git exclusion; the coding agents it wired, the MCP command, and every file it created or updated; then `orbit doctor` and the next commands, each with a short explanation. In a terminal it asks whether to wire the coding-agent harnesses it detects; without a terminal it writes no agent files and explains how to wire them. Pass `--agents` to decide up front, and `--print` to see the same report in future tense without writing anything.

```bash
orbit init                    # repo ./.orbit, or Workspace ./.orbit/board
orbit init --agents all       # wire every supported harness
orbit init --agents none      # Board only, no agent files
orbit init --print            # preview without writing
orbit init ~/src/acme         # Workspace Board at ~/src/acme/.orbit/board
orbit init --global           # create the global Board at ~/.orbit
```

**Core `init` parameters**

| Parameter | Use it when | Example |
| --- | --- | --- |
| *none* | Start a project-local Board in Git, or a Workspace Board outside Git. | `orbit init` |
| `DIR` | Name a Workspace root or explicit Board directory. A Workspace stores its Board at `<DIR>/.orbit/board`. | `orbit init ~/src/acme` |
| `--global` | Create or reuse the opt-in global Board at `~/.orbit`. It is the last discovery fallback and is never created implicitly. Conflicts with positional `DIR` and `--dir`/`ORBIT_DIR`. | `orbit init --global` |
| `--slug SLUG` | Choose the Task-ID prefix instead of deriving it from `DIR`. Explicit input is strict; a populated Board returns `SLUG_LOCKED`. | `orbit init --slug acme` |
| `--agents LIST` | Wire agents without a prompt: `none`, `all`, `auto`, or a comma-separated client list. | `orbit init --agents all` |
| `--print` | Preview the Board, exclusion entry, and generated agent files without writing or prompting. | `orbit init DIR --print` |

**Choose by scenario**

- **A normal product repository:** run `orbit init` at the repository root. The Board is `./.orbit`, shared by every linked worktree, and IDs use the repository folder name.
- **A multi-repository Workspace:** run `orbit init ~/src/acme` or `orbit init` at its non-Git root. Repositories inside discover `<workspace>/.orbit/board` automatically; repositories outside it can `orbit link ~/src/acme`.
- **A one-off or CI setup:** use `--print` first, then `--agents none` when the machine should not receive editor configuration. Pass `--slug` when IDs must be predictable.
- **A global fallback:** use `--global` only when you intentionally want one Board available when no project-local Board resolves. Discovery does not create it.
- **Joining an existing Board:** use `orbit link DIR`; do not run `init` again. A second local Board in the same working tree is refused.
- **Moving a supported legacy storage layout:** run `orbit doctor --migrate`; `init` is not a migration command. To consolidate a legacy Workspace Board and child-repository Boards, use `orbit workspace migrate`; it creates the Workspace Board only after merged data validates when no Board exists yet.

**Supporting `init` options**

| Parameter | Use it when | Example |
| --- | --- | --- |
| `--dir PATH` / `ORBIT_DIR` | Override discovery with a Workspace root or explicit Board directory. Positional `DIR` wins for `init`. | `orbit --dir ~/src/acme init` |
| `--no-agents` | You want the same result as `--agents none` without spelling the selector. | `orbit init --no-agents` |
| `--hooks` | You also want the Claude Code SessionStart hook. | `orbit init --agents claude --hooks` |
| `--portable-mcp` | Generated MCP entries should call bare `orbit` instead of an absolute binary path. | `orbit init --portable-mcp` |
| `--mcp-bin PATH` | You need to record a specific Orbit executable for MCP; conflicts with `--portable-mcp`. | `orbit init --mcp-bin /usr/local/bin/orbit` |

### 3. Put real work on the board

```bash
orbit add "Spec: SSO login across api, web, infra" \
  --type epic -p 1 --label ready-for-agent --body-file spec.md

orbit add "api: OIDC callback" \
  --parent orb-a7b2c9 -p 1 --label repo:api --label ready-for-agent

orbit add "web: login redirect" \
  --parent orb-a7b2c9 -p 2 --label repo:web --label ready-for-agent

orbit add "infra: register OIDC client" \
  --parent orb-a7b2c9 -p 0 --label repo:infra --label ready-for-agent
```

A spec or epic is a top-level Task with `type: epic`; the tickets under it are **SubTasks**. Each SubTask is claimed and completed independently, and the parent becomes Ready only when every child is done or cancelled.

### 4. Let an agent work

```bash
# Take exactly one task, atomically. The envelope includes the task and lease.
orbit ready --claim --by claude --json

# Load the body, acceptance criteria, dependencies, and comments.
orbit show orb-a7b2c9.z21c2w --json

# Extend the lease during a long build or test run.
orbit heartbeat orb-a7b2c9.z21c2w --by claude

# Record a checkpoint before pausing, releasing, or finishing the lease.
orbit checkpoint orb-a7b2c9.z21c2w --by claude \
  --progress "OIDC callback implemented; one regression open" \
  --next "Fix the round-trip test" --blocker "Waiting on review"

# Close the task and record the note in the same transaction.
orbit done orb-a7b2c9.z21c2w --by claude --comment "Implemented and verified; tests added."
```

That is the entire loop. There is no daemon to start, no database to migrate, and no background worker to supervise.

### 5. See the board

`orbit list` is a fast, human-readable view of the graph. `--group` keeps each Parent Task and its SubTasks together in one outlined table:

```text
$ orbit list --group
╭───────────────────┬────────┬────┬──────┬─────────────────────────────┬─────────────────────────────┬────────────╮
│ ID                │ STATUS │ P  │ TYPE │ TITLE                       │ LABELS                      │ CLAIMED BY │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9  Spec: SSO login across api, web, infra                           epic · todo · P1 · ready-for-agent │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9.z21c2w │ todo   │ P0 │ task │ infra: register OIDC client │ ready-for-agent, repo:infra │            │
│ orb-a7b2c9.hxdmqt │ todo   │ P1 │ task │ api: OIDC callback          │ ready-for-agent, repo:api   │            │
│ orb-a7b2c9.c5qmed │ todo   │ P2 │ task │ web: login redirect         │ ready-for-agent, repo:web   │            │
╰───────────────────┴────────┴────┴──────┴─────────────────────────────┴─────────────────────────────┴────────────╯
```

Columns are `ID`, `STATUS`, `P` (priority), `TYPE`, `TITLE`, `LABELS`, and `CLAIMED BY`. Root Tasks are ordered by priority (then creation time and ID), and each Parent Task is followed by its SubTasks, also ordered by priority; `--group` draws that hierarchy and puts tasks without a parent in a final `No parent` section. Long titles are shortened to 60 characters in the table; `--json` always returns the full title and every field.

For one feature, `orbit tree` shows the parent, its children, progress, and blockers:

```text
$ orbit tree orb-a7b2c9
orb-a7b2c9  todo  P1  epic  Spec: SSO login across api, web, infra  [ready-for-agent]  0/3 resolved
  orb-a7b2c9.c5qmed  todo  P2  web: login redirect  [ready-for-agent, repo:web]
  orb-a7b2c9.hxdmqt  todo  P1  api: OIDC callback  [ready-for-agent, repo:api]
  orb-a7b2c9.z21c2w  todo  P0  infra: register OIDC client  [ready-for-agent, repo:infra]
```

Verify Board health, its branch, and uncommitted changes. If a Claude SessionStart hook is configured, `doctor` also warns when `orbit` is not the binary on `PATH`:

```bash
orbit doctor
```

## Core concepts

Orbit keeps the vocabulary small on purpose. Everything else is derived.

| Concept | Meaning |
| --- | --- |
| **Task** | One unit of work. A Markdown file under `tasks/`. This is what external trackers call an issue. |
| **SubTask** | A Task with a `parent`. It is claimable and completable in its own right, not a checklist item. |
| **Parent Task** | A Task that groups SubTasks. Its completion waits for its children. |
| **Kind** | Structural and derived: `task` or `subtask`. |
| **Type** | Work shape: `task` (default), `epic`, `spike`, `chore`, or `decision`. Metadata only; it never changes readiness. `epic` is top-level only. |
| **Status** | Stored lifecycle: `todo`, `in_progress`, `done`, or `cancelled`. |
| **Priority** | `P0` (highest) through `P3` (lowest), stored as `0`–`3`. |
| **Label** | A lowercase string for filtering and workflow state. |
| **Hold Label** | A label that suspends readiness. Defaults: `needs-triage`, `needs-info`, `ready-for-human`. |
| **Dependency** | Another Task that must be `done` or `cancelled` before this one is Ready. |
| **Claim** | Exclusive, time-bounded responsibility for a Task, held by one agent. |
| **Lease** | The time window during which a Claim is valid. Default: 30 minutes. |
| **Checkpoint** | The claim holder's latest handoff note: current progress, the next useful action, and any blocker. It survives release and takeover but never affects readiness. |
| **Ready** | Derived: `todo`, unclaimed, not Waiting, no Hold Label, every Dependency resolved, and every SubTask resolved. Never stored. |
| **Waiting** | An external blocker recorded in `waiting_on`. Not a Status. |
| **Comment** | An append-only, authored, timestamped note attached to a Task. |

The full definitions, including the words Orbit avoids, are in [GLOSSARY.md](./docs/GLOSSARY.md).

## How it works

Orbit is one binary with two front doors: the JSON CLI and the MCP stdio server. A command is a short-lived process. It resolves one Board, takes the right lock, rebuilds the task graph from Markdown, applies one transition, publishes changed files atomically, and optionally commits to the orphan `orbit` branch.

```mermaid
flowchart LR
    subgraph Clients["Clients"]
        DEV["Developer"]
        AGENT["Coding agent<br/>Claude Code · Cursor · Codex · OpenCode"]
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
        SETUP["agent setup recipes"]
    end

    subgraph Storage["Durable state"]
        BOARD["Board<br/>Orbit folder or shared DIR"]
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
    HOOK -->|"prime --hook-json"| ROUTER
    MCPC --> MCP
    MCP -->|"spawns CLI per call"| ROUTER
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

    classDef actor fill:#fff7ed,stroke:#c2410c,color:#431407
    classDef iface fill:#eef2ff,stroke:#4f46e5,color:#1e1b4b
    classDef app fill:#f5f3ff,stroke:#7c3aed,color:#2e1065
    classDef domain fill:#ecfeff,stroke:#0e7490,color:#083344
    classDef store fill:#f0fdf4,stroke:#15803d,color:#052e16

    class DEV,AGENT,MCPC actor
    class ROUTER,MCP iface
    class CMD,GRAPH,LOCK,ATOMIC,GIT,SETUP app
    class BOARD,TASKS,COMMENTS,CONFIG,BRANCH store
    class FILES,HOOK app
```

The full component map, board-resolution flow, domain model, state machine, readiness gate, command map, and sequence diagrams for `init`, the task lifecycle, concurrent claims, and MCP live in [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md).

### Board anatomy

The board is real Git history on an orphan branch, checked out in the Orbit folder:

```text
<project>/.orbit/                       # DIR: shared by every worktree
├── config.toml                         # branch, storage mode, lease, priorities, hold labels
├── tasks/
│   ├── orb-a7b2c9.md                   # a top-level Task
│   └── orb-a7b2c9.d4b3df.md            # a SubTask
└── comments/
    └── orb-a7b2c9.md                   # append-only thread
```

Inside a Git working tree, `.orbit` is a worktree of the project repository on the `orbit` branch; the path is written to `.git/info/exclude`, so it never appears in code diffs. Outside every working tree, a Workspace root stores its Board at `.orbit/board` as an independent Git repository and reserves `.orbit/cache` separately. Child repositories discover that Board across their Git boundary. Both modes expose the same Board files and CLI contract.

```text
<workspace>/
├── api/                         # child Git repository; no orbit link needed
└── .orbit/
    ├── board/                   # versioned Board on its own orbit branch
    └── cache/                   # separate, rebuildable catalog data
```

Tasks are Markdown with YAML frontmatter:

```yaml
---
title: Build the CLI router
status: todo
type: task
priority: 2
labels:
- ready-for-agent
depends_on:
- orb-f14c02
created_at: 2026-10-23T11:00:00Z
updated_at: 2026-10-23T12:00:00Z
---

Body text, acceptance criteria, and notes.
```

The filename is the ID. There is no `id` field in frontmatter. A SubTask ID is the parent ID, a dot, and a fresh suffix (`orb-a7b2c9.d4b3df`). One dot, one level. A claimed Task may also carry one `checkpoint` block with `by`, `at`, `progress`, and optional `next` and `blocker`; Orbit replaces it in place and it never changes readiness.

### Why an orphan branch?

Task churn does not belong in your feature PRs. The `orbit` branch is separate from `main`, so an agent can update a ticket while a human reviews code without either change polluting the other. A Board created at the main checkout's `.orbit` is resolved by every linked worktree, so a claim made from a worktree and a claim made from the main checkout serialize on the same lock and see the same tasks.

### Safety without a server

- **One writer at a time.** Mutations take a Board-wide exclusive lock; readers take it shared for a consistent snapshot.
- **Atomic publishes.** Each file is written to a temp file, fsynced, renamed, and the directory is fsynced on Unix. A crash cannot leave a half-written task.
- **No persistent index.** Every command rebuilds the graph from Markdown. There is no cache to corrupt and no watcher to miss an event.
- **Auto-commit with an escape hatch.** Meaningful mutations commit to the `orbit` branch. Heartbeats are lease-only and wait for the next mutation or `orbit sync`. Nothing is pushed.
- **Typed failures.** `LOCKED` is retryable; dependency, claim, label, and parent failures carry the IDs you need to resolve them.

## The agent loop

The common path is `add` → `ready --claim` → `heartbeat` → `done`. Each step is a separate process, and the board lock is held only for that step.

```mermaid
sequenceDiagram
    autonumber
    actor Agent
    participant CLI as orbit CLI
    participant Lock as Board lock
    participant Board as Markdown board
    participant Git as orbit branch

    Agent->>CLI: orbit ready --claim --by claude --json
    CLI->>Lock: acquire exclusive
    CLI->>Board: rehydrate graph from Markdown
    CLI->>CLI: filter Ready tasks, pick the highest priority
    CLI->>Board: atomic write in_progress + lease
    CLI->>Git: commit "orbit: claim ID"
    CLI-->>Agent: JSON envelope with task and lease

    Agent->>CLI: orbit heartbeat ID --by claude
    CLI->>Lock: acquire exclusive
    CLI->>Board: extend lease_until
    Note right of Git: heartbeat is not committed until orbit sync
    CLI-->>Agent: lease_until, meta.committed false

    Agent->>CLI: orbit checkpoint ID --by claude --progress "..." --next "..."
    CLI->>Lock: acquire exclusive
    CLI->>Board: replace checkpoint in task frontmatter
    CLI->>Git: commit "orbit: checkpoint ID"
    CLI-->>Agent: JSON detail with the checkpoint

    Agent->>CLI: orbit done ID --by claude --comment "verified"
    CLI->>Lock: acquire exclusive
    CLI->>CLI: check dependencies, SubTasks, and claim holder
    CLI->>Board: atomic write done + append comment
    CLI->>Git: commit "orbit: done ID"
    CLI-->>Agent: JSON detail with comments
```

If nothing can start, `ready --claim` is still a success:

```json
{"ok": true, "data": null, "meta": {"reason": "NO_READY_TASKS"}}
```

Polling loops should wait and retry rather than treating that as an error. When another agent holds the task, the loser gets `ALREADY_CLAIMED` with the holder and lease. When a lease expires, the task becomes **stale**, not Ready; recover it with `orbit claim <id> --steal` or return it to the queue with `orbit release <id> --force`.

A checkpoint is the structured handoff for that recovery. Only the claim holder can write one; it records progress, the next action, and any blocker. Releasing or stealing the task keeps the checkpoint, so the next claimant sees it in `orbit show`, the JSON for `orbit ready`, MCP `get`, or the claim result. On a done or cancelled task the latest checkpoint stays in the file as archived history (`checkpoint_archived: true`) but is not part of the active queue.

### The readiness gate

`ready` is computed on every command. It is never a stored flag.

```mermaid
flowchart TD
    TASK["Task candidate"] --> STATUS{"status is todo?"}
    STATUS -->|no| NOTREADY["not ready"]
    STATUS -->|yes| CLAIMED{"claimed_by is null?"}
    CLAIMED -->|no| NOTREADY
    CLAIMED -->|yes| WAITING{"waiting_on is null?"}
    WAITING -->|no| NOTREADY
    WAITING -->|yes| HELD{"any hold label?"}
    HELD -->|yes| NOTREADY
    HELD -->|no| DEPS{"every dependency resolved?"}
    DEPS -->|no| BLOCKED["blocked by dependencies"]
    DEPS -->|yes| SUBTASKS{"every SubTask resolved?"}
    SUBTASKS -->|no| SUBTASKSOPEN["open SubTasks"]
    SUBTASKS -->|yes| READY["ready<br/>eligible for ready and claim"]

    classDef no fill:#fef2f2,stroke:#b91c1c,color:#450a0a
    classDef yes fill:#f0fdf4,stroke:#15803d,color:#052e16
    classDef gate fill:#eff6ff,stroke:#1d4ed8,color:#172554
    class NOTREADY,BLOCKED,SUBTASKSOPEN no
    class READY yes
    class STATUS,CLAIMED,WAITING,HELD,DEPS,SUBTASKS gate
```

The failure codes follow the gate: `WAITING_EXTERNAL` for `waiting_on`, `LABEL_BLOCKED` for a hold label, `DEPENDENCY_BLOCKED` for unfinished dependencies, `SUBTASKS_OPEN` for open children, `ALREADY_CLAIMED` for a live claim, and `CLAIM_STALE` for an expired one. `--steal` bypasses only the claim checks, never the gate.

### Status vs. derived state

Only four values are stored in `status`:

```mermaid
stateDiagram-v2
    direction LR

    [*] --> todo: add

    todo --> in_progress: claim or ready --claim
    todo --> done: done
    todo --> cancelled: cancel

    in_progress --> todo: release
    in_progress --> in_progress: heartbeat
    in_progress --> in_progress: checkpoint
    in_progress --> in_progress: claim --steal
    in_progress --> done: done
    in_progress --> cancelled: cancel

    done --> todo: reopen
    cancelled --> todo: reopen

    done --> done: done --comment
    cancelled --> cancelled: cancel --comment
```

Ready, blocked, waiting, stale, and held are derived from claims, dependencies, labels, and `waiting_on`. They cannot be written as a Status. `done` and `cancelled` both resolve dependents; `delete` is not a transition, it removes the task file.

## Features and SubTasks

A Parent Task groups SubTasks. The parent waits for the family; the children do not wait for each other unless you add a real dependency.

```mermaid
flowchart TD
    EPIC["Parent Task · type: epic<br/>Spec: SSO login across api, web, infra"]
    API["SubTask<br/>api: OIDC callback<br/>repo:api"]
    WEB["SubTask<br/>web: login redirect<br/>repo:web"]
    INFRA["SubTask<br/>infra: register OIDC client<br/>repo:infra"]
    GATE{{"Parent becomes Ready only when every SubTask is done or cancelled"}}

    EPIC --> API
    EPIC --> WEB
    EPIC --> INFRA
    API -.-> GATE
    WEB -.-> GATE
    INFRA -.-> GATE

    classDef parent fill:#f5f3ff,stroke:#7c3aed,color:#2e1065
    classDef child fill:#ecfeff,stroke:#0e7490,color:#083344
    classDef gate fill:#fef3c7,stroke:#b45309,color:#451a03
    class EPIC parent
    class API,WEB,INFRA child
    class GATE gate
```

```bash
# Create the parent once.
orbit add "SSO login across api, web, infra" --type epic --body-file feature.md
# created orb-a7b2c9

# One SubTask per piece of work.
orbit add "api: OIDC callback" --parent orb-a7b2c9 --label repo:api
orbit add "web: login redirect" --parent orb-a7b2c9 --label repo:web
orbit add "infra: register OIDC client" --parent orb-a7b2c9 --label repo:infra

# Scope an agent to one feature or one repository.
orbit ready --parent orb-a7b2c9 --claim --by api-agent
orbit ready --repo api --claim --by api-agent

# Inspect the family and its blockers.
orbit tree orb-a7b2c9
orbit list --parent orb-a7b2c9 --json

# Close the parent explicitly when the family is finished.
orbit done orb-a7b2c9 --by coordinator --comment "SSO shipped"
```

SubTasks are one level deep. If a ticket needs its own breakdown, flatten it into sibling tickets with `--depends-on` instead of nesting further.

> **Warning:** `orbit done --force` or `orbit cancel --force` on a parent with open SubTasks cancels every open child. Use it when the remaining pieces are genuinely abandoned, not as a shortcut.

`orbit prime` surfaces **close candidates**: non-terminal Parent Tasks with at least one SubTask and no open children. Orbit never closes them automatically, so a coordinator decides when the feature is actually done.

## Task-ID slugs

A Board has one slug that prefixes its Task IDs. `orbit init` derives it from the repository or Workspace root name; `orbit init --global` uses `orb` with a warning. Pass `--slug` to choose it explicitly:

```bash
orbit init --slug teramigo
orbit add "First task" --json
# teramigo-32fdfd
```

Re-running `init` preserves the stored prefix. An empty Board can change it with `--slug`; a populated Board returns `SLUG_LOCKED`. Existing `orb-` Boards keep working unchanged; migration is optional.

To change the identity of an existing Board, use the explicit migration path instead of hand-editing `config.toml`:

```bash
orbit slug show                       # current slug, prefix, and counts
orbit slug migrate atlas --dry-run    # counts, references, and affected claims
orbit slug migrate atlas              # rewrite, validate, and commit once
```

Migration keeps each random suffix (`orb-32fdfd` → `atlas-32fdfd`, `orb-32fdfd.7fk2m9` → `atlas-32fdfd.7fk2m9`), rewrites `parent` and `depends_on`, renames comment files and their Orbit header, and updates `id.prefix`. It refuses an unhealthy Board or a prefix collision, and live claims require `--force` so the affected agents know to re-read. Task bodies, checkpoint text, comment text, Git history, and external links are not rewritten.

## Multi-repository workspaces

A **Workspace** is an ordinary, non-Git folder containing repositories and one Board. Child repositories discover the Board by walking above their Git boundary; repositories outside the Workspace can link explicitly. Cross-repository dependencies share one graph, and every Git worktree resolves the same Board.

```mermaid
flowchart LR
    subgraph Workspace["Shared workspace · ~/src/acme"]
        BOARD[("Workspace Board<br/>~/src/acme/.orbit/board")]
        CACHE["catalog cache<br/>~/src/acme/.orbit/cache"]
        API["api repository"]
        WEB["web repository"]
        INFRA["infra repository"]
    end

    subgraph Standalone["Standalone repository"]
        SOLO["solo repository"]
        LOCAL[("local Board<br/>.orbit")]
    end

    API -.->|discover across Git boundary| BOARD
    WEB -.->|discover across Git boundary| BOARD
    INFRA -.->|discover across Git boundary| BOARD
    SOLO --> LOCAL

    classDef board fill:#f0fdf4,stroke:#15803d,color:#052e16
    classDef repo fill:#fdf2f8,stroke:#be185d,color:#500724
    class BOARD,LOCAL board
    class API,WEB,INFRA,SOLO repo
```

```bash
# Initialize the Workspace root and its internal Board.
mkdir -p ~/src/acme && cd ~/src/acme
orbit init                     # Board at ~/src/acme/.orbit/board

# From a child repository, use it without a per-repository link.
cd ~/src/acme/api
orbit list

# An outside repository can join the Workspace explicitly.
orbit link ~/src/acme
orbit --dir ~/src/acme list

# Return this repository to its own Board.
orbit unlink
```

`orbit init` only creates the Workspace Board; it does not clone repositories. A child-local Board is reported as `AMBIGUOUS_BOARD` during discovery and blocks Workspace initialization if it exists first. Once the Workspace Board exists, `init` reuses it while child Boards remain as backups. Unknown entries in `.orbit` prevent initialization rather than being overwritten; the `workspace-migration.lock` entry is managed by the migration command. The catalog cache is never stored in the Board repository.

If the Workspace already has a legacy `<workspace>/.board` or `<workspace>/board`
Board, or its child repositories have local Boards, consolidate their work with
`orbit workspace migrate`. The command can stage a Workspace Board when none
exists and activates it only after validation, so child Boards are not shadowed
early. If no Workspace Board exists, the command uses the Workspace folder's
derived slug and standard Board defaults. Preview before applying, resolve
duplicate GitHub Issue identities explicitly, and keep source Boards as backups:

```bash
orbit workspace migrate --dry-run
orbit workspace migrate
orbit doctor
```

The migration preserves Tasks, comments, claims, Checkpoints, and structured
relationships. It remaps colliding Task IDs and references, records differing
source ID prefixes as accepted legacy prefixes without changing the Workspace's
primary prefix, retains Workspace configuration when source settings differ,
and records the migration as one Board commit. A saved plan makes an interrupted apply resumable with
`orbit workspace migrate --resume`; the apply report gives `git revert` rollback
guidance.

Board discovery is: `--dir`/`ORBIT_DIR`, then the repository link, then the nearest `.orbit` walking up to the repository root (or to `$HOME` outside one, skipping `~/.orbit`), then the main checkout's `.orbit`, then the legacy hidden layout with a warning, then `~/.orbit` if it exists. An explicit override wins for one command:

```bash
ORBIT_DIR=~/src/acme/.orbit orbit ready --json
```

### GitHub Issues (optional)

Boards can link GitHub repositories and map Tasks to Issues by repository and issue number. Sync is explicit, detects field-level conflicts, and keeps Orbit claims separate from GitHub assignees. See [docs/github-sync.md](./docs/github-sync.md).

GitHub sync and Triage require Orbit **v0.9.0 or newer**. The v0.9.0 default
release binaries, shell/PowerShell installers, and npm package include both
commands. For an organization-owned repository with Issue Fields ready, start
with a fresh Board and the GitHub CLI authenticated:

```bash
orbit init
gh auth login
orbit github connect
orbit github status
orbit add "First GitHub task" -p 1
# Copy the Task ID printed by `orbit add` and replace orb-a7b2c9 below.
orbit github create orb-a7b2c9
orbit github sync orb-a7b2c9 # preview only
orbit github sync orb-a7b2c9 --apply
```

For a personal-account `origin`, `connect` reports the exact `[github]` entries
to add or merge into the Board's `config.toml`, without changing the Board.
Preserve other linked repositories. Set `mode = "basic"` and include the
repository name as described below before creating or syncing Issues.

`orbit github connect` checks `gh` authentication and access to the current checkout's GitHub `origin`. For an organization-owned repository, it validates the organization Issue Fields and writes the Board configuration. For a personal-account repository, it leaves configuration unchanged and reports the exact basic-mode `[github]` entries to add or merge into the Board's `config.toml`, preserving other linked repositories. Use `orbit github connect --json` for machine-readable status, including the same guidance in `remediation` and `next_actions`.

Run `orbit github status` for a read-only diagnostic of the authenticated account and scopes, linked repositories, applicable Issue Fields, linked Task count, pending sync changes, conflicts, and unsupported relationships. `orbit doctor` includes GitHub configuration checks when GitHub is configured on the Board.

```bash
# Personal repository: [github] mode = "basic" + repositories in config.toml.
orbit github repo link alice/widget
orbit github create orb-a7b2c9 --repo alice/widget

# Connect an organization-owned repository from its checkout:
orbit github connect
orbit github status --json

# Or configure organization mode manually:
orbit github repo link acme/widget
orbit github setup
orbit github setup --create-fields # create missing fields (organization admin required)
orbit github adopt orb-a7b2c9 https://github.com/acme/widget/issues/42
orbit github adopt orb-a7b2c9 --repo acme/widget --number 42
orbit github create orb-a7b2c9 # automatically uses the only linked repository
orbit github sync orb-a7b2c9 --apply
```

`orbit github setup` does not create fields unless `--create-fields` is passed.
See [the GitHub sync guide](./docs/github-sync.md) for required permissions and
the fields Orbit creates.

For personal-account repositories, `connect` reports the exact manual basic-mode
settings rather than changing the Board. Basic mode syncs titles, bodies,
labels, and open/closed state without Issue Fields. `area` and `verification`
sync and structured Triage require full mode; basic-mode Triage supports labels
only. See [docs/github-sync.md](./docs/github-sync.md) for limitations and
upgrade steps.

## Triage, labels, and comments

Labels are free-form lowercase slugs. Three of them are **hold labels**: while present, the task is not Ready and cannot be claimed.

```bash
# A new report arrives.
orbit add "Login rejects valid tokens" -p 1 \
  --label bug --label needs-triage --body-file report.md

# Triage it into the agent queue.
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage

# Record the reasoning without rewriting the body.
orbit comment add orb-a7b2c9 --by rodrigo \
  --body "Confirmed on macOS. Add a regression test before closing."

# Hand off a long command's output.
cargo test 2>&1 | orbit comment add orb-a7b2c9 --by claude --body-file -
```

- **Hold labels:** `needs-triage`, `needs-info`, `ready-for-human` (configurable in `config.toml`).
- **Neutral labels:** `ready-for-agent`, `bug`, `enhancement`.
- **Comments** are append-only and immutable. A correction is a new comment; the thread is merged into `show`.
- **Waiting** is an external blocker: `orbit add "..." --waiting-on "upstream 2.4"`, cleared later with `orbit update <id> --clear-waiting`.
- **Dependencies** are real blockers: `--depends-on` at creation or `orbit depend <id> <dep-id>` later.
- **`wontfix`** is a label plus a terminal status: add the label and `orbit cancel <id> --by <agent>`.

Triage is what keeps the agent queue clean without hiding unfinished work from humans. `orbit list --label needs-triage` is the inbox; `orbit ready` is the frontier.

## Two front doors: JSON CLI and MCP

Every board command prints human-readable text by default. Add `--json` and stdout becomes exactly one JSON object, including errors.

```bash
orbit ready --claim --by claude --json
orbit show orb-a7b2c9 --json
orbit list --all --json
```

```json
{
  "ok": true,
  "data": {
    "id": "orb-a7b2c9",
    "title": "Build the CLI router",
    "status": "in_progress",
    "priority": 2,
    "ready": false,
    "claimed_by": "claude"
  },
  "meta": {}
}
```

Errors are just as structured:

```json
{
  "ok": false,
  "error": {
    "code": "DEPENDENCY_BLOCKED",
    "message": "Cannot complete orb-a7b2c9",
    "retryable": false,
    "details": { "blocking": ["orb-f14c02"] }
  }
}
```

Exit codes are `0` for success, `1` for a domain failure, and `2` for a usage error. `LOCKED` is the one retryable code: another writer holds the lock, so retry after a short backoff.

### MCP

`orbit mcp` speaks JSON-RPC 2.0 over stdio and exposes 14 tools:

`ready` · `list` · `get` · `create` · `update` · `claim` · `release` · `heartbeat` · `checkpoint` · `finish` · `link` · `delete` · `label` · `comment`

```bash
orbit mcp
orbit --dir ~/src/acme mcp         # serve a Workspace Board
```

MCP is a bridge, not a second implementation. Each tool call spawns the installed `orbit` binary, so it reuses the same lock, atomic writes, auto-commit, and error envelope. It also serves `orbit://tasks/{id}` as a Markdown resource. `create` accepts `parent` and `type`; `update` accepts `type`; `checkpoint` records the active handoff state on a claimed task; `list` and `ready` accept `parent`, `repo`, `labels`, `exclude_labels`, `types`, and `exclude_types`.

Editors launch the server from their MCP configuration. `orbit setup` writes project-level configs for Claude, Cursor, Codex, and OpenCode by default, recording the absolute path of the running binary so editors launched outside a shell do not need `PATH`. `--scope user` writes the harness's user-level locations instead. Generated configs record no Board path; discovery starts from the project directory, and `--dir`/`ORBIT_DIR` covers an editor that launches elsewhere. Use `--no-mcp` for instructions only, or `--portable-mcp` to write the bare `orbit` command instead; the editor or agent that launches the MCP server must then find Orbit on its `PATH`. A repair without `--portable-mcp` returns MCP entries to the absolute-path default.

## Export the board

`orbit export --json` emits a deterministic snapshot of every Task and SubTask, including terminal work, type, labels, dependency and parent edges, parent progress fields, comments, and derived readiness.

```bash
orbit export --json > board.json
orbit export --json | jq '.data.tasks[] | select(.kind == "subtask")'
```

The snapshot is ordered by task ID, carries `schema_version` (currently `2`), and contains no timestamp or machine-local paths. The same board state produces the same data, which makes export suitable for a knowledge base, an archive, or a diff between two points in time. Pin `schema_version` in the collector.

## Agent setup

`orbit init` creates a repository-local Board in `./.orbit` or a Workspace Board in `<workspace>/.orbit/board`. It asks once, in a terminal, whether to wire the harnesses it detects; without a terminal it writes no agent files and prints the next step. On the first MCP setup it also asks whether generated configs should use an absolute path (the default) or the portable `orbit` command, which requires Orbit on the editor's `PATH`. Instruction files use managed markers and MCP configs are merged at the key level, so re-running is safe and `--remove` deletes only what Orbit added.

```bash
orbit init                    # repo Board, or Workspace Board outside Git
orbit init --agents all       # wire every supported harness
orbit init --agents none      # Board only; no agent files
orbit init --print            # preview without writing
orbit init --portable-mcp     # MCP command `orbit`; editor PATH must find it
orbit init ~/src/acme          # create a Workspace Board
orbit init --global           # global Board at ~/.orbit
```

Use `orbit setup` to wire, repair, or adjust a harness later. `--scope project` (the default) writes repository-level files; `--scope user` writes the harness's user-level locations:

```bash
orbit setup claude --hooks   # CLAUDE.md + SessionStart hook + MCP
orbit setup cursor           # .cursor/rules/orbit.mdc + skill + MCP
orbit setup codex            # AGENTS.md + skill + MCP
orbit setup opencode         # AGENTS.md + MCP
orbit setup mattpocock       # docs/agents/issue-tracker.md
orbit setup all --scope user # machine-wide instruction and MCP files
orbit setup all --check      # installed / missing / stale
orbit setup all --no-mcp     # instruction files only
orbit setup all --portable-mcp # MCP command `orbit`; editor PATH must find it
orbit setup all --remove     # remove only Orbit-managed content
```

**Core `setup` parameters**

| Parameter | Use it when | Example |
| --- | --- | --- |
| `--scope project` | Default. Write instruction and MCP files for this repository. | `orbit setup claude --scope project` |
| `--scope user` | One machine-wide integration; writes the harness's user-level locations. | `orbit setup cursor --scope user` |
| `--check` | CI or a repair check: report installed, missing, or stale and exit non-zero on drift. | `orbit setup all --check` |
| `--remove` | Uninstall only what Orbit manages, leaving unrelated configuration alone. | `orbit setup all --remove` |
| `--no-mcp` | Write instruction and skill files without MCP entries. | `orbit setup codex --no-mcp` |
| `--hooks` | Add the Claude Code SessionStart hook. | `orbit setup claude --hooks` |
| `--portable-mcp` | Write bare `orbit` in MCP configs; the editor's PATH must resolve it. | `orbit setup opencode --portable-mcp` |
| `--print` | Preview the exact files that would be written or removed. | `orbit setup all --print` |

Generated configs record the Orbit binary but not the Board path, so moving the Orbit folder does not leave them stale. MCP entries use an absolute binary path by default; `--portable-mcp` writes `orbit` instead, which only works when the editor's `PATH` resolves it. `doctor` reports an older config that still contains an absolute Board path.

A hand-written tracker guide is respected. If `docs/agents/issue-tracker.md` exists without Orbit's managed marker, `orbit setup mattpocock` reports it as `skipped` and leaves it untouched; `--check` treats the custom doc as installed, and `--remove` does not delete it. Tracker files Orbit wrote continue to update and remove normally.

CI runs `orbit setup opencode --no-mcp --check` on every push and pull request, so the checked-in `AGENTS.md` managed block cannot drift from the generator. `--no-mcp` keeps the machine-local `opencode.json` out of the check.

### Claude Code hook

`orbit setup claude --hooks` installs a SessionStart hook in `.claude/settings.json`. The hook records the absolute Orbit binary and runs:

```bash
orbit prime --hook-json --by claude
```

`prime` prints the agent workflow, board counts, top ready tasks, close candidates, and stale claims — about 1k tokens of starting context. `--hook-json` wraps the same text in Claude Code's SessionStart envelope.

Because generated MCP commands are machine-specific, keep those config files untracked when possible. If one is committed, regenerate it after installing Orbit elsewhere, or use `--portable-mcp` so the committed command stays `orbit`.

## Command map

| Stage | Commands |
| --- | --- |
| **Set up** | `init` · `link` · `unlink` · `setup` |
| **Author** | `add` · `update` · `depend` · `undepend` · `label` · `comment` · `delete` |
| **Inspect** | `list` · `ready` · `show` · `tree` · `prime` · `export` · `github status` |
| **Execute** | `claim` · `heartbeat` · `checkpoint` · `release` · `done` · `cancel` · `reopen` |
| **Maintain** | `doctor` · `sync` · `github` · `slug` · `mcp` |

Every command explains its own semantics:

```bash
orbit <command> --help    # long description, options, and examples
orbit <command> -h        # compact summary
```

The full guide — when to use each command, end-to-end scenarios, and a common-errors table — is in [docs/commands.md](./docs/commands.md).

## Recovery and maintenance

| Situation | What to do |
| --- | --- |
| Nothing is startable | `NO_READY_TASKS` is a success with `data: null`. Wait, triage, or remove a hold label. |
| Another agent owns the task | Pick other work, or `orbit claim <id> --by you --steal`. |
| A lease expired | `orbit claim <id> --by you --steal`, or `orbit release <id> --force` to requeue it. |
| Work must be handed off | `orbit checkpoint <id> --by you --progress "..." --next "..."`, then `orbit release <id> --by you`; the next claimant sees it in `ready` and `show`. |
| Dependencies are unfinished | Read `error.details.blocking`, then finish or cancel those IDs. |
| A parent has open children | Finish the children, or pass `--force` to `done`/`cancel` to cancel them. |
| A test failed after a crash | `orbit doctor` reports integrity issues; `doctor --fix` removes stale temp files and re-attaches a deleted `.orbit` worktree. |
| Heartbeats are uncommitted | `orbit sync` commits lease-only changes. |
| The Task-ID prefix must change | Preview with `orbit slug migrate <slug> --dry-run`, then apply. Live claims require `--force`; external old-ID links still break. |
| The Board is missing | `orbit init` creates one; a missing linked Board returns `LINK_NOT_FOUND`. |
| The Board is in the old hidden layout | It keeps working with a `LEGACY_LAYOUT` warning. Run `orbit doctor --migrate` to move it to `.orbit`; the migration is resumable. |
| A writer is busy | `LOCKED` is retryable. Retry after a short backoff. |

`doctor` never rewrites tasks or comments, so it is always safe to run. It works without a Board: it reports the discovery trace, legacy layout, `AMBIGUOUS_BOARD`, `ORPHAN_WORKTREE`, `STORAGE_MISMATCH`, and agent configs that still record an absolute Board path, then checks malformed tasks, dangling dependencies, missing or nested parents, parent/ID mismatches, cycles, orphan or corrupt comments, stale temp files, uncommitted changes, the checked-out branch, and a Claude hook that cannot find the Orbit binary.

## Why not Beads or Beans?

- **Beans** proved that Markdown-as-truth is pleasant for humans, but it has no atomic claims, leases, or dependency-aware ready queue.
- **Beads** proved that agents need claims, ready-work detection, priorities, and hash IDs, but its source of truth is a Dolt database and its multi-writer mode needs a server.

Orbit is the narrow overlap: Beans' human-readable storage with Beads' queue semantics, in one Rust binary with no daemon.

| | Orbit | Beads | Beans |
| --- | --- | --- | --- |
| Source of truth | Markdown on Git | Dolt database | Markdown |
| Task diffs reviewable in Git | Yes | No | Yes |
| Atomic claims + leases | Yes | Yes | No |
| Dependency-aware ready queue | Yes | Yes | No |
| Shared Board across repositories | Yes | Central database with hydrated copies | No |
| Server required for concurrency | No | Yes for multi-writer | No |

Orbit does not try to beat Beads at distributed sync, federation, or scale. It wins by keeping the board human-readable and diffable, and by making concurrent local agents safe with a single locked writer instead of a database server.

## Compatibility vocabulary

External trackers and skills use different words. Orbit translates at the boundary instead of inventing a second model:

- **Issue** = Task
- **Sub-issue** = SubTask, created with `--parent`
- **Epic** or **spec** = top-level Task with `type: epic`

`kind` is structural (`task` or `subtask`). `type` is the work shape (`task`, `epic`, `spike`, `chore`, or `decision`) and is metadata only. `epic` is refused on a SubTask. `bug` and `enhancement` remain triage labels, not Types.

The `epic` label is legacy. Orbit does not infer `type: epic` from it, and `--exclude-label epic` still works on boards created before the Type field. New work should use `--type epic` and `--exclude-type epic`.

See [ADR 0009](./docs/adr/0009-keep-task-vocabulary.md) and [ADR 0010](./docs/adr/0010-structural-type-field.md).

## Install details

Prebuilt binaries are the primary path. `v0.9.4` is the latest release. Every persistent route requires Git 2.31+ at runtime.

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

See [RELEASING.md](./docs/RELEASING.md) for the release process, supported targets, and packaging checks.

## Documentation

| Document | What it covers |
| --- | --- |
| [Command guide](./docs/commands.md) | Every command, when to use it, scenarios, and error recovery |
| [Architecture](./docs/ARCHITECTURE.md) | Component map, board resolution, domain model, state machine, readiness gate, command flow, sequence diagrams |
| [Specification](./docs/SPEC.md) | Canonical behavior, task format, config, command rules, JSON contract, concurrency, testing, distribution |
| [GitHub sync](./docs/github-sync.md) | Basic and full GitHub modes, sync behavior, limitations, and upgrade path |
| [Glossary](./docs/GLOSSARY.md) | Canonical vocabulary and the words Orbit avoids |
| [Issue tracker guide](./docs/agents/issue-tracker.md) | How agents should use the board in this repository |
| [Triage labels](./docs/agents/triage-labels.md) | The five canonical triage roles and category labels |
| [Triage workflow](./docs/agents/triage-workflow.md) | Review/apply proposals for GitHub-linked Tasks |
| [Domain docs](./docs/agents/domain.md) | How engineering skills consume this repo's domain documentation |
| [ADRs](./docs/adr/) | Durable design decisions, from the orphan branch to the Orbit folder and Type field |
| [Releasing](./docs/RELEASING.md) | cargo-dist, npm, smoke checks, and release operations |
| [Changelog](./CHANGELOG.md) | What changed in each release |

## Development

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

The test suite covers frontmatter round-trips, label normalization, comment parsing, ID generation, graph validation, readiness, lifecycle transitions, property-based DAG checks, multi-process claims and deletes, crash recovery, worktree sharing, Orbit folder discovery and migration, setup recipes and scopes, onboarding, SubTasks, export determinism, MCP over a real stdio child, golden JSON, and error envelopes. CI runs on macOS, Linux, and Windows.

Useful local checks:

```bash
cargo run -- --help
cargo run -- init --print
cargo test --test concurrency
cargo test --test mcp
```

## Contributing

Contributions are welcome. This repository tracks its own work on Orbit:

```bash
orbit ready --claim --by <agent>
orbit show <id> --json
```

Before opening a pull request, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`. Changes to behavior, output, configuration, or workflow must update the relevant docs in the same change; design decisions that constrain future work belong in an [ADR](./docs/adr/).

## License

Apache-2.0. See [LICENSE](./LICENSE).

---

<div align="center">

**If Orbit fits the way you work, give it a star — and put your next agent task on the board.**

```bash
orbit init && orbit add "Try Orbit" -p 2 --label ready-for-agent
```

</div>
