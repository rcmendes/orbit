# Comments: orbit-7r3q4c

<!-- orbit:comment {"author":"opencode","created_at":"2026-10-29T13:47:31.547463Z"} -->
Fixed both command-level causes. workspace migrate now stages a new Board from the legacy Workspace Board's config (carrying [github], [workspace], and board policy; adapting only id.prefix, board.storage, and board.branch) and reports configuration_source; dry-run/apply verified on a copy of Teramigo's 556-task legacy board produces the original [github] block. workspace refresh now infers [github].organization when every child origin shares one owner, persists it, and reports organization/organization_inferred; mixed owners still require explicit config. Added migration and refresh tests; docs, help, SPEC, ARCHITECTURE, README, and CHANGELOG updated. Full suite: 475 tests pass; clippy and fmt clean. Commit 55bd446.
<!-- /orbit:comment -->
