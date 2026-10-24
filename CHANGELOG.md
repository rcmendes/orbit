# Changelog

## Unreleased

## 0.2.0 - 2026-10-23

- `orbit setup` now installs MCP configuration by default for Claude, Cursor, Codex, and OpenCode. MCP entries record the absolute path of the running Orbit binary, so editors launched outside a shell no longer need `~/.cargo/bin` on `PATH`. Use `--no-mcp` to install instruction files only, or `--mcp-bin orbit` for a portable command that relies on the editor's `PATH`.
- Labels on tasks, with configured hold labels excluded from readiness and claims.
- Append-only per-task comments with author and timestamp, available through `orbit comment`.
- `orbit label`, `list --label`/`--unlabeled`, and `doctor` checks for comments.
- MCP `label` and `comment` tools; 13 tools total.
- Triage integration: `docs/agents/triage-labels.md` and updated issue-tracker guidance.

## 0.1.0 - 2026-10-23

- Initial pre-alpha: Markdown board on a dedicated Git branch, atomic claims and leases, strict JSON CLI, agent setup recipes, and the stdio MCP server.
- Release tooling: cargo-dist, shell/PowerShell installers, and the `@rcmendes/orbit` npm package.
