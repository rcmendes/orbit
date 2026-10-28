---
title: Add reusable agent Triage review and apply workflow
status: todo
type: task
priority: 2
depends_on:
- orbit-6pm4p0.2x66x3
- orbit-6pm4p0.2m09a7
parent: orbit-6pm4p0
created_at: 2026-10-28T14:07:27.390594Z
updated_at: 2026-10-28T14:07:27.390594Z
---

## Goal

Provide a reusable agent workflow, inspired by ticket-triage skills, that reviews Orbit-linked GitHub Issues and proposes actionable metadata updates.

## Acceptance criteria

- Triage reviews only Tasks already linked to GitHub Issues; adopting an issue remains explicit.
- It reads task/project guidance and proposes changes to the controlled `area`, task-level `verification`, and canonical Orbit triage/category labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`, `bug`, `enhancement`).
- It presents a reviewable preview and requires explicit approval before applying any proposal; it does not create or apply arbitrary labels.
- Approval writes changes to Orbit first. A separate explicit sync publishes approved field/label changes to GitHub.
- It does not rewrite GitHub-owned title, body, or open/closed status, and it does not execute proposed verification commands.
- The workflow explains uncertainty and leaves metadata unchanged when the ticket lacks enough evidence.
- Include tests or workflow validation and document how agents invoke Triage, review proposals, approve changes, and sync them.
