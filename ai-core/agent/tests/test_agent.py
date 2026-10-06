from __future__ import annotations

import asyncio
from typing import Any

import pytest
from conftest import FakeTools, ScriptedProvider, call, reply

from osaima_agent.agent import Agent, Conversation
from osaima_agent.audit import AuditLog
from osaima_agent.config import LimitsConfig
from osaima_agent.llm import Completion, NativeContent, ProviderUnavailable, ToolCall, ToolSpec
from osaima_agent.mcp_client import ToolOutcome
from osaima_agent.policy import Policy


class Recorder:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    async def __call__(self, event: dict[str, Any]) -> None:
        self.events.append(event)

    def types(self) -> list[str]:
        return [e["type"] for e in self.events]


def make_agent(
    local: ScriptedProvider | None,
    cloud: ScriptedProvider | None = None,
    *,
    tools: FakeTools | None = None,
    audit: AuditLog,
    policy: Policy | None = None,
    **kwargs: Any,
) -> tuple[Agent, FakeTools]:
    tools = tools or FakeTools()
    agent = Agent(
        tools=tools,
        local=local,
        cloud=cloud,
        policy=policy or Policy(),
        audit=audit,
        **kwargs,
    )
    return agent, tools


async def approve_all(call: ToolCall, spec: ToolSpec) -> bool:
    return True


async def deny_all(call: ToolCall, spec: ToolSpec) -> bool:
    return False


async def run(agent: Agent, text: str, **kwargs: Any) -> tuple[Any, Recorder, Conversation]:
    convo = await agent.new_conversation()
    rec = Recorder()
    kwargs.setdefault("approve", approve_all)
    result = await agent.run_turn(convo, text, emit=rec, **kwargs)
    return result, rec, convo


async def test_answers_without_tools(audit: AuditLog) -> None:
    agent, tools = make_agent(ScriptedProvider("local", [reply("Hello!")]), audit=audit)
    result, rec, convo = await run(agent, "hi")
    assert result.stopped == "answered"
    assert result.text == "Hello!"
    assert rec.types() == ["text", "done"]
    assert [m.role for m in convo.messages] == ["user", "assistant"]
    assert tools.calls == []


async def test_read_only_tool_runs_without_approval(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("get_system_stats"), reply("CPU is at 5%.")])
    agent, tools = make_agent(local, audit=audit)
    asked: list[str] = []

    async def approve(c: ToolCall, s: ToolSpec) -> bool:
        asked.append(c.name)
        return True

    result, rec, convo = await run(agent, "how busy is my cpu", approve=approve)
    assert result.text == "CPU is at 5%."
    assert tools.calls == [("get_system_stats", {})]
    assert asked == []
    assert "approval_required" not in rec.types()
    assert [m.role for m in convo.messages] == ["user", "assistant", "tool", "assistant"]
    # The model saw the tool result on its second turn.
    assert local.seen[1][-1].content == "get_system_stats ok"


async def test_reversible_tool_runs_without_approval_by_default(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("set_volume", percent=40), reply("Done.")])
    agent, tools = make_agent(local, audit=audit)
    _, rec, _ = await run(agent, "volume 40", approve=deny_all)
    assert tools.calls == [("set_volume", {"percent": 40})]
    assert "approval_required" not in rec.types()


async def test_all_changes_mode_confirms_reversible_tools(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("set_volume", percent=40), reply("Okay.")])
    agent, tools = make_agent(local, audit=audit, policy=Policy("all_changes"))
    _, rec, _ = await run(agent, "volume 40", approve=deny_all)
    assert tools.calls == []
    assert "approval_required" in rec.types()


@pytest.mark.parametrize(("approved", "executed"), [(True, True), (False, False)])
async def test_destructive_tool_needs_approval(
    audit: AuditLog, approved: bool, executed: bool
) -> None:
    local = ScriptedProvider("local", [call("kill_process", pid=4242), reply("Handled.")])
    agent, tools = make_agent(local, audit=audit)

    async def approve(c: ToolCall, s: ToolSpec) -> bool:
        return approved

    _, rec, convo = await run(agent, "kill 4242", approve=approve)
    assert (tools.calls == [("kill_process", {"pid": 4242})]) is executed
    request = next(e for e in rec.events if e["type"] == "approval_required")
    assert request["arguments"] == {"pid": 4242}
    assert request["destructive"] is True
    tool_msg = convo.messages[2]
    assert tool_msg.is_error is (not approved)
    if not approved:
        assert "declined" in tool_msg.content
    entries = audit.tail()
    assert entries[-1]["tool"] == "kill_process"
    assert entries[-1]["executed"] is approved


async def test_unanswered_approval_times_out_as_declined(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("kill_process", pid=7), reply("Okay.")])
    agent, tools = make_agent(local, audit=audit, approval_timeout_s=0.05)

    async def never(c: ToolCall, s: ToolSpec) -> bool:
        await asyncio.sleep(10)
        return True

    await run(agent, "kill 7", approve=never)
    assert tools.calls == []


async def test_denied_tools_never_run(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("kill_process", pid=7), reply("I can't.")])
    agent, tools = make_agent(local, audit=audit, policy=Policy(deny=["kill_process"]))
    _, rec, convo = await run(agent, "kill 7")
    assert tools.calls == []
    assert "approval_required" not in rec.types()
    assert "disabled by the system policy" in convo.messages[2].content


