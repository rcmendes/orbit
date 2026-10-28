---
title: Detach and reattach Workspace repositories and Linked Tasks
status: todo
type: task
priority: 2
labels:
- enhancement
depends_on:
- orbit-f1hqst.a2xpf8
- orbit-f1hqst.b2w48k
parent: orbit-f1hqst
area: cli
created_at: 2026-10-28T22:48:24.498420Z
updated_at: 2026-10-28T22:48:24.498420Z
---

## Goal

Let users intentionally stop syncing a repository or one Linked Task without losing local work, cached history, or remote GitHub data.

## Acceptance criteria

- Detach is available for stale/unavailable or healthy repositories and for individual Linked Tasks.
- Before applying, preview the repositories, Linked Tasks, and cached Remote Issues affected; require explicit confirmation.
- Detaching converts affected Linked Tasks to Local Tasks that behave as if local-only. Preserve all local data and retain the prior GitHub identity only as inactive provenance.
- Keep associated Remote Issue snapshots in a non-claimable archive; do not close, delete, or otherwise mutate GitHub Issues during detach.
- Reattach matches inactive provenance by exact repository/Issue number, shows differences and conflicts, and requires confirmation before resuming sync.
- Repeated detach/reattach is safe and idempotent; no duplicate Tasks or links are created.

## Verification

- Tests cover repository/task detach, stale and healthy states, cache retention, no GitHub writes, reattach with no changes, reattach with conflicts, and repeated operations.
- Document local-only lifecycle and reattachment behavior.
