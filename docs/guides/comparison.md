# Why Orbit, and what it is not

The problems Orbit solves, how it compares to Beads and Beans, and the vocabulary other trackers use.

## Why Orbit

Most trackers were designed for humans clicking through a web UI. Agents need something different: a stable contract, a small context footprint, and a queue that cannot hand the same ticket to two workers.

| The usual problem | Orbit's answer |
| --- | --- |
| Issue state lives in a database nobody can review | Issues are Markdown files on a dedicated Git branch, diffable like code |
| Two agents grab the same ticket | `ready --claim` is atomic; the loser gets a typed error |
| An agent dies mid-issue and the ticket is lost | Claims expire after a lease; recover with `claim --steal` |
| Agents burn tokens parsing the whole backlog | A dependency-aware ready queue returns one actionable issue |
| Writes race and truncate files | One board lock, temp-file + `fsync` + rename, serialize every mutation |
| Work spans repositories but the board does not | One shared Board for a whole anchor, including every Git worktree |
| The tracker requires infrastructure to run | One binary, no daemon, no database, no hosted service |
| Status is prose an agent has to interpret | Stable JSON envelope, typed error codes, and retryability |

Orbit is deliberately narrow. It is not a project-management suite, a time tracker, or a hosted sync service. It is the smallest reliable queue that lets several coding agents work on one product without stepping on each other.

### Built for

- **Solo builders** who want a fast local backlog that lives beside the code and never needs a server.
- **Agent squads** running Claude Code, Cursor, Codex, OpenCode, and Devin in parallel against one product.
- **Multi-repository products** where the API, web app, infrastructure, and agent tooling all need one dependency graph.

## Why not Beads or Beans?

- **Beans** proved that Markdown-as-truth is pleasant for humans, but it has no atomic claims, leases, or dependency-aware ready queue.
- **Beads** proved that agents need claims, ready-work detection, priorities, and hash IDs, but its source of truth is a Dolt database and its multi-writer mode needs a server.

Orbit is the narrow overlap: Beans' human-readable storage with Beads' queue semantics, in one Rust binary with no daemon.

| | Orbit | Beads | Beans |
| --- | --- | --- | --- |
| Source of truth | Markdown on Git | Dolt database | Markdown |
| Issue diffs reviewable in Git | Yes | No | Yes |
| Atomic claims + leases | Yes | Yes | No |
| Dependency-aware ready queue | Yes | Yes | No |
| Shared Board across repositories | Yes | Central database with hydrated copies | No |
| Server required for concurrency | No | Yes for multi-writer | No |

Orbit does not try to beat Beads at distributed sync, federation, or scale. It wins by keeping the board human-readable and diffable, and by making concurrent local agents safe with a single locked writer instead of a database server.

## Compatibility vocabulary

External trackers and skills use different words. Orbit translates at the boundary instead of inventing a second model:

- **Issue** = Issue
- **Sub-issue** = sub-issue, created with `--parent`
- **Epic** or **spec** = top-level Issue with `type: epic`

`type` is the Board-configured Issue type (`epic`, `story`, `task`, `bug`, `sub-task` by default) and decides where in `board.hierarchy` the Issue may sit; a child filed with `--parent` takes the leaf `sub-task` type. `bug` and `enhancement` remain triage labels, not Types.

The `epic` label is legacy. Orbit does not infer `type: epic` from it, and `--exclude-label epic` still works on boards created before the Type field. New work should use `--type epic` and `--exclude-type epic`.

See [ADR 0022](../../docs/adr/0022-issue-model-and-platform-architecture.md) and [ADR 0010](../../docs/adr/0010-structural-type-field.md).
