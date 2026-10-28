---
title: Improve GitHub sync plan and conflict output
status: done
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-ytwjhm
checkpoint:
  by: impl-r290vd
  at: 2026-10-28T15:32:04.061635Z
  progress: Implemented and tested human-readable GitHub sync dry-run plans for empty, clean, pull, push, and conflict cases; JSON structure remains covered.
  next: Review docs/diff, commit, merge latest integrate/orbit-ytwjhm, then close ticket.
area: cli
created_at: 2026-10-28T15:23:46.555436Z
updated_at: 2026-10-28T15:32:41.143458Z
---

## Goal

Make the default `orbit github sync` dry run useful to a human without requiring `--json`.

## Scope

- Render a readable plan showing each linked Task, its Issue, pull changes, push changes, and conflicts.
- Show field-level conflict details, including the local and remote values.
- Keep `--json` machine-readable and backward compatible.
- Keep `--apply` explicit; do not auto-apply from the preview.
- Make an empty plan and an all-clear result easy to recognize.

## Acceptance criteria

- A human can identify which fields will change in each direction.
- Conflicts name the affected field and both competing values.
- The preview makes it clear that nothing was written.
- `--json` output retains the existing structured plan and conflict data.
- Tests cover a clean plan, a pull-only plan, a push-only plan, and a conflicting plan.

## Verification

- Add or update integration tests for human and JSON output.
- Run a dry run against the GitHub fixture.
