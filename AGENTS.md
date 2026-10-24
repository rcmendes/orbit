<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->
## Orbit task tracking

Orbit is the task tracker for this repository. Use the `orbit` CLI; never edit task files or YAML frontmatter directly.

- `orbit ready --claim --by <agent>` claims the highest-priority ready task atomically.
- `orbit heartbeat <id> --by <agent>` extends the lease while you work.
- `orbit done <id> --by <agent>` completes it; `orbit cancel <id> --by <agent>` abandons it.
- `orbit add "title" -p N --depends-on <id>` creates work.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- Run `orbit prime` at session start for ready work, stale claims, and counts.

Parse the JSON envelope: `ok`, `data`, `error.code`, and `error.retryable`. If a command returns `LOCKED`, retry after a short backoff. Never infer state from prose.
<!-- END ORBIT INTEGRATION -->

## Agent skills

### Issue tracker

Orbit local CLI tracker; see `docs/agents/issue-tracker.md`.

### Triage labels

Canonical five roles plus `bug`/`enhancement`; see `docs/agents/triage-labels.md`.

### Domain docs

Single-context; see `docs/agents/domain.md`.
