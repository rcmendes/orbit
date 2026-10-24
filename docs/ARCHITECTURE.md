# Orbit Architecture

Status: describes v0.2.0. [`SPEC.md`](./SPEC.md) is canonical for behavior and the JSON contract; [`GLOSSARY.md`](./GLOSSARY.md) defines the vocabulary. This document visualizes that design and maps it to the Rust modules.

The short version: Orbit is one binary, two front doors (the JSON CLI and the MCP stdio server), and no daemon. A board command resolves a Board, takes a lock, rebuilds the task graph from Markdown, applies one transition, publishes files atomically, and optionally commits to an orphan Git branch.

## 1. Component map

Solid arrows are calls or data flow. Dotted arrows are selection or lookup. The `orbit mcp` server is a bridge, not a second implementation: it spawns the CLI for every tool call.

```mermaid
flowchart LR
    subgraph Actors["Actors and harnesses"]
        DEV["Developer"]
        AGENT["Coding agent<br/>Claude Code · Cursor · Codex · OpenCode"]
        MCPC["MCP client"]
        HOOK["Claude SessionStart hook"]
    end

    subgraph Interfaces["Interfaces"]
        CLI["JSON CLI<br/>src/cli.rs"]
        MCP["MCP stdio server<br/>src/mcp.rs"]
        ENVELOPE["Envelope renderer<br/>src/output.rs"]
    end

    subgraph Application["Application"]
        CMD["Command handlers<br/>src/commands.rs"]
        PRIME["prime context"]
        DOCTOR["doctor and sync"]
        SETUP["Harness setup<br/>src/setup.rs"]
    end

    subgraph Domain["Domain"]
        BOARD["Board resolution and config<br/>src/board.rs"]
        GRAPH["Task graph<br/>src/graph.rs"]
        MODEL["Task model<br/>src/task.rs"]
        COMMENTS["Comment sidecars<br/>src/comment.rs"]
    end

    subgraph Durability["Durability"]
        LOCK["Board lock<br/>src/lock.rs"]
        ATOMIC["Atomic publish<br/>src/atomic.rs"]
        GIT["Git adapter<br/>src/git.rs"]
        ERRORS["Error contract<br/>src/error.rs"]
    end

    subgraph Storage["Durable state"]
        BRANCH["orphan branch<br/>orbit"]
        WORKTREE["hidden worktree<br/>.git/orbit/board"]
        EXTERNAL["shared Board<br/>--board or ORBIT_DIR"]
        LINKPTR["link pointer<br/>.git/orbit/link"]
        LOCKFILE["lock file<br/>.git/orbit/lock"]
        TASKFILES["tasks/ID.md"]
        COMMENTFILES["comments/ID.md"]
        CONFIGFILE["config.toml"]
    end

    subgraph Repo["Consumer repository"]
        WORKTREES["main and linked worktrees"]
        HARNESSFILES["CLAUDE.md · AGENTS.md<br/>.cursor/rules · opencode.json<br/>.mcp.json · .codex/config.toml"]
    end

    DEV --> CLI
    AGENT --> CLI
    AGENT --> HOOK
    HOOK -->|runs prime --hook-json| CLI
    MCPC --> MCP
    MCP -->|spawns one CLI child per tool call| CLI
    CLI --> CMD
    CLI --> ENVELOPE
    CMD --> PRIME
    CMD --> DOCTOR
    CMD --> SETUP
    CMD --> Domain
    CMD --> Durability
    CMD --> ERRORS
    Domain --> Storage
    Durability --> Storage
    SETUP --> HARNESSFILES
    SETUP --> HOOK
    WORKTREES -.->|all resolve one Board| BOARD
    LINKPTR -.->|selects| EXTERNAL
    BOARD --> WORKTREE
    BOARD --> EXTERNAL
    GRAPH --> MODEL
    GRAPH --> TASKFILES
    GIT --> BRANCH
    WORKTREE --> TASKFILES
    WORKTREE --> COMMENTFILES
    WORKTREE --> CONFIGFILE
    EXTERNAL --> TASKFILES
    EXTERNAL --> COMMENTFILES
    EXTERNAL --> CONFIGFILE
    LOCK --> LOCKFILE
    ATOMIC --> TASKFILES
    ATOMIC --> COMMENTFILES
    COMMENTS --> COMMENTFILES

    classDef actor fill:#fff7ed,stroke:#c2410c,color:#431407
    classDef iface fill:#eef2ff,stroke:#4f46e5,color:#1e1b4b
    classDef app fill:#f5f3ff,stroke:#7c3aed,color:#2e1065
    classDef domain fill:#ecfeff,stroke:#0e7490,color:#083344
    classDef durable fill:#fef3c7,stroke:#b45309,color:#451a03
    classDef store fill:#f0fdf4,stroke:#15803d,color:#052e16
    classDef repo fill:#fdf2f8,stroke:#be185d,color:#500724

    class DEV,AGENT,MCPC,HOOK actor
    class CLI,MCP,ENVELOPE iface
    class CMD,PRIME,DOCTOR,SETUP app
    class BOARD,GRAPH,MODEL,COMMENTS domain
    class LOCK,ATOMIC,GIT,ERRORS durable
    class BRANCH,WORKTREE,EXTERNAL,LINKPTR,LOCKFILE,TASKFILES,COMMENTFILES,CONFIGFILE store
    class WORKTREES,HARNESSFILES repo
```