async def test_unknown_tool_is_reported_to_the_model(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("format_disk"), reply("Sorry.")])
    agent, tools = make_agent(local, audit=audit)
    _, _, convo = await run(agent, "format my disk")
    assert tools.calls == []
    assert convo.messages[2].is_error
    assert "no tool named" in convo.messages[2].content


async def test_tool_failures_are_marked_as_errors(audit: AuditLog) -> None:
    tools = FakeTools({"get_system_stats": ToolOutcome(ok=False, text="sensor offline")})
    local = ScriptedProvider("local", [call("get_system_stats"), reply("I couldn't read it.")])
    agent, _ = make_agent(local, tools=tools, audit=audit)
    _, rec, convo = await run(agent, "stats")
    assert convo.messages[2].content == "ERROR: sensor offline"
    result_event = next(e for e in rec.events if e["type"] == "tool_result")
    assert result_event["ok"] is False


async def test_falls_back_to_cloud_when_local_is_down(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [ProviderUnavailable("Ollama is not running")])
    cloud = ScriptedProvider("cloud", [reply("From the cloud.", provider="cloud")])
    agent, _ = make_agent(local, cloud, audit=audit)
    result, rec, _ = await run(agent, "hi")
    assert result.provider == "cloud"
    assert "notice" in rec.types()


async def test_local_only_mode_reports_unavailability(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [ProviderUnavailable("Ollama is not running")])
    cloud = ScriptedProvider("cloud", [reply("unused", provider="cloud")])
    agent, _ = make_agent(local, cloud, audit=audit)
    result, rec, _ = await run(agent, "hi", model="local")
    assert result.stopped == "no_model"
    assert rec.types() == ["error"]
    assert cloud.seen == []


async def test_repeated_invalid_tool_calls_escalate_to_cloud(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("bogus"), call("bogus")])
    cloud = ScriptedProvider("cloud", [reply("Fixed it.", provider="cloud")])
    agent, _ = make_agent(local, cloud, audit=audit)
    result, _, _ = await run(agent, "do the thing")
    assert result.provider == "cloud"
    assert result.text == "Fixed it."


async def test_stops_after_max_steps(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("get_system_stats") for _ in range(3)])
    agent, _ = make_agent(local, audit=audit, limits=LimitsConfig(max_steps=3))
    result, rec, _ = await run(agent, "loop forever")
    assert result.stopped == "max_steps"
    assert rec.types()[-1] == "done"


async def test_no_model_configured(audit: AuditLog) -> None:
    agent, _ = make_agent(None, audit=audit)
    result, rec, _ = await run(agent, "hi")
    assert result.stopped == "no_model"
    assert "Install Ollama" in rec.events[0]["message"]


async def test_refusals_are_not_replayed(audit: AuditLog) -> None:
    refused = Completion(
        text="partial",
        tool_calls=[],
        stop="refusal",
        provider="cloud",
        model="test",
        native=NativeContent("claude", ["raw"]),
        detail="declined (cyber)",
    )
    agent, _ = make_agent(None, ScriptedProvider("cloud", [refused]), audit=audit)
    result, _, convo = await run(agent, "something", model="cloud")
    assert result.stopped == "declined"
    assert convo.messages[-1].native is None
    assert convo.messages[-1].content == "The model declined this request."


async def test_cancelled_turn_leaves_a_valid_history(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [call("kill_process", pid=9)])
    agent, tools = make_agent(local, audit=audit)
    convo = await agent.new_conversation()
    waiting = asyncio.Event()

    async def approve(c: ToolCall, s: ToolSpec) -> bool:
        waiting.set()
        await asyncio.sleep(60)
        return True

    task = asyncio.create_task(agent.run_turn(convo, "kill 9", emit=Recorder(), approve=approve))
    await waiting.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert tools.calls == []
    assert convo.messages[-1].role == "tool"
    assert convo.messages[-1].tool_call_id == convo.messages[1].tool_calls[0].id


async def test_client_tools_are_run_by_the_shell(audit: AuditLog) -> None:
    open_app = ToolSpec(
        name="shell_open_app",
        description="Open a built-in app",
        input_schema={"type": "object", "properties": {"app": {"type": "string"}}},
        annotations={"readOnlyHint": False, "destructiveHint": False},
    )
    local = ScriptedProvider("local", [call("shell_open_app", app="files"), reply("Opened.")])
    agent, tools = make_agent(local, audit=audit)
    convo = await agent.new_conversation([open_app])
    ran: list[ToolCall] = []

    async def run_client_tool(c: ToolCall) -> ToolOutcome:
        ran.append(c)
        return ToolOutcome(ok=True, text="opened files")

    await agent.run_turn(
        convo, "open files", emit=Recorder(), approve=approve_all, run_client_tool=run_client_tool
    )
    assert [c.arguments for c in ran] == [{"app": "files"}]
    assert tools.calls == []
    assert convo.messages[2].content == "opened files"


async def test_conversation_history_carries_over(audit: AuditLog) -> None:
    local = ScriptedProvider("local", [reply("I'm Interstellar."), reply("You asked who I am.")])
    agent, _ = make_agent(local, audit=audit)
    convo = await agent.new_conversation()
    await agent.run_turn(convo, "who are you", emit=Recorder(), approve=approve_all)
    await agent.run_turn(convo, "what did I ask", emit=Recorder(), approve=approve_all)
    assert [m.content for m in local.seen[1]] == [
        "who are you",
        "I'm Interstellar.",
        "what did I ask",
    ]
