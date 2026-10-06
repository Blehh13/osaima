"""Client for the OSAIMA AI Core (MCP over a Unix socket, newline-delimited JSON-RPC)."""

from __future__ import annotations

import asyncio
import itertools
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import __version__
from .llm import ToolSpec

PROTOCOL_VERSION = "2025-06-18"
DEFAULT_REQUEST_TIMEOUT_S = 30.0


class McpError(RuntimeError):
    """The AI Core is unreachable or returned a JSON-RPC error."""

    def __init__(self, message: str, code: int | None = None) -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True)
class ToolOutcome:
    ok: bool
    text: str
    structured: Any = None


class McpClient:
    """One persistent session; reconnects transparently after a failure."""

    def __init__(
        self, socket_path: Path, request_timeout_s: float = DEFAULT_REQUEST_TIMEOUT_S
    ) -> None:
        self._path = socket_path
        self._timeout = request_timeout_s
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None
        self._lock = asyncio.Lock()
        self._ids = itertools.count(1)
        self._tools: list[ToolSpec] | None = None

    async def _connect(self) -> None:
        try:
            self._reader, self._writer = await asyncio.open_unix_connection(
                str(self._path), limit=4 * 1024 * 1024
            )
        except OSError as err:
            raise McpError(f"AI Core is not running ({self._path}): {err}") from err
        await self._send_locked(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "osaima-agent", "version": __version__},
            },
        )
        await self._notify_locked("notifications/initialized")

    async def _send_locked(self, method: str, params: dict[str, Any]) -> Any:
        assert self._reader is not None and self._writer is not None
        request_id = next(self._ids)
        payload = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        self._writer.write(json.dumps(payload).encode() + b"\n")
        await self._writer.drain()
        line = await asyncio.wait_for(self._reader.readline(), self._timeout)
        if not line:
            raise McpError("AI Core closed the connection")
        reply = json.loads(line)
        if reply.get("id") != request_id:
            raise McpError("AI Core reply did not match the request")
        if error := reply.get("error"):
            raise McpError(str(error.get("message", "AI Core error")), error.get("code"))
        return reply.get("result")

    async def _notify_locked(self, method: str) -> None:
        assert self._writer is not None
        self._writer.write(json.dumps({"jsonrpc": "2.0", "method": method}).encode() + b"\n")
        await self._writer.drain()

    async def request(self, method: str, params: dict[str, Any] | None = None) -> Any:
        async with self._lock:
            for attempt in range(2):
                try:
                    if self._writer is None:
                        await self._connect()
                    return await self._send_locked(method, params or {})
                except McpError as err:
                    if err.code is not None:
                        raise  # a real JSON-RPC error, not a broken connection
                    self._reset()
                    if attempt == 1:
                        raise
                except (TimeoutError, OSError, json.JSONDecodeError) as err:
                    self._reset()
                    if attempt == 1:
                        raise McpError(f"lost connection to the AI Core: {err}") from err
        raise AssertionError("unreachable")

    def _reset(self) -> None:
        if self._writer is not None:
            self._writer.close()
        self._reader = self._writer = None

    async def list_tools(self) -> list[ToolSpec]:
        if self._tools is None:
            result = await self.request("tools/list")
            self._tools = [
                ToolSpec(
                    name=t["name"],
                    title=t.get("title", ""),
                    description=t.get("description", ""),
                    input_schema=t.get("inputSchema", {"type": "object"}),
                    annotations=t.get("annotations", {}),
                )
                for t in result.get("tools", [])
            ]
        return self._tools

    async def call_tool(self, name: str, arguments: dict[str, Any]) -> ToolOutcome:
        try:
            result = await self.request("tools/call", {"name": name, "arguments": arguments})
        except McpError as err:
            if err.code is None:
                raise
            # Invalid arguments etc.: tell the model so it can correct itself.
            return ToolOutcome(ok=False, text=f"invalid request: {err}")
        text = "\n".join(
            block.get("text", "")
            for block in result.get("content", [])
            if block.get("type") == "text"
        )
        return ToolOutcome(
            ok=not result.get("isError", False),
            text=text,
            structured=result.get("structuredContent"),
        )

    async def aclose(self) -> None:
        async with self._lock:
            self._reset()
