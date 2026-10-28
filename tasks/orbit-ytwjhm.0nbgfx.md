---
title: Accept GitHub Issue URLs and default repository selection
status: done
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
parent: orbit-ytwjhm
checkpoint:
  by: impl-0nbgfx
  at: 2026-10-28T15:28:28.629628Z
  progress: Verified ticket branch and integration base; identified CLI args, GitHub command handlers, config selection, integration test harness, and docs. Implementing CLI-level tests at the public command seam for both adopt forms and repository selection.
  next: Add failing GitHub CLI integration tests, implement parsing/default selection, then update docs and verify.
area: cli
created_at: 2026-10-28T15:23:46.658254Z
updated_at: 2026-10-28T15:36:28.133354Z
---

## Goal

Remove unnecessary repository and issue-number typing from common GitHub workflows.

## Scope

- Let `orbit github adopt <task> <github-issue-url>` parse `owner/repo` and the issue number.
- Keep the existing `--repo` and `--number` form working.
- Let `orbit github create <task>` use the only linked repository when exactly one exists.
- Keep `--repo` required when multiple repositories are linked or no repository is linked.
- Reject malformed URLs and mismatched repository/number inputs with actionable errors.

## Acceptance criteria

- A standard GitHub Issue URL works with `adopt`.
- The legacy `--repo`/`--number` form remains compatible.
- `create` works without `--repo` when exactly one repository is linked.
- Ambiguous or missing repository selection produces a clear error.
- Tests cover URL parsing, one linked repository, multiple linked repositories, and malformed URLs.

## Verification

- Add CLI/integration tests for both adopt forms and both create repository cases.
