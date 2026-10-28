---
title: Document Matt Pocock compatibility boundary and agent-brief handoff
status: done
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
parent: orbit-dkyngc
checkpoint:
  by: implement-dkyngc-z1wgx4
  at: 2026-10-28T15:33:47.528206Z
  progress: Documented the Orbit vs Matt Pocock triage boundary and added docs/agents/agent-brief.md. Commit 5d8906e on ticket/orbit-dkyngc.z1wgx4. Wayfinding command examples and the Errors table were left for the other tickets. integrate/orbit-dkyngc was already contained; no merge conflict.
  next: merge to integrate/orbit-dkyngc
area: docs
created_at: 2026-10-28T15:20:07.774349Z
updated_at: 2026-10-28T15:51:50.388171Z
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
