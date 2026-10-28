---
title: Add structured agent metadata and shared project guidance
status: in_progress
type: task
priority: 2
parent: orbit-6pm4p0
claimed_by: implementer-2m09a7
claimed_at: 2026-10-28T14:27:19.795011Z
lease_until: 2026-10-28T14:57:19.795011Z
created_at: 2026-10-28T14:07:19.676388Z
updated_at: 2026-10-28T14:27:19.795011Z
---

## Goal

Make stable task classification and verification guidance explicit, queryable, and available to Orbit CLI/MCP agents instead of relying on undocumented frontmatter or inconsistent label conventions.

## Acceptance criteria

- Add an optional single-value `area` classification with controlled, documented values and filtering support.
- Add optional task-level `verification` guidance for concise checks/commands, distinct from narrative acceptance criteria in the task body.
- Surface both fields consistently in task detail, JSON projections, CLI and MCP create/update/read workflows; validate values and preserve them through ordinary mutations.
- Provide a checked-in repository guidance file for shared project-wide agent conventions; task verification can add to or override the applicable project defaults.
- Keep personal preferences and transient agent execution state out of these durable shared fields.
- Document the semantics and cover storage, filtering, CLI/MCP, and serialization behavior with tests.
