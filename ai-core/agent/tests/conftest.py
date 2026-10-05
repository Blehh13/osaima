"""Shared fakes: a scripted model and an in-memory AI Core."""

from __future__ import annotations

import copy
from typing import Any

import pytest

from osaima_agent.audit import AuditLog
from osaima_agent.llm import Completion, Message, TextSink, ToolCall, ToolSpec, new_call_id
from osaima_agent.mcp_client import ToolOutcome


class ScriptedProvider:
    """Plays back a fixed list of completions (or raises listed exceptions)."""

    def __init__(self, name: str, script: list[Completion | Exception], model: str = "test") -> None:
        self.name = name
        self.model = model
        self.script = list(script)
        self.seen: list[list[Message]] = []

    async def available(self) -> bool:
        return True

    async def complete(
        self, system: str, messages: list[Message], tools: list[ToolSpec], on_text: TextSink
    ) -> Completion:
        self.seen.append(copy.deepcopy(messages))
        if not self.script:
            raise AssertionError(f"{self.name}: script exhausted")
        item = self.script.pop(0)
        if isinstance(item, Exception):
            raise item
        if item.text:
            await on_text(item.text)
        return item


def reply(text: str, provider: str = "local") -> Completion:
    return Completion(text=text, tool_calls=[], stop="end", provider=provider, model="test")


def call(name: str, provider: str = "local", **arguments: Any) -> Completion:
    return Completion(
        text="",
        tool_calls=[ToolCall(id=new_call_id(), name=name, arguments=arguments)],
        stop="tool_calls",
        provider=provider,
        model="test",
    )


SPECS = [
    ToolSpec(
        name="get_system_stats",
        title="System statistics",
        description="Stats",
        input_schema={"type": "object", "properties": {}},
        annotations={"readOnlyHint": True},
    ),
    ToolSpec(
        name="set_volume",
        title="Set volume",
        description="Volume",
        input_schema={"type": "object", "properties": {"percent": {"type": "integer"}}},
        annotations={"readOnlyHint": False, "destructiveHint": False},
    ),
    ToolSpec(
        name="kill_process",
        title="End a process",
        description="Kill",
        input_schema={"type": "object", "properties": {"pid": {"type": "integer"}}},
        annotations={"readOnlyHint": False, "destructiveHint": True},
    ),
]


class FakeTools:
    def __init__(self, results: dict[str, ToolOutcome] | None = None) -> None:
        self.results = results or {}
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def list_tools(self) -> list[ToolSpec]:
        return list(SPECS)

    async def call_tool(self, name: str, arguments: dict[str, Any]) -> ToolOutcome:
        self.calls.append((name, arguments))
        return self.results.get(name, ToolOutcome(ok=True, text=f"{name} ok", structured={"ok": 1}))


@pytest.fixture
def audit(tmp_path: Any) -> AuditLog:
    return AuditLog(tmp_path / "audit.jsonl")