### What each layer owns

| Layer | Modules | Responsibility |
| --- | --- | --- |
| Interface | `src/cli.rs`, `src/output.rs`, `src/mcp.rs` | Parse commands, emit exactly one JSON object on stdout in `--json`, and expose the same operations over MCP. |
| Application | `src/commands.rs` | Dispatch, acquire the right lock mode, hydrate the graph, enforce transition rules, call the writer, and shape the result. |
| Domain | `src/board.rs`, `src/graph.rs`, `src/task.rs`, `src/comment.rs` | Board discovery and config, task parsing and rendering, dependency validation, readiness, counts, and comments. |
| Durability | `src/lock.rs`, `src/atomic.rs`, `src/git.rs` | One repo-wide advisory lock, temp-file + fsync + rename publication, and auto-commit to the board branch. |
| Contract | `src/error.rs` | Stable error codes, messages, details, and retryability. |
| Setup | `src/setup.rs` | Detect harnesses and write managed instruction files, MCP entries, and the Claude SessionStart hook. |

### Board resolution

Every command resolves the Board before it does any work. The first match wins.

```mermaid
flowchart TD
    START(["orbit command"]) --> OVERRIDE{"--dir or ORBIT_DIR set?"}
    OVERRIDE -->|yes| LOAD["Board::discover(path)<br/>load config.toml"]
    LOAD --> EXISTS{"config.toml exists?"}
    EXISTS -->|no| NOBOARD["BOARD_NOT_FOUND"]
    EXISTS -->|yes| LOCK["acquire shared or exclusive lock"]

    OVERRIDE -->|no| REPO{"inside a Git repository?"}
    REPO -->|no| NOREPO["GIT_ERROR<br/>init and link also return NOT_A_REPOSITORY"]
    REPO -->|yes| LINKED{"link pointer exists?<br/>.git/orbit/link"}
    LINKED -->|yes| TARGET{"target has config.toml?"}
    TARGET -->|no| LINKERR["LINK_NOT_FOUND"]
    TARGET -->|yes| SHARED["use linked Board<br/>lock at board/.lock"]
    LINKED -->|no| LOCAL{"local Board has config.toml?<br/>.git/orbit/board"}
    LOCAL -->|no| NOBOARD2["BOARD_NOT_FOUND<br/>run orbit init"]
    LOCAL -->|yes| OWN["use local Board<br/>lock at .git/orbit/lock"]

    SHARED --> LOCK
    OWN --> LOCK
```

The pointer lives in the Git common directory, not in a worktree. That is why a claim made from a linked worktree and a claim made from the main worktree serialize on the same lock and see the same tasks. A linked Board keeps its own lock at `<board>/.lock`; the local Board keeps its lock at `<git-common-dir>/orbit/lock`.

### Domain relationships

A Task is the unit of work. Its claim fields are a single time-bounded claim, and its comments live in a sidecar file keyed by the same ID. Dependency edges are IDs stored in frontmatter, so they can point across the whole Workspace.

