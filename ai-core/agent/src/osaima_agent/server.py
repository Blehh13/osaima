"""JSON-RPC server for the shell, on a per-user Unix socket.

Requests (newline-delimited JSON-RPC 2.0):

- ``agent.status`` -> which models are configured and reachable
- ``agent.chat {message, conversation_id?, model?, client_tools?}`` ->
  ``{turn_id, conversation_id}``; progress then arrives as ``agent.event``
  notifications carrying ``turn_id``
- ``agent.approve {turn_id, call_id, approved}`` answers an ``approval_required`` event
- ``agent.client_tool_result {turn_id, call_id, ok, output}`` answers a
  ``client_tool_call`` event
- ``agent.cancel {turn_id}``, ``agent.reset {conversation_id}``, ``agent.audit {limit?}``

Only processes of the same user may connect (checked with SO_PEERCRED).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import socket
import struct
import uuid
from collections import OrderedDict
from pathlib import Path
from typing import Any

from . import __version__
from .agent import Agent, Conversation, ModelChoice
from .audit import AuditLog
from .llm import ToolCall, ToolSpec
from .mcp_client import McpError, ToolOutcome

log = logging.getLogger(__name__)

MAX_LINE_BYTES = 1024 * 1024
MAX_CONVERSATIONS = 32
MAX_MESSAGE_CHARS = 8000

INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
BUSY = -32001
UNAVAILABLE = -32002


class RpcError(Exception):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


class AgentServer:
    def __init__(self, agent: Agent, audit: AuditLog) -> None:
        self._agent = agent
        self._audit = audit
        self._conversations: OrderedDict[str, Conversation] = OrderedDict()
        self._busy: set[str] = set()

    async def serve(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if path.exists():
            if await _is_live(path):
                raise RuntimeError(f"another agent is already listening on {path}")
            path.unlink()
        old_umask = os.umask(0o177)  # socket created as 0600
        try:
            server = await asyncio.start_unix_server(
                self._handle_connection, path=str(path), limit=MAX_LINE_BYTES
            )
        finally:
            os.umask(old_umask)
        log.info("agent listening on %s", path)
        try:
            async with server:
                await server.serve_forever()
        finally:
            with contextlib.suppress(FileNotFoundError):
                path.unlink()

    async def _handle_connection(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        sock = writer.get_extra_info("socket")
        if sock is not None and not _same_user(sock):
            log.warning("rejected connection from another user")
            writer.close()
            return
        session = _Session(self, writer)
        try:
            while line := await reader.readline():
                if not line.strip():
                    continue
                await session.handle_line(line)
        except (ConnectionError, asyncio.LimitOverrunError, ValueError) as err:
            log.debug("connection closed: %s", err)
        finally:
            await session.close()

    def conversation(self, conversation_id: str) -> Conversation | None:
        convo = self._conversations.get(conversation_id)
        if convo is not None:
            self._conversations.move_to_end(conversation_id)
        return convo

    def remember(self, convo: Conversation) -> None:
        self._conversations[convo.id] = convo
        while len(self._conversations) > MAX_CONVERSATIONS:
            self._conversations.popitem(last=False)


class _Session:
    """One shell connection: its running turns and pending answers."""

    def __init__(self, server: AgentServer, writer: asyncio.StreamWriter) -> None:
        self._server = server
        self._writer = writer
        self._write_lock = asyncio.Lock()
        self._turns: dict[str, asyncio.Task[None]] = {}
        self._pending: dict[tuple[str, str], asyncio.Future[Any]] = {}

    async def send(self, message: dict[str, Any]) -> None:
        data = json.dumps(message, default=str).encode() + b"\n"
        async with self._write_lock:
            self._writer.write(data)
            await self._writer.drain()

    async def handle_line(self, line: bytes) -> None:
        request_id: Any = None
        try:
            try:
                message = json.loads(line)
            except json.JSONDecodeError as err:
                raise RpcError(-32700, f"parse error: {err}") from err
            if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
                raise RpcError(INVALID_REQUEST, "expected a JSON-RPC 2.0 object")
            request_id = message.get("id")
            method = message.get("method")
            params = message.get("params") or {}
            if not isinstance(method, str) or not isinstance(params, dict):
                raise RpcError(INVALID_REQUEST, "method must be a string, params an object")
            result = await self._dispatch(method, params)
        except RpcError as err:
            if request_id is not None or err.code == -32700:
                await self.send(
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": err.code, "message": str(err)},
                    }
                )
            return
        if request_id is not None:
            await self.send({"jsonrpc": "2.0", "id": request_id, "result": result})

    async def _dispatch(self, method: str, params: dict[str, Any]) -> Any:
        if method in ("initialize", "ping"):
            return {"name": "osaima-agent", "version": __version__}
        if method == "agent.status":
            status = await self._server._agent.status()
            return {**status, "cloud_enabled": self._server._agent.cloud_enabled}
        if method == "agent.chat":
            return await self._chat(params)
        if method == "agent.approve":
            self._resolve(params, bool(_require(params, "approved", bool)))
            return {}
        if method == "agent.client_tool_result":
            ok = bool(_require(params, "ok", bool))
            output = str(params.get("output", ""))
            self._resolve(params, ToolOutcome(ok=ok, text=output))
            return {}
        if method == "agent.cancel":
            task = self._turns.get(_require(params, "turn_id", str))
            if task is not None:
                task.cancel()
            return {"cancelled": task is not None}
        if method == "agent.reset":
            self._server._conversations.pop(_require(params, "conversation_id", str), None)
            return {}
        if method == "agent.audit":
            limit = params.get("limit", 50)
            if not isinstance(limit, int) or not 1 <= limit <= 500:
                raise RpcError(INVALID_PARAMS, "limit must be an integer between 1 and 500")
            return {"entries": self._server._audit.tail(limit)}
        raise RpcError(METHOD_NOT_FOUND, f"method not found: {method}")

    async def _chat(self, params: dict[str, Any]) -> dict[str, Any]:
        text = _require(params, "message", str).strip()
        if not text or len(text) > MAX_MESSAGE_CHARS:
            raise RpcError(INVALID_PARAMS, f"message must be 1-{MAX_MESSAGE_CHARS} characters")
        model: ModelChoice
        match params.get("model", "auto"):
            case "auto":
                model = "auto"
            case "local":
                model = "local"
            case "cloud":
                model = "cloud"
            case _:
                raise RpcError(INVALID_PARAMS, "model must be 'auto', 'local' or 'cloud'")

        convo: Conversation | None = None
        if conversation_id := params.get("conversation_id"):
            convo = self._server.conversation(str(conversation_id))
        if convo is None:
            try:
                convo = await self._server._agent.new_conversation(
                    _client_tools(params.get("client_tools", []))
                )
            except McpError as err:
                raise RpcError(UNAVAILABLE, f"AI Core unavailable: {err}") from err
            self._server.remember(convo)
        if convo.id in self._server._busy:
            raise RpcError(BUSY, "this conversation is still answering the previous message")

        turn_id = uuid.uuid4().hex
        self._server._busy.add(convo.id)
        task = asyncio.create_task(self._run_turn(turn_id, convo, text, model))
        self._turns[turn_id] = task
        return {"turn_id": turn_id, "conversation_id": convo.id}

    async def _run_turn(
        self, turn_id: str, convo: Conversation, text: str, model: ModelChoice
    ) -> None:
        async def emit(event: dict[str, Any]) -> None:
            with contextlib.suppress(ConnectionError):
                await self.send(
                    {
                        "jsonrpc": "2.0",
                        "method": "agent.event",
                        "params": {"turn_id": turn_id, **event},
                    }
                )

        async def approve(call: ToolCall, spec: ToolSpec) -> bool:
            return bool(await self._wait(turn_id, call.id))

        async def run_client_tool(call: ToolCall) -> ToolOutcome:
            await emit(
                {
                    "type": "client_tool_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments,
                }
            )
            try:
                outcome = await asyncio.wait_for(self._wait(turn_id, call.id), timeout=30)
            except TimeoutError:
                return ToolOutcome(ok=False, text="The shell did not respond.")
            if not isinstance(outcome, ToolOutcome):
                return ToolOutcome(ok=False, text="The shell sent an invalid result.")
            return outcome

        try:
            await self._server._agent.run_turn(
                convo,
                text,
                emit=emit,
                approve=approve,
                run_client_tool=run_client_tool,
                model=model,
            )
        except asyncio.CancelledError:
            await emit({"type": "cancelled"})
        except Exception as err:  # never let one turn take the server down
            log.exception("turn failed")
            await emit({"type": "error", "message": f"internal error: {err}"})
        finally:
            self._server._busy.discard(convo.id)
            self._turns.pop(turn_id, None)

    async def _wait(self, turn_id: str, call_id: str) -> Any:
        future: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[(turn_id, call_id)] = future
        try:
            return await future
        finally:
            self._pending.pop((turn_id, call_id), None)

    def _resolve(self, params: dict[str, Any], value: Any) -> None:
        key = (_require(params, "turn_id", str), _require(params, "call_id", str))
        future = self._pending.get(key)
        if future is None or future.done():
            raise RpcError(INVALID_PARAMS, "nothing is waiting for that call")
        future.set_result(value)

    async def close(self) -> None:
        for task in list(self._turns.values()):
            task.cancel()
        for future in self._pending.values():
            if not future.done():
                future.cancel()
        self._writer.close()


def _require(params: dict[str, Any], key: str, kind: type) -> Any:
    value = params.get(key)
    if not isinstance(value, kind):
        raise RpcError(INVALID_PARAMS, f"{key} must be a {kind.__name__}")
    return value


def _client_tools(raw: Any) -> list[ToolSpec]:
    """Tools the shell implements itself (open a built-in app, switch workspace...)."""
    if not isinstance(raw, list):
        raise RpcError(INVALID_PARAMS, "client_tools must be a list")
    tools = []
    for item in raw:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            raise RpcError(INVALID_PARAMS, "each client tool needs a name")
        tools.append(
            ToolSpec(
                name=item["name"],
                title=str(item.get("title", "")),
                description=str(item.get("description", "")),
                input_schema=item.get("inputSchema") or {"type": "object", "properties": {}},
                annotations=item.get("annotations") or {},
            )
        )
    return tools


def _same_user(sock: socket.socket) -> bool:
    try:
        creds = sock.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
    except (OSError, AttributeError):
        return False
    _pid, uid, _gid = struct.unpack("3i", creds)
    return uid in (os.geteuid(), 0)


async def _is_live(path: Path) -> bool:
    try:
        _, writer = await asyncio.open_unix_connection(str(path))
    except OSError:
        return False
    writer.close()
    return True
