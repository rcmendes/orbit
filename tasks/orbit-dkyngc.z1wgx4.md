---
title: Document Matt Pocock compatibility boundary and agent-brief handoff
status: in_progress
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
parent: orbit-dkyngc
area: docs
claimed_by: implement-dkyngc-z1wgx4
claimed_at: 2026-10-28T15:31:03.193550Z
lease_until: 2026-10-28T16:03:25.651822Z
created_at: 2026-10-28T15:20:07.774349Z
updated_at: 2026-10-28T15:33:25.651822Z
---

## Goal

Make the division of responsibility between Orbit and Matt Pocock's engineering skills explicit, and give agents a discoverable handoff to the agent-brief convention.

## Scope

- Update `docs/agents/issue-tracker.md` and `docs/agents/triage-workflow.md`.
- State that Orbit is the tracker. Matt's skills own the full triage state machine: context gathering, verification, grilling, agent briefs, and out-of-scope handling.
- State that `orbit triage` is a deterministic metadata-enrichment primitive, not a replacement for Matt's `triage` skill.
- Add `docs/agents/agent-brief.md` with a concise agent-brief template and a pointer to the convention.
- Reference the agent-brief document from the tracker and triage workflow docs.
- Update `README.md` only if the existing compatibility description becomes misleading.

## Acceptance criteria

- A reader can tell which responsibilities belong to Orbit and which belong to Matt's skills.
- The docs do not claim full triage parity or imply that `orbit triage` verifies claims, grills, writes agent briefs, or maintains `.out-of-scope/`.
- The agent-brief template is discoverable from the tracker documentation.
- `orbit triage` is described as metadata-only and Orbit-first.
- Links remain valid and the docs match current command behavior.

## Verification

- Review the rendered Markdown for contradictions and broken links.
- Confirm the new document is referenced from the tracker and triage workflow docs.
