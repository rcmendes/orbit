---
title: Add Workspace-root initialization and Board discovery
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-f1hqst
area: board
claimed_by: rodrigomendes
claimed_at: 2026-10-28T22:50:33.806362Z
lease_until: 2026-10-28T23:20:33.806362Z
created_at: 2026-10-28T22:47:09.472535Z
updated_at: 2026-10-28T22:50:33.806362Z
---

## Goal

Let a user initialize and use Orbit from a non-Git Workspace folder while keeping the actual Board storage internal.

## Acceptance criteria

- `orbit init <workspace-root>` creates the Workspace's internal `.orbit/board` Board layout; `.orbit/cache` is reserved for the separate rebuildable catalog cache.
- The Workspace root—not `.orbit/board`—is the user-facing path for `--dir` and `ORBIT_DIR`.
- Commands invoked at the Workspace root or inside a discovered child Git repository resolve the same Board, including across the child repository's Git boundary.
- Child repositories inside the Workspace need no per-repository `orbit link`; repositories outside it can still be explicitly linked.
- An existing child-repository Board or conflicting `.orbit` layout is reported; initialization never silently shadows or discards it.
- Update the Workspace/Board vocabulary and record the new path contract; retain compatibility or a clear migration diagnostic for existing Board layouts.
- Tests cover initialization at a non-Git root, resolution from each nested-repo level, explicit path overrides, and ambiguous/local Board conflicts.

## Verification

- `cargo test --test discovery`
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
