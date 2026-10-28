---
title: Add multi-repository GitHub Issue provider and bidirectional sync
status: in_progress
type: task
priority: 2
parent: orbit-6pm4p0
claimed_by: implementer-2x66x3
claimed_at: 2026-10-28T14:27:40.202284Z
lease_until: 2026-10-28T14:57:40.202284Z
created_at: 2026-10-28T14:07:19.613572Z
updated_at: 2026-10-28T14:27:40.202284Z
---

## Goal

Connect an Orbit Board to explicitly linked GitHub repositories and synchronize durable Orbit Tasks with GitHub Issues without losing Orbit-specific workflow semantics.

## Acceptance criteria

- A Board can explicitly link multiple GitHub repositories; issue identity is repository plus issue number, not title or body text.
- Creating a durable Orbit Task can create its GitHub Issue, and an existing GitHub Issue can be explicitly adopted. No repository-wide bulk import occurs by default.
- GitHub is authoritative for shared title, body, and open/closed state. One Orbit Task maps to one GitHub Issue.
- GitHub organization-level Issue Fields are used for Orbit-owned `area` and `verification` values. Required field setup is validated and missing configuration is reported clearly.
- Labels synchronize both ways. Sync detects field-level conflicts and never silently overwrites concurrent changes; sync remains explicit.
- GitHub open Issues preserve Orbit todo/in_progress state; closed Issues map to done unless the `wontfix` label maps to cancelled; reopening maps to todo.
- Orbit parent/SubTask relationships and dependencies map to GitHub sub-issues and blocked-by/blocking relationships where supported. Unsupported hierarchy or cross-repository relationships are surfaced clearly.
- GitHub assignees are not treated as Orbit claims.
- Setup, repository linking, issue adoption, sync behavior, and conflict resolution are documented and tested.
