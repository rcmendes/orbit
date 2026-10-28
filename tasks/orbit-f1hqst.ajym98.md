---
title: Migrate existing Boards into the Workspace Board
status: todo
type: task
priority: 1
labels:
- enhancement
depends_on:
- orbit-f1hqst.84hvkj
parent: orbit-f1hqst
area: board
created_at: 2026-10-28T22:47:25.015063Z
updated_at: 2026-10-28T22:47:25.015063Z
---

## Goal

Move existing Orbit work into the new Workspace Board without losing task history or creating two active Boards.

## Acceptance criteria

- Preview migration from the existing Teramigo `.board` and any child-repository Boards before writing.
- Transfer all Tasks, comments, labels, metadata, claims/checkpoints where valid, parent/subtask edges, and dependencies into `.orbit/board`.
- Preserve IDs unless a collision exists; on collision, remap IDs and all structured references atomically.
- Detect duplicate GitHub identities (`owner/repo#number`) and stop for an explicit user resolution; never merge by title/body similarity.
- Workspace configuration takes precedence over conflicting source Board configuration; report each differing setting.
- Keep source Boards intact as backups and switch the migrated Workspace/member repos to the Workspace Board only after successful validation.
- Support dry-run, resumable application, rollback guidance, and a post-migration `orbit doctor` check.

## Verification

- Integration tests cover clean migration, Task-ID collisions, duplicate GitHub links, config conflicts, interruption/resume, and preserved comments/edges.
- Verify source Boards remain unchanged after migration.
