# One Board may span repositories

A Workspace is one Board plus every Repository linked to it, so a product backlog can span several repositories without copying Tasks between Boards. A Repository selects a non-local Board through an explicit pointer in its Git common directory, written by `orbit link`; `--dir` and `ORBIT_DIR` still override it, and a Repository with no pointer uses the Board inside its Git common directory. Cross-repository Dependencies work because every linked Repository shares one graph.

> **Amended by [ADR 0012](./0012-orbit-folder.md):** the pointer target is the Orbit folder `DIR`, not a board root or workspace root. A Repository without a pointer resolves the nearest `.orbit`, then the main checkout's `.orbit`. The only user-level Board is the opt-in `~/.orbit` fallback.

## Considered Options

- **Per-repository Boards only.** Rejected: related repositories cannot share a queue or express cross-repository Dependencies.
- **Hydrating foreign Tasks into a hub Board (the Beads model).** Rejected: it duplicates the source of truth and inherits one-way staleness, orphaned copies, and inert cross-prefix Dependencies.
- **Board as a Git submodule.** Rejected: a submodule pins a commit, while the Board commits on every write. The pin is stale immediately, leaving permanent dirty state or, with `ignore = all`, a stale checkout on fresh clones.
- **A user-level registry of named Boards.** Rejected for v1: it introduces global state that Orbit otherwise avoids. A `orbit board clone` helper can provide the same portability by writing the pointer.

## Consequences

- A Board shared across Repositories is versioned in its own repository, not in any linked Repository.
- Linking a Repository that already has a local Board shadows that Board; the local Tasks remain in place and the command must say so.
- Originally, agent configuration recorded the resolved Board path, so linking after `setup` could leave those configs stale. **Superseded by ADR 0012:** generated configs no longer record a Board path; `link` refreshes existing managed integrations as a version reconciliation.
