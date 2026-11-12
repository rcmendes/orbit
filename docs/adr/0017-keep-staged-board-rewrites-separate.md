# Keep staged Board rewrites separate

> **Amended by [ADR 0019](./0019-board-folder-is-its-own-repository.md):**
> `orbit workspace migrate` is retired; the second whole-Board rewrite is now
> `orbit migrate` (`src/board_migrate.rs`), which consolidates legacy Boards
> while converting them to standalone repositories. The separation decision
> stands.

Orbit rewrites a whole Board in two places: `orbit slug migrate` (`src/migrate.rs`) and `orbit workspace migrate` (`src/workspace_migrate.rs`). Both flush heartbeat-only writes, build the new Board off to the side, commit, swap, and clean up. Each keeps its own Git plumbing and failure ladder, and they recover differently. Slug migration uses a detached worktree and `reset_hard`, and rewrites IDs in one commit (ADR 0011). Workspace migration uses a stage directory with a JSON state file, and keeps the source Boards as backups.

We keep the two flows as separate modules and do not extract a shared "rewrite the Board with rollback" module. Two flows that recover differently would make a shared module's interface carry both recovery models, so it would add no depth. It would hide the difference that each flow's correctness depends on.

## Considered options

- **One staged-rewrite module with pluggable recovery.** It removes repeated "stage, commit, swap" steps, but callers would choose a recovery mode, and the mode is most of the logic. The interface would be about as large as the code behind it.
- **Route both flows through `Board`.** `Board` does not own whole-Board replacement today, and adding it widens `Board` for two callers.

## Consequences

The repetition between the two flows is accepted. Revisit this decision when a third whole-Board rewrite appears, or when the two recovery models converge. Then a shared module would have three callers and would earn its interface.
