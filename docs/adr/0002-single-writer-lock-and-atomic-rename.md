# Serialize writes with one repo-wide lock and atomic renames

Every mutation takes a non-blocking exclusive lock on `<git-common-dir>/orbit/lock`, performs its read-modify-write, and publishes each file by writing a temp file in the same directory, fsyncing it, and renaming it over the target. Readers take a shared lock for the duration of a command so multi-file reads see a consistent snapshot. The lock is runtime state and is never committed.

> **Amended by [ADR 0012](./0012-orbit-folder.md):** the single data lock moved to `<DIR>/.lock`, inside the Orbit folder, in both storage modes. Board creation uses a separate short-lived bootstrap lock that never covers task writes. The single-writer and atomic-rename rules are unchanged.

## Considered options

- Per-file advisory locks: cannot enforce cross-file invariants such as dependency satisfaction, cycle freedom, and delete cascades; a rename also swaps the inode.
- `set_len(0)` rewrites: not crash-safe; a crash leaves a truncated Task file.
- A daemon or database: adds a process or a second source of truth.

## Consequences

Writers serialize, which is acceptable because task mutations are short and rare relative to agent work. `LOCKED` is a retryable error. Locks are released on process exit, so there is no stale-lock cleanup. Network filesystems are unsupported. The implementation uses the standard library's file locks (Rust 1.89+), which map to `flock` on Unix and `LockFileEx` on Windows.
