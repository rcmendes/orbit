<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->
## Orbit task tracking

Orbit is the task tracker for this project. Use the `orbit` CLI; never edit task files or YAML frontmatter directly.

From any member Repository, these commands resolve the shared Board across the Git boundary. Tasks, dependencies, and claims are shared across its Repositories.

- `orbit ready --claim --by <agent>` claims the highest-priority ready task atomically.
- `orbit heartbeat <id> --by <agent>` extends the lease while you work.
- `orbit checkpoint <id> --by <agent> --progress "..." [--next "..."] [--blocker "..."]` records a handoff that survives release and takeover.
- `orbit done <id> --by <agent> --comment "..."` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it.
- `orbit add "title" -p N --depends-on <id>` creates work.
- `orbit add "title" --parent <id>` creates a SubTask. A spec or epic is a top-level Task with `--type epic`; its issues are SubTasks. `type` is metadata: task, epic, spike, chore, decision. Epic is top-level only. SubTasks are one level deep and the parent waits for them.
- Never use `done --force` or `cancel --force` on a parent with open SubTasks; it cancels them.
- Use `--type epic` or `--exclude-type epic` on `orbit ready` or `orbit list` to find or skip spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Use `orbit tree <id>` to inspect one epic/spec with its SubTasks and blockers.
- Inside a member Repository, `orbit ready` and `orbit list` scope to it plus Board-wide Tasks; `--all` widens to the whole Board.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- Run `orbit prime` at session start for ready work, stale claims, and counts.
- `orbit list` prints a table for humans; the `ISSUE` column is `-` for a Local Task, `#N` for a Linked Task, and `~#N` for detached provenance. Filter with `--linked` or `--local`; parse `--json` in automation. `orbit ready` and `orbit tree` print compact summary lines.

Parse the JSON envelope: `ok`, `data`, `error.code`, and `error.retryable`. If a command returns `LOCKED`, retry after a short backoff. Never infer state from prose.
<!-- END ORBIT INTEGRATION -->

## Worktrees for Orbit tasks

Implement every Orbit task in a dedicated git worktree — never in the main
checkout, so parallel agent sessions and reviews stay undisturbed.

1. Claim the task, then create the worktree and branch:

   ```bash
   git worktree add .worktrees/<task-id> -b ticket/<task-id> <base>
   ```

   `<base>` is the repo's default branch, or the base branch the task specifies.
   `.worktrees/` is gitignored.
2. Do all work — edits, builds, tests, commits — inside `.worktrees/<task-id>`.
   `orbit` commands resolve the shared board from inside the worktree.
3. Commit, push, and open the PR from the `ticket/<task-id>` branch.
4. Keep the worktree while review is pending; after merge or `orbit done`,
   remove it with `git worktree remove .worktrees/<task-id>`.


## Documentation

Always update the documentation after committing a change that affects behavior,
output, configuration, or workflow. A commit is not finished until the docs
describe what the code now does.

- Review `README.md`, `AGENTS.md`, and every document in `docs/` after every
  commit and update anything the change made stale.
- Update CLI `--help` text, generated setup templates, and examples when the
  CLI or agent workflow changes.
- If the code commit is already made, create an immediate follow-up docs commit.
  Do not leave `main` describing the previous behavior.
- Pure internal refactors with no user-visible effect do not need docs, but say
  so explicitly when closing the work.

## Agent skills

### Issue tracker

Orbit local CLI tracker; see `docs/agents/issue-tracker.md`.

### Triage labels

Canonical five roles plus `bug`/`enhancement`; see `docs/agents/triage-labels.md`.

### Domain docs

Single-context; see `docs/agents/domain.md`.
