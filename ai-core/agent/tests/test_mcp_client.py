from __future__ import annotations

import asyncio
import json
import tempfile
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest

from osaima_agent.mcp_client import McpClient, McpError

TOOLS = [
    {
        "name": "get_volume",
        "title": "Get volume",
        "description": "Volume",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
    }
]


class FakeDaemon:
    """Speaks just enough MCP; can drop the connection after N replies."""

    def __init__(self, drop_after: int | None = None) -> None:
        self.drop_after = drop_after
        self.methods: list[str] = []
        self.connections = 0

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.connections += 1
        replies = 0
        while line := await reader.readline():
            msg = json.loads(line)
            self.methods.append(msg["method"])
            if "id" not in msg:
                continue
            result: Any
            if msg["method"] == "initialize":
                result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}
            elif msg["method"] == "tools/list":
                result = {"tools": TOOLS}
            elif msg["params"]["name"] == "broken":
                result = {"content": [{"type": "text", "text": "it broke"}], "isError": True}
            elif msg["params"]["name"] == "invalid":
                reply = {
                    "jsonrpc": "2.0",
                    "id": msg["id"],
                    "error": {"code": -32602, "message": "bad pid"},
                }
                writer.write(json.dumps(reply).encode() + b"\n")
                continue
            else:
                result = {
                    "content": [{"type": "text", "text": "40%"}],
                    "structuredContent": {"percent": 40},
                    "isError": False,
                }
            writer.write(
                json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}).encode() + b"\n"
            )
            await writer.drain()
            replies += 1
            if self.drop_after is not None and replies >= self.drop_after:
                self.drop_after = None
                writer.close()
                return


@pytest.fixture
async def socket_path() -> AsyncIterator[Path]:
    with tempfile.TemporaryDirectory(prefix="osa-") as d:
        yield Path(d) / "mcp.sock"


async def serve(daemon: FakeDaemon, path: Path) -> asyncio.base_events.Server:
    return await asyncio.start_unix_server(daemon.handle, path=str(path))


async def test_handshake_tools_and_calls(socket_path: Path) -> None:
    daemon = FakeDaemon()
    server = await serve(daemon, socket_path)
    client = McpClient(socket_path)
    try:
        tools = await client.list_tools()
        assert tools[0].name == "get_volume" and tools[0].read_only
        assert daemon.methods[:3] == ["initialize", "notifications/initialized", "tools/list"]

        outcome = await client.call_tool("get_volume", {})
        assert outcome.ok and outcome.text == "40%" and outcome.structured == {"percent": 40}

        broken = await client.call_tool("broken", {})
        assert not broken.ok and broken.text == "it broke"

        invalid = await client.call_tool("invalid", {})
        assert not invalid.ok and "bad pid" in invalid.text
    finally:
        await client.aclose()
        server.close()


async def test_reconnects_after_the_daemon_drops_the_connection(socket_path: Path) -> None:
    daemon = FakeDaemon(drop_after=2)  # initialize + tools/list, then hang up
    server = await serve(daemon, socket_path)
    client = McpClient(socket_path)
    try:
        await client.list_tools()
        outcome = await client.call_tool("get_volume", {})
        assert outcome.ok
        assert daemon.connections == 2
    finally:
        await client.aclose()
        server.close()


async def test_missing_daemon_is_a_clear_error(socket_path: Path) -> None:
    client = McpClient(socket_path)
    with pytest.raises(McpError, match="not running"):
        await client.list_tools()
