---
title: Add actionable GitHub remediation errors
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.z30hsp
- orbit-ytwjhm.v8vgqe
parent: orbit-ytwjhm
area: cli
claimed_by: impl-63nzes
claimed_at: 2026-10-28T16:06:52.480142Z
lease_until: 2026-10-28T16:36:52.480142Z
created_at: 2026-10-28T15:24:06.082078Z
updated_at: 2026-10-28T16:06:52.480142Z
---

## Goal

Turn GitHub and Triage failures into clear next actions instead of bare error codes.

## Scope

- Improve `GITHUB_NOT_CONFIGURED`, `GITHUB_SETUP_INCOMPLETE`, `GITHUB_REPO_NOT_LINKED`, and `GITHUB_ISSUE_CONFLICT`.
- Improve `TRIAGE_NOT_LINKED`, `TRIAGE_APPROVAL_REQUIRED`, and `TRIAGE_INVALID_PROPOSAL` where the fix is not obvious.
- Include the cause, the affected organization/repository/field, and the exact command or UI action to resolve it.
- Include remediation guidance in JSON error details without breaking the existing error envelope.
- Avoid exposing tokens or sensitive authentication data.

## Acceptance criteria

- Missing Issue Fields name the field and the exact UI path or setup command.
- Unlinked repositories name the repository and the `orbit github repo link` command.
- Conflicts name the Task, Issue, and conflicting fields.
- Triage errors state whether the Task is unlinked, approval is missing, or the proposal is invalid.
- Human and JSON outputs remain consistent.
- Tests assert the remediation text or structured remediation data for each error family.

## Verification

- Add integration tests for each improved error family.
- Review messages for actionable, non-sensitive content.