```mermaid
classDiagram
    direction LR

    class Board {
        +PathBuf root
        +PathBuf lock_path
        +Config config
        +discover(dir)
        +lock_exclusive()
        +lock_shared()
        +commit(message, force)
    }

    class Config {
        +BoardConfig board
        +IdConfig id
    }

    class Task {
        +String id
        +String title
        +Status status
        +u8 priority
        +Vec labels
        +Vec depends_on
        +String waiting_on
        +DateTime created_at
        +DateTime updated_at
        +String body
        +parse(content)
        +render()
    }

    class Claim {
        +String claimed_by
        +DateTime claimed_at
        +DateTime lease_until
    }

    class Comment {
        +String author
        +DateTime created_at
        +String body
    }

    class Graph {
        +Map tasks
        +is_ready(task)
        +blocking_ids(task)
        +ready_tasks()
        +stale_tasks()
        +would_create_cycle(id, dep)
    }

    Board "1" *-- "1" Config : loads
    Board "1" o-- "0..*" Task : stores
    Graph "1" o-- "0..*" Task : indexes
    Task "1" *-- "0..1" Claim : claim fields
    Task "1" o-- "0..*" Comment : sidecar
    Task "0..*" --> "0..*" Task : depends_on
```

## 2. Task state machine

Only four values are **stored** in `status`: `todo`, `in_progress`, `done`, and `cancelled`. Ready, blocked, waiting, stale, and held are **derived** from other fields, so they are not states and cannot be written as one.

```mermaid
stateDiagram-v2
    direction LR

    [*] --> todo: add

    todo --> in_progress: claim or ready --claim
    todo --> done: done
    todo --> cancelled: cancel

    in_progress --> todo: release
    in_progress --> in_progress: heartbeat
    in_progress --> in_progress: claim --steal
    in_progress --> done: done
    in_progress --> cancelled: cancel

    done --> todo: reopen
    cancelled --> todo: reopen

    done --> done: done --comment
    cancelled --> cancelled: cancel --comment

    note right of in_progress
        Stale is derived, not a Status:
        in_progress with lease_until <= now.
        Release it or take it over with
        claim --steal.
    end note

    note left of todo
        Ready is derived, not a Status:
        todo, no claim, no waiting_on,
        no hold label, and every dependency
        done or cancelled.
    end note

    note right of done
        done and cancelled both resolve
        dependents. cancel is not delete:
        delete removes the task file.
    end note
```

### Transition rules

| From | Command | To | Rules |
| --- | --- | --- | --- |
| none | `add` | `todo` | Creates `tasks/ID.md`. Priority defaults to `2`; dependencies must exist. |
| `todo` | `claim` or `ready --claim` | `in_progress` | The readiness gate must pass. `--by` is required unless `ORBIT_AGENT`, `USER`, or `USERNAME` identifies the agent. A live claim returns `ALREADY_CLAIMED`; a stale claim returns `CLAIM_STALE` unless `--steal`. |
| `in_progress` | `heartbeat` | `in_progress` | Only the claim holder; extends `lease_until` by `lease_minutes`. The heartbeat write is not auto-committed. |
| `in_progress` | `claim --steal` | `in_progress` | Replaces the holder and resets `claimed_at` and `lease_until`. `--steal` does not bypass hold labels or dependencies. |
| `in_progress` | `release` | `todo` | The holder must release unless `--force`; clears all claim fields. Releasing an unclaimed task is a no-op. |
| `todo` or `in_progress` | `done` | `done` | Every dependency must be `done` or `cancelled`. A **live** claim requires the caller to be the holder unless `--force`; a stale claim does not block. Clears claim fields and `waiting_on`. |
| `todo` or `in_progress` | `cancel` | `cancelled` | A live claim requires the holder unless `--force`. Clears claim fields and `waiting_on`. Resolves dependents. |
| `done` or `cancelled` | `reopen` | `todo` | Refuses when a dependent is already `done` unless `--force`. Clears claim fields and `waiting_on`. |
| `done` | `done --comment` | `done` | Appends a comment and updates `updated_at`; without a comment it is a no-op (`changed: false`). |
| `cancelled` | `cancel --comment` | `cancelled` | Appends a comment and updates `updated_at`; without a comment it is a no-op (`changed: false`). |
| `done` | `cancel` | rejected | `INVALID_TRANSITION`; reopen first. |
| `cancelled` | `done` | rejected | `INVALID_TRANSITION`; reopen first. |
| any | `delete` | removed | Not a Status transition. Removes `tasks/ID.md` and `comments/ID.md`. A live claim requires the holder unless `--force`. With dependents, requires `--force`, which first removes the edge from each dependent. |
| any | `update`, `label`, `comment`, `depend`, `undepend` | unchanged | Metadata only. `update`, `label`, and `comment` also work on terminal tasks. `depend` refuses terminal tasks and cycles; `undepend` is idempotent. |

