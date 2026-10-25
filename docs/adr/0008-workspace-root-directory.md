# Workspace root and `orbit workspace`

> **Replaced by [ADR 0012](./0012-orbit-folder.md).** `orbit workspace` and the
> `<workspace>/board` layout no longer exist; a workspace is an Orbit folder
> (`DIR`) plus the repositories linked to it. This document is kept for history.

A Workspace is anchored to a directory that contains its Board at `board/`; member repositories may live inside that directory or elsewhere. `orbit workspace <folder>` creates `<folder>/board` as an external Board (its own Git repository on the `orbit` branch) and does nothing else: it does not clone or link repositories. Each repository joins with `orbit link <folder>`, which resolves the workspace root to its `board/` and writes the pointer in the repository's Git common directory. A workspace root inside a Git working tree is rejected. There is no registry and no manifest; the directory convention and the per-repository pointer are the only state.

## Considered options

- **A Board inside one member repository's `.git`.** Ties the Workspace to that repository's lifecycle; deleting or re-cloning it loses the Board.
- **A user-level `~/orbit/<name>` Board.** Survives workspace moves but detaches the Board from the workspace and invites name collisions; [ADR 0006](./0006-shared-board-across-repositories.md) rejected global state.
- **A workspace manifest or registry.** Makes membership explicit, but adds a second source of truth next to the link pointers; ADR 0006 rejected a user-level registry for v1.
- **`orbit workspace` also cloning or linking repositories.** Convenient, but mixes creation with joining and makes the command mutate repositories the user did not name.

## Consequences

- The Board survives deleting or re-cloning any member repository, and a collector has one stable path: `<workspace>/board`.
- Link pointers are absolute; moving the workspace breaks them until each repository re-links. Relative pointers are a follow-up, not part of this decision.
- A fresh clone of a member repository has no link; `orbit link <workspace>` is an explicit onboarding step.
- `orbit link` accepts either a workspace root or a Board root, so the same command works for both layouts.
