# OSAIMA AI Core (`mcp-daemon`)

A [Model Context Protocol](https://modelcontextprotocol.io) server that gives the
shell and AI agents live system context and a small set of safe system actions.

```
osaima-shell ──┐                       ┌── sysinfo (/proc, /sys)
local agents ──┼── JSON-RPC 2.0 ──► mcp-daemon
MCP clients ───┘   (socket or stdio)   └── kill(2) with ownership checks
```

## Running

```bash
cargo build --release
./target/release/mcp-daemon            # listen on the per-user socket
./target/release/mcp-daemon --stdio    # one MCP session on stdin/stdout
OSAIMA_LOG=debug ./target/release/mcp-daemon
```

The socket path is, in order: `$OSAIMA_MCP_SOCKET`,
`$XDG_RUNTIME_DIR/osaima/mcp.sock`, `/tmp/osaima-<uid>/mcp.sock`. The shell
uses the same lookup.

To use it from any MCP client (for example Claude Desktop or an Ollama-based
agent), configure a stdio server with command `mcp-daemon` and args `["--stdio"]`.

## Protocol

Messages are newline-delimited JSON-RPC 2.0, one per line, at most 1 MiB.
Supported MCP revisions: `2025-06-18`, `2025-03-26`, `2024-11-05`.

| Method | Purpose |
|---|---|
| `initialize` | Version negotiation; advertises the `tools` capability |
| `ping` | Liveness check; returns `{}` |
| `tools/list` | Tool definitions with JSON Schemas and annotations |
| `tools/call` | Run a tool; returns `content` (text) and `structuredContent` |
| `system.get_stats`, `system.get_processes` | Pre-MCP methods kept for compatibility |

### Tools

| Tool | Arguments | Notes |
|---|---|---|
| `get_system_stats` | none | OS, kernel, host, uptime, load, CPU %, memory, swap |
| `list_processes` | `limit` (1–200, default 15), `sort_by` (`cpu`\|`memory`) | `cpu_percent` is per core, like `top` |
| `kill_process` | `pid`, `signal` (`TERM` default, `KILL`, `INT`, `HUP`, `STOP`, `CONT`) | Destructive; clients must confirm with the user |
| `list_disks` | none | Mounted filesystems with total and free space |

Example session:

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo","version":"1"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{...}}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_processes","arguments":{"limit":3}}}
← {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"..."}],"structuredContent":{"processes":[...]},"isError":false}}
```

Errors use the standard codes: `-32700` parse error, `-32600` invalid request,
`-32601` unknown method, `-32602` invalid params or unknown tool. A tool that
runs but fails (for example, permission denied) returns `isError: true` with
the reason as text.

## Security model

- The socket is created with mode `0600` inside a `0700` directory, and every
  connection's peer credentials are checked: only the daemon's own user (or
  root) is served.
- `kill_process` never touches PID 0, PID 1 or the daemon itself. Non-root
  callers can only signal processes they own; ownership is read fresh from
  `/proc` at call time.
- Oversized messages close the connection.
- The daemon runs as the session user, never as root.

## Development

```bash
cargo test                  # unit + socket integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt
python tests/interop/mcp_sdk_client.py target/release/mcp-daemon   # needs `pip install mcp`
```

CI runs all of these on every push. If you can't run `cargo fmt` locally, CI
publishes a `rustfmt-patch-daemon` artifact you can apply with `git apply`.
