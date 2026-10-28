---
title: Discover and manage Workspace repositories
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.84hvkj
parent: orbit-f1hqst
area: setup
claimed_by: rodrigomendes
claimed_at: 2026-10-28T23:10:04.529162Z
lease_until: 2026-10-28T23:40:04.529162Z
created_at: 2026-10-28T22:47:25.108099Z
updated_at: 2026-10-28T23:10:04.529162Z
---

## Goal

Make direct child GitHub repositories members of the Workspace without requiring per-repository linking.

## Acceptance criteria

- Scan direct child directories for Git repositories and read their GitHub `origin` remotes.
- Include eligible repositories owned by the configured Workspace organization; report unsupported owners, non-Git directories, missing origins, and access failures clearly.
- Support explicit include/exclude overrides and show discovered, included, excluded, inaccessible, stale, and removed repositories in `orbit workspace status` or equivalent status output.
- A newly cloned child repository is discovered on the next explicit refresh/reconcile, not by background polling.
- A missing or inaccessible repository becomes stale/unavailable; preserve its cached Issues until explicit detachment.
- Full mode remains one organization per Workspace. Do not create Tasks or Issues during repository discovery.

## Verification

- Integration tests cover six sibling repos, extra non-repo folders, excluded repos, inaccessible origins, stale repos, and adding a new child repo.
