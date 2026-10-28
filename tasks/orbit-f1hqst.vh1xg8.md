---
title: Set up coding agents across Workspace repositories
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.84hvkj
- orbit-f1hqst.r02zj4
parent: orbit-f1hqst
checkpoint:
  by: rodrigomendes
  at: 2026-10-28T23:33:49.729900Z
  progress: Workspace agent setup implementation is running in workspace-agent-setup; prerequisite init and repo discovery are merged.
  next: Review, merge, close ticket.
area: setup
claimed_by: rodrigomendes
claimed_at: 2026-10-28T23:20:47.097557Z
lease_until: 2026-10-29T00:03:44.176094Z
created_at: 2026-10-28T22:47:34.547399Z
updated_at: 2026-10-28T23:33:49.729900Z
---

## Goal

Make Orbit instructions and tools available when Claude Code, Codex, Cursor, or OpenCode starts in any child repository of the Workspace.

## Acceptance criteria

- Workspace setup detects the four supported harnesses across discovered child repos and offers a preview/selection; `--agents all` can configure all four.
- Write or update managed, harness-native instructions and MCP entries in each selected child repo, all resolving the shared Workspace Board without absolute Board paths.
- Maintain Workspace-level `AGENTS.md` guidance and Claude Code `CLAUDE.md` entry points; preserve hand-written content outside Orbit-managed sections and report collisions rather than overwriting.
- When a new child repo is discovered, a Workspace reconcile/setup action previews and installs its selected harness integrations.
- Detect Matt Pocock skills separately from agent harnesses. If present, maintain the Orbit tracker and triage-label guidance they consume without requiring `orbit setup mattpocock` or treating the plugin as an agent.
- Re-running setup is idempotent; removal affects only Orbit-managed content.

## Verification

- Integration tests cover all four harness generators, nested Git repositories, shared Board resolution, idempotence, custom-file preservation, new-repo reconciliation, and Matt-skill detection.
