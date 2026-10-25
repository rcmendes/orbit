# Markdown board on a dedicated orphan branch

Orbit stores Tasks as Markdown files on a dedicated `orbit` branch, checked out in a hidden worktree under `<git-common-dir>/orbit/board` and shared by every worktree of the repository. The board is auto-committed on meaningful mutations, so `git log orbit` is the task audit trail and task changes never enter code PRs. `ORBIT_DIR` can point at an external board root for monorepos or local-only use.

> **Extended by ADR 0006:** a repository may link to an external Board shared with related repositories; the local worktree remains the default.

> **Amended by [ADR 0012](./0012-orbit-folder.md):** the local Board is checked out at `<project>/.orbit` instead of `<git-common-dir>/orbit/board`, and a Board outside every working tree is its own repository. The orphan branch, shared worktree, and Git-reviewable history are unchanged.

## Considered options

- Committing `.orbit/` on the code branch: ties the board to whichever branch is checked out and puts task churn into code PRs.
- A Dolt or SQLite database: breaks human-readable diffs and the Git-native promise.
- A per-worktree board: agents diverge and cannot coordinate.

## Consequences

Task history is separate from code history. Orbit owns a small amount of Git plumbing, and the board branch must not be based on code history. A future `orbit export` can produce snapshots for review.
