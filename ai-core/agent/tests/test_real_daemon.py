"""End-to-end against the real AI Core binary. CI sets OSAIMA_DAEMON_BIN."""

from __future__ import annotations

import asyncio
import os
import tempfile
from collections.abc import AsyncIterator
from pathlib import Path

import pytest
from conftest import ScriptedProvider, call, reply

from osaima_agent.agent import Agent
from osaima_agent.audit import AuditLog
from osaima_agent.mcp_client import McpClient
from osaima_agent.policy import Policy

DAEMON = os.environ.get("OSAIMA_DAEMON_BIN")
pytestmark = pytest.mark.skipif(not DAEMON, reason="OSAIMA_DAEMON_BIN not set")


@pytest.fixture
async def daemon_socket() -> AsyncIterator[Path]:
    with tempfile.TemporaryDirectory(prefix="osa-") as d:
        path = Path(d) / "mcp.sock"
        assert DAEMON is not None
        proc = await asyncio.create_subprocess_exec(DAEMON, "--socket", str(path))
        try:
            for _ in range(200):
                if path.exists():
                    break
                await asyncio.sleep(0.02)
            yield path
        finally:
            proc.terminate()
            await proc.wait()


async def test_agent_drives_the_real_ai_core(daemon_socket: Path, audit: AuditLog) -> None:
    mcp = McpClient(daemon_socket)
    try:
        tools = await mcp.list_tools()
        names = {t.name for t in tools}
        assert {"get_system_stats", "list_processes", "kill_process", "launch_app"} <= names
        assert next(t for t in tools if t.name == "kill_process").destructive

        local = ScriptedProvider(
            "local",
            [call("list_processes", limit=3, sort_by="memory"), reply("Here are the top 3.")],
        )
        agent = Agent(tools=mcp, local=local, cloud=None, policy=Policy(), audit=audit)
        convo = await agent.new_conversation()
        events: list[dict[str, object]] = []

        async def emit(e: dict[str, object]) -> None:
            events.append(e)

        async def approve(*_: object) -> bool:
            return False

        result = await agent.run_turn(convo, "what uses memory", emit=emit, approve=approve)
        assert result.stopped == "answered"
        tool_result = next(e for e in events if e["type"] == "tool_result")
        assert tool_result["ok"] is True
        structured = tool_result["structured"]
        assert isinstance(structured, dict)
        assert 0 < len(structured["processes"]) <= 3
    finally:
        await mcp.aclose()