### The readiness gate

`ready` is computed fresh on every command. It never comes from a stored flag.

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
    HELD -->|no| DEPS{"every dependency done or cancelled?"}
    DEPS -->|no| BLOCKED["not ready<br/>blocked by dependencies"]
    DEPS -->|yes| READY["ready<br/>eligible for ready and claim"]
```

The failure codes follow the gate: `WAITING_EXTERNAL` for `waiting_on`, `LABEL_BLOCKED` for a hold label, `DEPENDENCY_BLOCKED` for unfinished dependencies, `ALREADY_CLAIMED` for a live claim, and `CLAIM_STALE` for an expired one. `--steal` bypasses only the claim checks, never the gate.

### Stored versus derived

| Concept | Stored? | Source |
| --- | --- | --- |
| Status | yes | `status` in frontmatter |
| Priority | yes | `priority`, integer `0`–`3` |
| Labels | yes | `labels`, normalized, sorted, deduplicated |
| Dependencies | yes | `depends_on` IDs |
| External wait | yes | `waiting_on` text |
| Claim and lease | yes | `claimed_by`, `claimed_at`, `lease_until` |
| Comments | yes, separately | `comments/ID.md` |
| Ready | no | `Graph::is_ready` |
| Blocked | no | `Graph::blocking_ids` |
| Stale | no | `in_progress` with `lease_until <= now` |
| Held | no | labels intersect `non_ready_labels` |

## 3. Command flow

The command map shows which command feeds the next stage. The state machine above defines the actual status transitions; this map shows how the commands relate.

```mermaid
flowchart TB
    subgraph Bootstrap["1. Bootstrap"]
        INIT["init"]
        LINK["link PATH"]
        UNLINK["unlink"]
        SETUP["setup CLIENT"]
    end

    subgraph Author["2. Author and triage"]
        ADD["add"]
        UPDATE["update"]
        LABEL["label"]
        COMMENT["comment"]
        DEPEND["depend / undepend"]
        DELETE["delete"]
        DELETED["task and comments removed"]
    end

    subgraph Inspect["3. Inspect"]
        LIST["list"]
        SHOW["show"]
        READY["ready"]
        PRIME["prime"]
    end

    subgraph Execute["4. Claim and resolve"]
        CLAIM["claim<br/>or ready --claim"]
        HEART["heartbeat"]
        RELEASE["release"]
        DONE["done"]
        CANCEL["cancel"]
        REOPEN["reopen"]
    end

    subgraph Maintain["5. Maintain"]
        DOCTOR["doctor"]
        SYNC["sync"]
        MCP["mcp"]
    end

    INIT -->|creates or reuses| SETUP
    LINK -->|selects shared Board| SETUP
    UNLINK -->|restores local Board| SETUP
    SETUP -->|wires harnesses| ADD
    ADD -->|creates todo| READY
    UPDATE -->|changes metadata| READY
    LABEL -->|hold labels gate| READY
    DEPEND -->|dependency edges gate| READY
    DELETE -->|removes| DELETED
    LIST -.->|filters| READY
    SHOW -.->|reads detail| CLAIM
    COMMENT -.->|annotates| SHOW
    PRIME -.->|prints context| READY
    READY -->|lists candidates| CLAIM
    CLAIM -->|in_progress and lease| HEART
    HEART -->|extends lease| DONE
    HEART -->|extends lease| CANCEL
    CLAIM -->|stops work| RELEASE
    DONE -->|terminal| REOPEN
    CANCEL -->|terminal| REOPEN
    DOCTOR -.->|inspects| READY
    MCP -.->|delegates each call| CLAIM
