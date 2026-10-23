# Hand-rolled MCP JSON-RPC over stdio

Orbit implements the MCP subset it needs directly: newline-delimited JSON-RPC 2.0 over stdio, with `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`, `resources/templates/list`, and `resources/read`.

The official `rmcp` SDK was rejected because it would add a Tokio runtime and SDK version coupling for a protocol surface this small. A synchronous loop keeps the binary lean and the framing explicit. The cost is tracking protocol changes manually; the MCP integration tests cover initialization, tool calls, resources, and errors over a real stdio child process.
