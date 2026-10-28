---
title: Harden Matt Pocock skill compatibility
status: done
type: epic
priority: 2
labels:
- enhancement
created_at: 2026-10-28T15:19:56.555280Z
updated_at: 2026-10-28T15:52:00.641754Z
---

## Goal

Make Orbit's tracker contract predictable for Matt Pocock's engineering skills without duplicating their workflow intelligence. Orbit remains the tracker; the skills own the full triage state machine, verification, grilling, agent briefs, and out-of-scope handling.

## Scope

- Document the compatibility boundary and the agent-brief handoff convention.
- Align Wayfinder operations with the skill contract: `wayfinder:<type>` child labels and Orbit lease claims instead of GitHub assignees.
- Enforce canonical triage label invariants when `orbit triage` proposes label changes.

## Non-goals

- Reimplement Matt's full triage workflow inside Orbit.
- Add PR-as-a-request-surface support.
- Add an `.out-of-scope/` knowledge base or agent-brief generator to Orbit.
- Achieve 100% feature parity with Matt's skills.

## Completion criteria

The tracker docs state the boundary unambiguously, Wayfinder operations match the skill contract, invalid triage label combinations are rejected, and agents can find the agent-brief convention from the tracker docs.