```

This map shows forward flow; solid arrows are the primary path, while dotted arrows are inspection or maintenance relationships. The state machine above holds the back edges where `release`, `done`, `cancel`, or `reopen` return a task to an earlier state. `delete` removes a task and its comments without a Status transition. `sync` commits pending heartbeat-only changes independently of the forward path.

| Family | Commands | Effect |
| --- | --- | --- |
| Bootstrap | `init`, `link`, `unlink`, `setup` | Create or select the Board, then reconcile harness files, MCP entries, and hooks. |
| Authoring | `add`, `update`, `label`, `comment`, `depend`, `undepend`, `delete` | Create tasks and change metadata. No command here changes Status except `delete`, which removes the task. |
| Inspection | `list`, `show`, `ready`, `prime` | Read-only. `ready` is the derived queue; `prime` is the compact agent context. |
| Execution | `claim`, `ready --claim`, `heartbeat`, `release`, `done`, `cancel`, `reopen` | Change Status or the claim. Writes take the exclusive lock. |
| Maintenance | `doctor`, `sync`, `mcp` | Check integrity, commit heartbeat-only changes, and serve MCP over stdio. |

## 4. Workspace initialization

### Workspace topology

A Workspace is one Board plus every Repository linked to it. Linking writes a pointer in the Repository's Git common directory; the Board itself stays in one place, so cross-repository Dependencies share one graph. `unlink` removes the pointer and returns the Repository to its local Board, if one exists.

```mermaid
flowchart LR
    subgraph Shared["Shared Workspace"]
        BOARD["shared Board<br/>~/src/acme-board"]
        R1["repository A<br/>.git/orbit/link"]
        R2["repository B<br/>.git/orbit/link"]
    end

    subgraph Own["Repository with its own Board"]
        R3["repository C"]
        LOCAL["local Board<br/>.git/orbit/board"]
    end

    R1 -.->|links to| BOARD
    R2 -.->|links to| BOARD
    R3 -->|resolves| LOCAL
    BOARD --> SHAREDGRAPH["one task graph<br/>tasks and comments"]
    LOCAL --> LOCALGRAPH["local task graph<br/>tasks and comments"]
```

### Initializing a workspace

`orbit init` is idempotent. It creates or reuses the Board, links an explicit target, and reconciles the selected harnesses instead of rejecting an existing installation.

```mermaid
sequenceDiagram
    autonumber
    actor Dev as Developer or agent
    participant CLI as orbit init
    participant SET as setup
    participant BRD as Board
    participant GIT as Git
    participant FS as Files

    Dev->>CLI: orbit init --agents auto
    CLI->>GIT: is_git_repo and repo_root
    alt repository is linked and no --local, --board, or --dir
        CLI-->>Dev: ALREADY_LINKED
    else repository is allowed
        CLI->>SET: selected_agents(args, root)
        SET-->>CLI: detected or selected clients
        opt not --print
            alt --board PATH or --dir PATH
                alt external Board already exists
                    CLI->>BRD: reuse it
                else new external Board
                    CLI->>BRD: init external Board
                    BRD->>GIT: init repo, ensure identity, HEAD orbit
                    BRD->>FS: config.toml, .gitignore, tasks, comments
                    BRD->>GIT: commit orbit: initialize board
                end
            else default local Board
                CLI->>BRD: init local Board
                alt Board already exists
                    BRD-->>CLI: reuse it
                else new Board
                    BRD->>GIT: common dir, identity, orphan branch
                    BRD->>GIT: worktree add .git/orbit/board orbit
                    BRD->>FS: config.toml, .gitignore, tasks, comments
                    BRD->>GIT: commit orbit: initialize board
                end
            end
            opt explicit target in a Git repository
                CLI->>BRD: write link pointer
            end
        end
        opt at least one client
            CLI->>SET: install clients, hooks, MCP config, print flag
            SET->>FS: harness files, MCP entries, hook when not --print
            SET-->>CLI: created, updated, or planned files
        end
        CLI-->>Dev: Board, branch, created, linked, agents, files
    end
