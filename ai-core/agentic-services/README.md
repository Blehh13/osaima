# Interstellar OS Agentic Services

This directory contains the AI agents that will operate within the Interstellar OS environment.

## Architecture

Agentic services run independently and communicate with the OS core via the **MCP Daemon** (`/tmp/interstellar_mcp.sock`).

The `mcp-daemon` acts as the standard interface for:
- Fetching system context (Memory, CPU, Window state)
- Requesting system modifications (Closing windows, changing settings)
- Registering tools that local or cloud LLMs can use

## Developing an Agent

Agents can be written in any language (Python and Node.js are recommended due to strong LLM ecosystem support). 
They simply need to connect to the Unix Domain Socket and communicate using standard JSON-RPC 2.0 formatting as defined by the Model Context Protocol.

### Example (Python Pseudo-code)
```python
import socket
import json

client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect("/tmp/interstellar_mcp.sock")

req = {
    "jsonrpc": "2.0",
    "id": "1",
    "method": "system.get_stats"
}
client.sendall((json.dumps(req) + "\n").encode())
response = client.recv(4096)
print(json.loads(response))
```
