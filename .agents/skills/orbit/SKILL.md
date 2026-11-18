---
name: orbit
description: "Use the Orbit CLI to claim, track, and complete tasks in this repository. Trigger when the user asks what to work on, asks to create or update a task, or starts a multi-step implementation."
---

<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->
## Orbit issue tracking

Orbit is the issue tracker for this project. Use the `orbit` CLI; never edit task files or YAML frontmatter directly.

From any member Repository, these commands resolve the shared Board across the Git boundary. Issues, dependencies, and claims are shared across its Repositories.

- `orbit ready --claim --by <agent>` claims the highest-priority ready Issue atomically.
- `orbit claim <id> --by <agent> --worktree` claims one Issue and creates `.worktrees/<id>` on `ticket/<id>` in the Issue's Repository.
- `orbit assign <id> --to <name>` assigns an Issue to a person or agent without a lease.
- `orbit heartbeat <id> --by <agent>` extends the lease while you work.
- `orbit checkpoint <id> --by <agent> --progress "..." [--next "..."] [--blocker "..."]` records a handoff that survives release and takeover.
- `orbit done <id> --by <agent> --comment "..."` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it.
- `orbit add "title" -p high` creates work; `orbit depend <id> <dep-id>` adds a blocker.
- `orbit add "title" --parent <id>` creates a child Issue. Issue types and the parent hierarchy are configured per Board under `board.types` and `board.hierarchy` (default: `epic` above `story`/`task`/`bug` above `sub-task`); a child must sit below its parent in the hierarchy, `--parent` without `--type` files a `sub-task`, and the parent waits for every descendant to resolve.
- Never use `done --force` or `cancel --force` on a parent with open sub-issues; it cancels them.
- Use `--type epic` or `--exclude-type epic` on `orbit ready` or `orbit list` to find or skip spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Use `orbit tree <id>` to inspect one epic/spec with its sub-issues and blockers.
- Inside a member Repository, `orbit ready` and `orbit list` scope to it plus Board-wide Issues; `--all` widens to the whole Board.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- `orbit batch --file plan.json` applies many changes in one commit, all or none; per-operation `key`/`if_version` make retries safe.
- Pass `--if-version <updated_at>` (from `orbit show`) on a mutation to fail with a retryable `VERSION_CONFLICT` on stale reads, and `--idempotency-key <key>` so a retry returns the first result instead of reapplying.
- Run `orbit prime` at session start for ready work, stale claims, and counts.
- `orbit list` prints a table for humans; parse `--json` in automation. `orbit ready`, `orbit search`, `orbit digest`, and `orbit tree` print compact summary lines; `list`/`search` page with `--limit`/`--cursor`.

Parse the JSON envelope: `ok`, `data`, `error.code`, and `error.retryable`. If a command returns `LOCKED`, retry after a short backoff. Never infer state from prose.
<!-- END ORBIT INTEGRATION -->
