from __future__ import annotations

import json
from types import SimpleNamespace
from typing import Any

import httpx
import pytest
from conftest import SPECS

from osaima_agent.llm import Message, NativeContent, ProviderError, ProviderUnavailable, ToolCall
from osaima_agent.llm.claude import BETAS, ClaudeProvider, completion_from, to_claude
from osaima_agent.llm.ollama import MAX_HISTORY_MESSAGES, OllamaProvider, to_ollama


async def collect() -> tuple[list[str], Any]:
    seen: list[str] = []

    async def sink(delta: str) -> None:
        seen.append(delta)

    return seen, sink


# ── Ollama ──────────────────────────────────────────────────────────────────


def ndjson(*chunks: dict[str, Any]) -> bytes:
    return b"".join(json.dumps(c).encode() + b"\n" for c in chunks)


def ollama(handler: Any) -> OllamaProvider:
    return OllamaProvider(
        "http://ollama.test", "qwen2.5:7b-instruct", transport=httpx.MockTransport(handler)
    )


async def test_ollama_streams_text_and_tool_calls() -> None:
    sent: list[dict[str, Any]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        sent.append(json.loads(request.content))
        return httpx.Response(
            200,
            content=ndjson(
                {"message": {"role": "assistant", "content": "Check"}, "done": False},
                {"message": {"role": "assistant", "content": "ing."}, "done": False},
                {
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [
                            {"function": {"name": "get_volume", "arguments": {}}},
                            {"function": {"name": "set_volume", "arguments": '{"percent": 30}'}},
                        ],
                    },
                    "done": False,
                },
                {"done": True, "done_reason": "stop"},
            ),
        )

    seen, sink = await collect()
    result = await ollama(handler).complete("sys", [Message("user", "hi")], SPECS, sink)
    assert seen == ["Check", "ing."]
    assert result.text == "Checking."
    assert result.stop == "tool_calls"
    assert [c.name for c in result.tool_calls] == ["get_volume", "set_volume"]
    assert result.tool_calls[1].arguments == {"percent": 30}
    body = sent[0]
    assert body["messages"][0] == {"role": "system", "content": "sys"}
    assert body["tools"][0]["function"]["name"] == "get_system_stats"
    assert body["stream"] is True


async def test_ollama_missing_model_and_server() -> None:
    _, sink = await collect()
    missing = ollama(lambda r: httpx.Response(404, json={"error": "model not found"}))
    with pytest.raises(ProviderUnavailable, match="ollama pull"):
        await missing.complete("s", [Message("user", "hi")], [], sink)

    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    with pytest.raises(ProviderUnavailable, match="not running"):
        await ollama(refuse).complete("s", [Message("user", "hi")], [], sink)

    broken = ollama(lambda r: httpx.Response(200, content=ndjson({"error": "out of memory"})))
    with pytest.raises(ProviderError, match="out of memory"):
        await broken.complete("s", [Message("user", "hi")], [], sink)


async def test_ollama_availability_checks_the_model_list() -> None:
    tags = {"models": [{"name": "qwen2.5:7b-instruct"}]}
    assert await ollama(lambda r: httpx.Response(200, json=tags)).available()
    assert not await ollama(lambda r: httpx.Response(200, json={"models": []})).available()


def test_ollama_history_is_trimmed_to_a_user_turn() -> None:
    history: list[Message] = []
    for i in range(20):
        history.append(Message("user", f"q{i}"))
        history.append(Message("assistant", "", tool_calls=[ToolCall(f"c{i}", "get_volume", {})]))
        history.append(Message("tool", "50%", tool_call_id=f"c{i}", tool_name="get_volume"))
    converted = to_ollama(history)
    assert len(converted) <= MAX_HISTORY_MESSAGES
    assert converted[0]["role"] == "user"
    assert converted[-1] == {"role": "tool", "content": "50%", "tool_name": "get_volume"}


# ── Claude ──────────────────────────────────────────────────────────────────