```

With `--print`, the same plan is returned without writing the Board, the link pointer, or harness files. Re-running `init` reuses an existing Board and reports `unchanged` files where the managed content already matches.

### Linking and unlinking

| Command | Pointer | Board | Harness config |
| --- | --- | --- | --- |
| `init` | unchanged | Creates or reuses the local Board. | Reconciles detected or selected clients. |
| `init --local` | removed | Creates or reuses the local Board even when linked. | Reconciles. |
| `init --board PATH` | written | Creates or reuses the external Board. | Reconciles; MCP entries record the shared Board path. |
| `link PATH` | written | Must already be a Board. | Refreshes existing client config unless `--no-refresh`. |
| `unlink` | removed | Local Board is kept and becomes active again. | Refreshes if a local Board exists. |

A link warning about a shadowed Board means the repository still has its own Board on disk. Linking did not delete it. A missing target returns `LINK_NOT_FOUND`; the pointer is not repaired automatically.

## 5. Task lifecycle

The common agent path is `add` → `ready --claim` → `heartbeat` → `done`. Each command is a separate process; the Board lock is held only for that process.

```mermaid
sequenceDiagram
    autonumber
    actor Agent
    participant CLI as orbit CLI
    participant BRD as Board
    participant LCK as Board lock
    participant FS as Markdown files
    participant GIT as Git

    Note over Agent,GIT: add
    Agent->>CLI: orbit add "Build the router" -p 1
    CLI->>BRD: discover
    BRD->>LCK: acquire exclusive
    CLI->>FS: read all tasks, hydrate graph
    CLI->>FS: atomic write new todo
    CLI->>GIT: commit orbit: add ID
    CLI-->>Agent: JSON task detail

    Note over Agent,GIT: claim
    Agent->>CLI: orbit ready --claim --by claude
    CLI->>BRD: discover
    BRD->>LCK: acquire exclusive
    CLI->>FS: read all tasks and compute ready
    alt no ready task
        CLI-->>Agent: ok true, data null, meta.reason NO_READY_TASKS
    else first ready task
        CLI->>CLI: check waiting, hold labels, dependencies, claim
        CLI->>FS: atomic write in_progress plus lease
        CLI->>GIT: commit orbit: claim ID
        CLI-->>Agent: JSON detail, meta.claimed true
    end

    Note over Agent,GIT: heartbeat
    Agent->>CLI: orbit heartbeat ID --by claude
    CLI->>BRD: discover
    BRD->>LCK: acquire exclusive
    CLI->>FS: read all tasks
    CLI->>CLI: verify in_progress and holder
    CLI->>FS: atomic write new lease_until
    Note right of GIT: no commit until orbit sync
    CLI-->>Agent: lease_until, meta.committed false

    Note over Agent,GIT: complete
    Agent->>CLI: orbit done ID --by claude --comment "verified"
    CLI->>BRD: discover
    BRD->>LCK: acquire exclusive
    CLI->>FS: read all tasks
    CLI->>CLI: check dependencies and live claim holder
    CLI->>FS: atomic write done, clear claim and waiting
    opt comment supplied
        CLI->>FS: append immutable comment
        CLI->>FS: atomic write updated_at
    end
    CLI->>GIT: commit orbit: done ID
    CLI-->>Agent: JSON detail with comments

    Note over CLI,LCK: Every writer can return LOCKED after lock_timeout_ms. The caller retries with backoff.
```

`done` refuses unfinished dependencies and cancelled tasks. A live claim requires the holder unless `--force`; a stale claim does not. If a completion comment fails after the status write, the task is still done and the result carries `COMMENT_FAILED` with exit code `1`.

### Concurrent claims

Two agents can ask for work at the same time. The second process waits for the lock, then re-reads the board, so it cannot claim the task the first process just took.

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent A
    participant B as Agent B
    participant OA as orbit A
    participant OB as orbit B
    participant L as board lock
    participant FS as tasks

    A->>OA: ready --claim --by A
    B->>OB: ready --claim --by B
    OA->>L: try exclusive lock
    OB->>L: try exclusive lock every 10 ms
    OA->>FS: claim first ready task and commit
    OA->>L: release
    OA-->>A: claimed orb-aaaaaa
    L-->>OB: granted after wait
    OB->>FS: rehydrate and see orb-aaaaaa claimed
    alt another ready task exists
        OB->>FS: claim it and commit
        OB-->>B: claimed orb-bbbbbb
    else none ready
        OB-->>B: ok true, data null, NO_READY_TASKS
    end
    Note over OB,L: If the wait exceeds lock_timeout_ms, the command returns LOCKED and the caller retries.
```

## 6. MCP bridge

`orbit mcp` speaks JSON-RPC 2.0 over stdio. For each `tools/call`, it maps the tool arguments to CLI flags, spawns the installed `orbit` binary with `--json`, and returns the CLI envelope as text. The server holds no board lock between calls.

