---
title: Remove Matt Pocock from orbit init agent discovery and selection
status: done
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
area: setup
created_at: 2026-10-31T12:45:31.323349Z
updated_at: 2026-10-31T12:49:11.961156Z
---

## Goal

Make `orbit init` treat Matt Pocock as a skills/plugin compatibility integration, never as a coding-agent harness.

## Acceptance criteria

- `orbit init --agents auto`, `--agents all`, `--agents ask`, and interactive detection consider only Claude Code, Cursor, Codex, and OpenCode.
- `orbit init --agents mattpocock` is rejected as an unknown harness; `orbit setup mattpocock` remains available as an explicit compatibility command.
- `--agents all` does not write the Matt tracker/triage guidance.
- Help and documentation no longer describe Matt as an init-detected agent, while retaining the separate compatibility setup documentation.
- Tests cover init auto/all/interactive detection, rejection of Matt as an init harness, and the unchanged explicit setup path.

## Verification

- `cargo test --test onboarding`
- `cargo test --test setup`
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
