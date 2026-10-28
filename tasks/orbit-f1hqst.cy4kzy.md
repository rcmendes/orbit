---
title: Import Remote Issues into Linked Tasks
status: todo
type: task
priority: 1
labels:
- enhancement
depends_on:
- orbit-f1hqst.a2xpf8
parent: orbit-f1hqst
area: cli
created_at: 2026-10-28T22:48:01.175045Z
updated_at: 2026-10-28T22:48:01.175045Z
---

## Goal

Let users bring selected catalog Remote Issues into Orbit's claimable Task workflow without losing their GitHub identity or creating duplicate work.

## Acceptance criteria

- Import supports one Remote Issue or a reviewed batch from one or more discovered repositories.
- A selected Remote Issue can either create a new Linked Task or link to a specifically selected existing Local Task.
- New Tasks receive a stable GitHub identity and baseline; initialize title/body/labels/fields from the Issue, and map closed Issues to `done` or `cancelled` when `wontfix` applies.
- When linking an existing Local Task, preserve Orbit values as authoritative and preview differences before the user confirms the link; do not overwrite local data silently.
- Reject an Issue already linked to another Task and a Task already linked to another Issue. Matching is by repository and Issue number, never title.
- Import never writes to GitHub. Batch preview reports creates, links, terminal Tasks, skipped duplicates, and errors before confirmation.
- Imported Linked Tasks participate in Orbit readiness/claims according to their mapped status, dependencies, and hold labels.

## Verification

- Fixture-backed tests cover importing open/closed Issues, `wontfix`, linking an existing Local Task, both duplicate directions, batch preview/apply, and unchanged GitHub data.
- Document import and duplicate-resolution behavior.
