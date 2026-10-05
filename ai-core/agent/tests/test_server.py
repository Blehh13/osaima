from __future__ import annotations

import asyncio
import json
import os
import stat
import tempfile
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest
from conftest import FakeTools, ScriptedProvider, call, reply

from osaima_agent.agent import Agent
from osaima_agent.audit import AuditLog
from osaima_agent.policy import Policy
from osaima_agent.server import AgentServer


class Client:
    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.reader = reader
        self.writer = writer
        self.ids = 0
        self.events: list[dict[str, Any]] = []

    async def send(self, method: str, **params: Any) -> dict[str, Any]:
        self.ids += 1
        msg = {"jsonrpc": "2.0", "id": self.ids, "method": method, "params": params}
        self.writer.write(json.dumps(msg).encode() + b"\n")
        await self.writer.drain()
        while True:
            reply = await self.next()
            if reply.get("id") == self.ids:
                return reply
            self.events.append(reply["params"])

    async def next(self) -> dict[str, Any]:
        line = await asyncio.wait_for(self.reader.readline(), 5)
        assert line, "server closed the connection"
        return json.loads(line)

    async def event(self, kind: str) -> dict[str, Any]:
        for i, e in enumerate(self.events):
            if e["type"] == kind:
                return self.events.pop(i)
        while True:
            msg = await self.next()
            assert msg.get("method") == "agent.event", msg
            if msg["params"]["type"] == kind:
                return msg["params"]
            self.events.append(msg["params"])


@pytest.fixture
async def socket_dir() -> AsyncIterator[Path]:
    # Unix socket paths are limited to ~108 bytes, so avoid long pytest paths.
    with tempfile.TemporaryDirectory(prefix="osa-") as d:
        yield Path(d)


async def start(
    socket_dir: Path, local: ScriptedProvider, audit: AuditLog, tools: FakeTools | None = None
) -> tuple[asyncio.Task[None], Path]:
    agent = Agent(tools=tools or FakeTools(), local=local, cloud=None, policy=Policy(), audit=audit)
    path = socket_dir / "agent.sock"
    task = asyncio.create_task(AgentServer(agent, audit).serve(path))
    for _ in range(100):
        if path.exists():
            break
        await asyncio.sleep(0.01)
    return task, path


async def connect(path: Path) -> Client:
    reader, writer = await asyncio.open_unix_connection(str(path))
    return Client(reader, writer)


async def test_chat_streams_events_and_keeps_conversations(socket_dir: Path, audit: AuditLog) -> None:
    local = ScriptedProvider("local", [reply("First."), reply("Second.")])
    task, path = await start(socket_dir, local, audit)
    try:
        assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
        client = await connect(path)
        started = await client.send("agent.chat", message="hello")
        turn = started["result"]
        done = await client.event("done")
        assert done["turn_id"] == turn["turn_id"]
        assert done["text"] == "First."

        again = await client.send(
            "agent.chat", message="and again", conversation_id=turn["conversation_id"]
        )
        assert again["result"]["conversation_id"] == turn["conversation_id"]
        assert (await client.event("done"))["text"] == "Second."
        assert [m.content for m in local.seen[1]] == ["hello", "First.", "and again"]
    finally:
        task.cancel()


async def test_approval_round_trip(socket_dir: Path, audit: AuditLog) -> None:
    tools = FakeTools()
    local = ScriptedProvider("local", [call("kill_process", pid=99), reply("Ended it.")])
    task, path = await start(socket_dir, local, audit, tools)
    try:
        client = await connect(path)
        turn = (await client.send("agent.chat", message="kill 99"))["result"]
        request = await client.event("approval_required")
        assert request["arguments"] == {"pid": 99}
        ack = await client.send(
            "agent.approve", turn_id=turn["turn_id"], call_id=request["call_id"], approved=True
        )
        assert ack["result"] == {}
        assert (await client.event("done"))["text"] == "Ended it."
        assert tools.calls == [("kill_process", {"pid": 99})]
    finally:
        task.cancel()


async def test_client_tool_round_trip(socket_dir: Path, audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("shell_open_app", app="files"), reply("Opened.")])
    task, path = await start(socket_dir, local, audit)
    try:
        client = await connect(path)
        turn = (
            await client.send(
                "agent.chat",
                message="open files",
                client_tools=[
                    {
                        "name": "shell_open_app",
                        "description": "Open a built-in app",
                        "inputSchema": {"type": "object", "properties": {"app": {"type": "string"}}},
                        "annotations": {"readOnlyHint": False, "destructiveHint": False},
                    }
                ],
            )
        )["result"]
        request = await client.event("client_tool_call")
        assert request["arguments"] == {"app": "files"}
        await client.send(
            "agent.client_tool_result",
            turn_id=turn["turn_id"],
            call_id=request["call_id"],
            ok=True,
            output="opened Files",
        )
        await client.event("done")
        assert local.seen[1][-1].content == "opened Files"
    finally:
        task.cancel()


async def test_rejects_bad_requests_and_concurrent_turns(socket_dir: Path, audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("kill_process", pid=1), reply("ok")])
    task, path = await start(socket_dir, local, audit)
    try:
        client = await connect(path)
        assert (await client.send("agent.chat", message=""))["error"]["code"] == -32602
        assert (await client.send("agent.chat", message="x", model="gpt"))["error"]["code"] == -32602
        assert (await client.send("agent.nope"))["error"]["code"] == -32601
        stray = await client.send("agent.approve", turn_id="t", call_id="c", approved=True)
        assert stray["error"]["code"] == -32602

        turn = (await client.send("agent.chat", message="kill 1"))["result"]
        await client.event("approval_required")
        busy = await client.send(
            "agent.chat", message="again", conversation_id=turn["conversation_id"]
        )
        assert busy["error"]["code"] == -32001

        cancelled = await client.send("agent.cancel", turn_id=turn["turn_id"])
        assert cancelled["result"] == {"cancelled": True}
        await client.event("cancelled")
    finally:
        task.cancel()


async def test_refuses_to_replace_a_live_socket(socket_dir: Path, audit: AuditLog) -> None:
    task, path = await start(socket_dir, ScriptedProvider("local", []), audit)
    try:
        second = AgentServer(
            Agent(tools=FakeTools(), local=None, cloud=None, policy=Policy(), audit=audit), audit
        )
        with pytest.raises(RuntimeError, match="already listening"):
            await second.serve(path)
    finally:
        task.cancel()