```mermaid
sequenceDiagram
    autonumber
    participant Client as MCP client
    participant MCP as orbit mcp
    participant CLI as orbit CLI child
    participant BRD as Board
    participant FS as Board files
    participant GIT as Git

    Client->>MCP: initialize
    MCP-->>Client: protocol, tools and resources capabilities
    Client->>MCP: tools/call ready with claim true
    MCP->>MCP: map tool arguments to CLI flags
    MCP->>CLI: spawn orbit --json ready --claim --by claude
    CLI->>BRD: discover and lock
    BRD->>FS: read task files and hydrate graph
    CLI->>FS: atomic write claim
    CLI->>GIT: auto-commit
    CLI-->>MCP: JSON envelope on stdout
    MCP-->>Client: content[0].text envelope, isError when not ok

    Client->>MCP: resources/read orbit://tasks/ID
    MCP->>CLI: spawn orbit show ID
    CLI->>FS: read task, body, and comments
    CLI-->>MCP: Markdown detail on stdout
    MCP-->>Client: contents[0].text

    Note over MCP,CLI: No board lock is held between calls and each call is a fresh CLI process.
```

## 7. Failure and recovery

| Situation | Behavior |
| --- | --- |
| Lock contention | Writers wait up to `lock_timeout_ms`, retrying every 10 ms, then return `LOCKED` with `retryable: true`. Retry with backoff. |
| Crash during a write | The old file survives; a `.tmp-orbit-*` file may remain. `doctor` reports it and `doctor --fix` removes it. |
| Multi-file mutation crash | Files are serialized but not transactional across files. `doctor` reports dangling dependencies and a repeated `delete --force` resumes cleanup. |
| Auto-commit failure | The state is already on disk. The command succeeds with `meta.warnings: ["COMMIT_FAILED: ..."]`; `orbit sync` retries the commit. |
| Heartbeat-only changes | Not committed immediately. The next meaningful mutation or `orbit sync` commits them. |
| Missing linked Board | `LINK_NOT_FOUND`; re-link or restore the Board. |
| Missing local Board | `BOARD_NOT_FOUND`; run `orbit init`. |
| Corrupt task, comment, or graph | `doctor` reports `CORRUPT_TASK`, `CORRUPT_COMMENT`, `ORPHAN_COMMENT`, `DANGLING_DEP`, or `CYCLE_DETECTED`. `doctor --fix` only removes stale temp files; it does not rewrite tasks or comments. |
| Forced reopen of a done dependency | A done dependent can keep an unfinished dependency as a historical fact. `doctor` warns; the next `done` of that dependent still enforces its dependencies. |

## 8. Where to change what

| Change | Start here | Also update |
| --- | --- | --- |
| Add a CLI command | `src/cli.rs`, then the dispatch in `src/commands.rs` | `tests/cli.rs`, `SPEC.md`, and this command map |
| Change a status transition | The handler in `src/commands.rs`, plus `Graph::is_ready` or `blocking_ids` if readiness changes | `tests/properties.rs`, the state machine in this document, and `SPEC.md` |
| Add a task field | `TaskFrontmatter` and parsing in `src/task.rs` | `graph::task_summary` and `task_detail`, plus round-trip tests |
| Add an MCP tool | `tool_definitions` and `dispatch_tool` in `src/mcp.rs` | `tests/mcp.rs`, the MCP bridge section, and `SPEC.md` |
| Add a harness | `SetupClient` and the install and remove plans in `src/setup.rs` | Detection tests in `tests/setup.rs`, onboarding tests, and `docs/agents/` |
| Change board storage | `src/board.rs` and the init path | [ADR 0001](adr/0001-markdown-board-on-orphan-branch.md), [ADR 0002](adr/0002-single-writer-lock-and-atomic-rename.md), and the component map here |

## 9. Invariants

- The board is the only source of truth. There is no persistent index, cache, or watcher; every command rehydrates from Markdown.
- Readers take the shared lock for a consistent snapshot. Writers take the exclusive lock for the read-modify-write.
- Each file is published with a temp file, `fsync`, and rename. A crash cannot truncate a task.
- Ready and blocked are derived on every command. They are never stored in frontmatter.
- `done` and `cancelled` both resolve dependents. `delete` is not a status; it removes the task and its comments.
- Comments are append-only and immutable. A correction is a new comment.
- Meaningful mutations auto-commit to the `orbit` branch. Heartbeats wait for the next mutation or `sync`. Nothing is pushed.
- MCP does not bypass the CLI. Every tool call reuses the same lock, atomic-write, auto-commit, and error envelope.
- Agent configuration is written with managed markers or key-level merges so `setup --remove` deletes only Orbit-managed content.
