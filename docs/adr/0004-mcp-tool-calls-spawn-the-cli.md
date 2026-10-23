# MCP tool calls spawn the orbit CLI

Every MCP tool call invokes the installed `orbit` binary with `--json` and returns its envelope as the tool's text content. This guarantees one implementation of argument validation, locking, atomic writes, auto-commit, and error codes; CLI and MCP parity is exact by construction.

The cost is one process spawn per tool call, which is small relative to agent work. If latency ever becomes a bottleneck, replace the spawn with a shared in-process command layer while keeping the same JSON envelope and `isError` contract.
