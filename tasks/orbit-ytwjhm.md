---
title: GitHub integration UX hardening
status: done
type: epic
priority: 2
labels:
- enhancement
created_at: 2026-10-28T15:23:33.240523Z
updated_at: 2026-10-28T16:48:27.352747Z
---

## Goal

Make GitHub setup, status, sync, and Triage understandable and safe for humans and agents while preserving Orbit's local-first model.

## Scope

- One-command GitHub connection with detection and validation.
- Read-only status and diagnostics.
- Human-readable sync plans and conflict output.
- Issue URL support and sensible repository defaults.
- Interactive Triage review for humans.
- Explicit basic mode for personal GitHub accounts.
- Opt-in Issue Field creation when permissions allow.
- Actionable remediation errors.
- Release the feature with a binary capability check.

## Non-goals

- Full Matt Pocock triage parity.
- PR-as-a-request-surface support.
- Changing GitHub ownership of title, body, or open/closed state.
- Silently falling back from Issue Fields to body-text conventions.

## Completion criteria

A new user can connect a supported GitHub repository without editing TOML, understand the current sync state and conflicts, approve Triage changes interactively, use a documented personal-account basic mode, and recover from setup errors without guessing.