def test_to_claude_groups_results_and_replays_native_content() -> None:
    native = [SimpleNamespace(type="thinking"), SimpleNamespace(type="tool_use")]
    history = [
        Message("user", "how loud is it, and the cpu?"),
        Message(
            "assistant",
            "",
            tool_calls=[ToolCall("a", "get_volume", {}), ToolCall("b", "get_system_stats", {})],
            native=NativeContent("claude", native),
        ),
        Message("tool", "40%", tool_call_id="a", tool_name="get_volume"),
        Message("tool", "ERROR: no", tool_call_id="b", tool_name="get_system_stats", is_error=True),
        Message("assistant", "Volume is 40%."),
        Message("user", "thanks"),
        Message("assistant", ""),  # empty local reply: skipped
        Message("user", "bye"),
    ]
    out = to_claude(history)
    assert [m["role"] for m in out] == ["user", "assistant", "user", "assistant", "user", "user"]
    assert out[1]["content"] is native
    results = out[2]["content"]
    assert [r["tool_use_id"] for r in results] == ["a", "b"]
    assert results[1]["is_error"] is True
    assert out[3]["content"] == [{"type": "text", "text": "Volume is 40%."}]
    # Deterministic: converting again yields an identical prefix.
    assert to_claude(history) == out


def test_local_tool_calls_convert_to_tool_use_blocks() -> None:
    out = to_claude(
        [
            Message("user", "mute"),
            Message(
                "assistant", "Muting.", tool_calls=[ToolCall("x", "set_volume", {"muted": True})]
            ),
            Message("tool", "ok", tool_call_id="x", tool_name="set_volume"),
        ]
    )
    assert out[1]["content"] == [
        {"type": "text", "text": "Muting."},
        {"type": "tool_use", "id": "x", "name": "set_volume", "input": {"muted": True}},
    ]


def block(kind: str, **fields: Any) -> SimpleNamespace:
    return SimpleNamespace(type=kind, **fields)


def message(stop: str, *content: SimpleNamespace, **extra: Any) -> SimpleNamespace:
    return SimpleNamespace(
        content=list(content), stop_reason=stop, model="claude-opus-5-5", **extra
    )


def test_completion_from_maps_stop_reasons() -> None:
    tool = block("tool_use", id="t1", name="get_volume", input={})
    done = completion_from(message("tool_use", block("text", text="Let me check."), tool), "claude")
    assert done.stop == "tool_calls"
    assert done.tool_calls[0].name == "get_volume"
    assert done.native is not None and done.native.provider == "claude"

    truncated = completion_from(message("max_tokens", tool), "claude")
    assert truncated.stop == "max_tokens"
    assert truncated.tool_calls == []  # a truncated tool input must never run

    refused = completion_from(
        message("refusal", stop_details=SimpleNamespace(category="cyber")), "claude"
    )
    assert refused.stop == "refusal"
    assert "cyber" in refused.detail

    odd = completion_from(message("tool_use", block("tool_use", id="t", name="x", input=[1])), "c")
    assert "__invalid_json__" in odd.tool_calls[0].arguments


class FakeStream:
    def __init__(self, final: SimpleNamespace, texts: list[str]) -> None:
        self._final = final
        self._texts = texts

    async def __aenter__(self) -> FakeStream:
        return self

    async def __aexit__(self, *exc: object) -> None:
        return None

    def __aiter__(self) -> Any:
        async def gen() -> Any:
            for t in self._texts:
                yield SimpleNamespace(type="text", text=t)
            yield SimpleNamespace(type="message_stop")

        return gen()

    async def get_final_message(self) -> SimpleNamespace:
        return self._final


class FakeClient:
    def __init__(self, final: SimpleNamespace, texts: list[str]) -> None:
        self.requests: list[dict[str, Any]] = []
        outer = self

        class Messages:
            def stream(self, **request: Any) -> FakeStream:
                outer.requests.append(request)
                return FakeStream(final, texts)

        self.beta = SimpleNamespace(messages=Messages())


async def test_claude_request_shape_and_streaming() -> None:
    final = message("end_turn", block("text", text="Hi there"))
    client = FakeClient(final, ["Hi ", "there"])
    provider = ClaudeProvider(effort="low", client=client)  # type: ignore[arg-type]
    seen, sink = await collect()
    result = await provider.complete("system prompt", [Message("user", "hi")], SPECS, sink)

    assert seen == ["Hi ", "there"]
    assert result.text == "Hi there"
    assert result.stop == "end"
    request = client.requests[0]
    assert request["model"] == "claude-opus-5-5"
    assert request["fallbacks"] == "default"
    assert request["betas"] == BETAS
    assert request["output_config"] == {"effort": "low"}
    assert request["thinking"]["block_binding"] == {"prefix_mismatch_behavior": "drop_block"}
    assert all(t["eager_input_streaming"] for t in request["tools"])
    assert request["tools"][0]["input_schema"] == SPECS[0].input_schema
