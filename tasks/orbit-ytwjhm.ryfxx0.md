---
title: Release GitHub integration and add capability checks
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.z30hsp
- orbit-ytwjhm.r290vd
- orbit-ytwjhm.0nbgfx
- orbit-ytwjhm.2c9xqh
- orbit-ytwjhm.v8vgqe
- orbit-ytwjhm.q567ya
- orbit-ytwjhm.7x1cqh
- orbit-ytwjhm.63nzes
parent: orbit-ytwjhm
checkpoint:
  by: impl-ryfxx0
  at: 2026-10-28T16:32:56.716763Z
  progress: 'Implementation and docs are committed (47fd12b + afb5630); verified broad tests, no-default capability checks, artifact smoke, dist metadata, and setup template. Final action: merge latest integrate/orbit-ytwjhm, resolve conflicts if any, then rerun checks.'
  next: Merge latest integration branch into ticket branch; preserve both changes and verify clean status before reporting.
area: release
claimed_by: impl-ryfxx0
claimed_at: 2026-10-28T16:21:24.532555Z
lease_until: 2026-10-28T17:02:48.542612Z
created_at: 2026-10-28T15:24:14.709930Z
updated_at: 2026-10-28T16:32:56.716763Z
---

## Goal

Make the GitHub provider and Triage workflow available in released binaries, and make version or capability mismatches obvious.

## Scope

- Include `orbit github` and `orbit triage` in the next release artifacts.
- Update `CHANGELOG.md`, `README.md`, and the GitHub setup documentation for the released UX.
- Add onboarding and smoke coverage for connect, status, adopt/create, sync preview/apply, and Triage review.
- Add a capability check for Boards with `[github]` configuration when the running binary does not support the provider.
- Document the minimum Orbit version required for GitHub sync.
- Ensure installers and package metadata do not ship a binary without the feature.

## Acceptance criteria

- A released binary exposes `orbit github` and `orbit triage`.
- The documented quickstart works against a fresh Board.
- CI smoke tests exercise the GitHub fixture without calling the network.
- A Board with GitHub configuration produces a clear upgrade message when the binary lacks support.
- Release notes state the minimum version and any personal-account basic-mode limitations.

## Verification

- Run the release smoke suite on supported platforms.
- Run `orbit doctor` against a configured fixture Board.
