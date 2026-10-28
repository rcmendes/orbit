---
title: Add orbit github connect setup wizard
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-ytwjhm
checkpoint:
  by: impl-z30hsp
  at: 2026-10-28T15:33:43.711001Z
  progress: Implemented fixture-backed orbit github connect for origin parsing, gh auth and repository discovery, organization field validation, safe config updates, and personal-account remediation. Integration suite and full cargo test pass.
  next: Review diffs/docs, commit, then merge latest integrate/orbit-ytwjhm into this ticket branch and verify.
area: setup
claimed_by: impl-z30hsp
claimed_at: 2026-10-28T15:27:21.046648Z
lease_until: 2026-10-28T15:57:44.638850Z
created_at: 2026-10-28T15:23:46.720999Z
updated_at: 2026-10-28T15:33:43.711001Z
---

## Goal

Replace the manual GitHub setup sequence with one guided command that detects the environment, explains blockers, and writes a valid Board configuration.

## Scope

- Add `orbit github connect` with human and `--json` output.
- Check `gh auth status` and report missing or insufficient authentication.
- Infer `owner/repo` from `git remote get-url origin`.
- Detect whether the owner is a User or Organization.
- For an Organization, detect `Area` and `Verification` Issue Fields.
- Write the `[github]` config, link the repository, and run validation.
- Offer a clear personal-account path: use an organization-owned repository, use basic mode when available, or cancel.
- Make the command idempotent and safe to rerun.

## Acceptance criteria

- A fresh organization-owned repository can be connected without manually editing `config.toml`.
- Missing `gh` authentication, missing repository access, missing Issue Fields, and personal-account ownership produce clear remediation steps.
- Existing valid configuration is preserved and not duplicated.
- `--json` exposes the detected owner type, organization, repository, field status, and next action.
- A rerun is safe and reports whether anything changed.

## Verification

- Add CLI/integration tests with a GitHub fixture for organization and personal-owner paths.
- Exercise connect against a scratch Board.
