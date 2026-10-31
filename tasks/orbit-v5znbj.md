---
title: GitHub status and full sync cost one API call per linked Task
status: todo
type: task
priority: 1
created_at: 2026-10-31T12:50:18.633126Z
updated_at: 2026-10-31T12:50:18.633126Z
---

`orbit github status` and a whole-Board `orbit github sync` make one `gh api` call per linked Task, sequentially. On a real Workspace Board (teramigo: 6 repositories, 582 links) neither finishes in a useful time.

Measured 2026-10-31 on the teramigo Workspace Board:

- `orbit github sync` (all links): exceeded 10 minutes, killed.
- `orbit github status`: exceeded 4 minutes, killed.
- `orbit github sync <task>`: about 2 seconds, so the per-call cost is the `gh` process, not the Board.
- `orbit github create` with 100 tasks worked because it touches GitHub only for new Issues.

Cause: `cmd_status` loops `graph.iter()` and calls `client.get_issue` (and `repository_info`/`label_exists`) one at a time; `synchronize` does the same loop. Every call spawns `gh api`, so 582 links means 582 sequential subprocess round trips.

Acceptance criteria:

- [ ] A read-only listing of linked Issues uses batched GraphQL (or bounded-concurrency REST) instead of one `gh` process per link.
- [ ] `orbit github status` on the 582-link teramigo Board completes in under 60 seconds.
- [ ] `orbit github sync` on the same Board completes without timing out, and a single-Task sync is not slower than today.
- [ ] Concurrency is bounded and respects GitHub rate limits; on a 403/secondary-limit response the run reports which Tasks were not checked instead of failing the whole command.
- [ ] `orbit github status --repo <owner/repo>` (or equivalent scoping) limits the work to one repository.
- [ ] Documented in `docs/github-sync.md` with the new timing expectation.
