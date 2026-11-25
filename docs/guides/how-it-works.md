# How it works

One binary, two front doors, one lock, atomic Markdown writes. The architecture in one page.

Orbit is one binary with two front doors: the JSON CLI and the MCP stdio server. A command is a short-lived process. It resolves one Board, takes the right lock, rebuilds the issue graph from Markdown, applies one transition, publishes changed files atomically, and optionally commits to the `orbit` branch.

```mermaid
flowchart LR
    subgraph Clients["Clients"]
        DEV["Developer"]
        AGENT["Coding agent<br/>Claude Code · Cursor · Codex · OpenCode · Devin"]
        MCPC["MCP client"]
    end

    subgraph Binary["orbit binary"]
        ROUTER["CLI router<br/>clap"]
        MCP["MCP stdio server"]
        CMD["command handlers"]
        GRAPH["in-memory issue graph"]
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
        BRANCH["board branch: orbit"]
    end

    subgraph Harness["Harness files"]
        FILES["CLAUDE.md · AGENTS.md<br/>.mcp.json · opencode.json · .devin/"]
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

The full component map, board-resolution flow, domain model, state machine, readiness gate, command map, and sequence diagrams for `init`, the issue lifecycle, concurrent claims, and MCP live in [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md).

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
    CLI->>CLI: filter Ready issues, pick the highest priority
    CLI->>Board: atomic write the active status + lease
    CLI->>Git: commit "orbit: claim ID"
    CLI-->>Agent: JSON envelope with issue and lease

    Agent->>CLI: orbit heartbeat ID --by claude
    CLI->>Lock: acquire exclusive
    CLI->>Board: extend lease_until
    Note right of Git: heartbeat is not committed until orbit sync
    CLI-->>Agent: lease_until, meta.committed false

    Agent->>CLI: orbit checkpoint ID --by claude --progress "..." --next "..."
    CLI->>Lock: acquire exclusive
    CLI->>Board: replace checkpoint in issue frontmatter
    CLI->>Git: commit "orbit: checkpoint ID"
    CLI-->>Agent: JSON detail with the checkpoint

    Agent->>CLI: orbit done ID --by claude --comment "verified"
    CLI->>Lock: acquire exclusive
    CLI->>CLI: check dependencies, sub-issues, and claim holder
    CLI->>Board: atomic write done + append comment
    CLI->>Git: commit "orbit: done ID"
    CLI-->>Agent: JSON detail with comments
```

If nothing can start, `ready --claim` is still a success:

```json
{"ok": true, "data": null, "meta": {"reason": "NO_READY_TASKS"}}
```

Polling loops should wait and retry rather than treating that as an error. When another agent holds the issue, the loser gets `ALREADY_CLAIMED` with the holder and lease. When a lease expires, the issue becomes **stale**, not Ready; recover it with `orbit claim <id> --steal` or return it to the queue with `orbit release <id> --force`.

The queue order is priority, then age — opt-in scheduler strategies (`critical-path`, `touches` overlap avoidance, `capability`, `wip`) reorder or constrain it via `orbit ready --strategy` or `[scheduler]` in `config.toml`; see `docs/commands.md`.

A checkpoint is the structured handoff for that recovery. Only the claim holder can write one; it records progress, the next action, and any blocker. Releasing or stealing the issue keeps the checkpoint, so the next claimant sees it in `orbit show`, the JSON for `orbit ready`, MCP `get`, or the claim result. On a done or cancelled issue the latest checkpoint stays in the file as archived history (`checkpoint_archived: true`) but is not part of the active queue.

### The readiness gate

`ready` is computed on every command. It is never a stored flag.

```mermaid
flowchart TD
    TASK["Issue candidate"] --> STATUS{"status is the workflow's ready status?"}
    STATUS -->|no| NOTREADY["not ready"]
    STATUS -->|yes| CLAIMED{"assignee is null?"}
    CLAIMED -->|no| NOTREADY
    CLAIMED -->|yes| WAITING{"no blocking link to an open Issue or external reference?"}
    WAITING -->|no| NOTREADY
    WAITING -->|yes| HELD{"any hold label?"}
    HELD -->|yes| NOTREADY
    HELD -->|no| DEPS{"every dependency resolved?"}
    DEPS -->|no| BLOCKED["blocked by dependencies"]
    DEPS -->|yes| SUBTASKS{"every sub-issue resolved?"}
    SUBTASKS -->|no| SUBTASKSOPEN["open sub-issues"]
    SUBTASKS -->|yes| READY["ready<br/>eligible for ready and claim"]

    classDef no fill:#fef2f2,stroke:#b91c1c,color:#450a0a
    classDef yes fill:#f0fdf4,stroke:#15803d,color:#052e16
    classDef gate fill:#eff6ff,stroke:#1d4ed8,color:#172554
    class NOTREADY,BLOCKED,SUBTASKSOPEN no
    class READY yes
    class STATUS,CLAIMED,WAITING,HELD,DEPS,SUBTASKS gate
```

The failure codes follow the gate: `WAITING_EXTERNAL` for an external blocking link, `LABEL_BLOCKED` for a hold label, `DEPENDENCY_BLOCKED` for unfinished blocking dependencies, `SUBTASKS_OPEN` for open children, `ALREADY_CLAIMED` for a live claim, and `CLAIM_STALE` for an expired one. `--steal` bypasses only the claim checks, never the gate.

### Status vs. derived state

The lifecycle is a workflow defined by the Board in `config.toml`: named statuses, each in a category (`todo`, `active`, `done`, `canceled`), named transitions between them, the initial status, and one ready status. Boards pick a shipped template — `minimal`, `standard` (the default), or `agentic` — or write a custom `[workflow]` section; `orbit transition <id> <name>` runs any defined edge, and `orbit show` lists `allowed_transitions`. The standard template:

```mermaid
stateDiagram-v2
    direction LR

    [*] --> ready: add

    ready --> in_progress: claim or ready --claim
    ready --> done: done
    ready --> cancelled: cancel
    ready --> todo: defer
    todo --> ready: promote
    todo --> done: done
    todo --> cancelled: cancel

    in_progress --> ready: release
    in_progress --> in_progress: heartbeat
    in_progress --> in_progress: checkpoint
    in_progress --> in_progress: claim --steal
    in_progress --> done: done
    in_progress --> cancelled: cancel

    done --> ready: reopen
    cancelled --> ready: reopen
```

A transition into a done or cancelled status records its resolution on the issue; reopening clears it. Ready, blocked, waiting, stale, and held are derived from claims, blocking links, labels, and sub-issues. They cannot be written as a Status. Statuses in the `done` and `canceled` categories both resolve dependents; `delete` is not a transition, it removes the task files.
