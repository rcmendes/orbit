<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->
## Orbit task tracking

Orbit is the task tracker for this repository. Use the `orbit` CLI; never edit task files or YAML frontmatter directly.

- `orbit ready --claim --by <agent>` claims the highest-priority ready task atomically.
- `orbit heartbeat <id> --by <agent>` extends the lease while you work.
- `orbit done <id> --by <agent> --comment "..."` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it.
- `orbit add "title" -p N --depends-on <id>` creates work.
- `orbit add "title" --parent <id>` creates a SubTask. A spec or epic is a top-level Task with `--type epic`; its issues are SubTasks. `type` is metadata: task, epic, spike, chore, decision. Epic is top-level only. SubTasks are one level deep and the parent waits for them.
- Never use `done --force` or `cancel --force` on a parent with open SubTasks; it cancels them.
- Use `--type epic` or `--exclude-type epic` on `orbit ready` or `orbit list` to find or skip spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Use `orbit tree <id>` to inspect one epic/spec with its SubTasks and blockers.
- In a multi-repository workspace, add `--repo <name>` to `orbit ready` or `orbit list`.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- Run `orbit prime` at session start for ready work, stale claims, and counts.

Parse the JSON envelope: `ok`, `data`, `error.code`, and `error.retryable`. If a command returns `LOCKED`, retry after a short backoff. Never infer state from prose.
<!-- END ORBIT INTEGRATION -->

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
