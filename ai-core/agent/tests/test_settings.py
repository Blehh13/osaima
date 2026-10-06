"""Every tunable value is configurable, validated, and actually reaches the code that uses it."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import httpx
import pytest
from conftest import SPECS

from osaima_agent import config
from osaima_agent.config import ConfigError, from_dict
from osaima_agent.llm import Message
from osaima_agent.llm.claude import BINDING_BETA, FALLBACK_BETA, ClaudeProvider
from osaima_agent.llm.ollama import OllamaProvider

GIB = 1024**3


# ── validation ──────────────────────────────────────────────────────────────


def test_defaults_are_valid_and_complete() -> None:
    cfg = from_dict({})
    assert cfg.limits.max_steps == 6
    assert cfg.local.temperature == 0.2
    assert cfg.local.keep_alive == "10m"
    assert cfg.cloud.server_fallback is True
    assert cfg.system_prompt == config.DEFAULT_SYSTEM_PROMPT


def test_every_setting_can_be_overridden() -> None:
    cfg = from_dict(
        {
            "local": {
                "model": "llama3.2:3b",
                "url": "http://gpu-box:11434/",
                "context_tokens": 4096,
                "timeout_s": 30,
                "connect_timeout_s": 1.5,
                "temperature": 0.7,
                "keep_alive": "1h",
                "history_messages": 10,
            },
            "cloud": {
                "enabled": True,
                "model": "claude-sonnet-5-5",
                "effort": "low",
                "max_tokens": 4000,
                "server_fallback": False,
                "timeout_s": 60,
                "max_retries": 0,
                "json_retries": 1,
            },
            "policy": {
                "confirm": "all_changes",
                "deny": ["power_action"],
                "approval_timeout_s": 30,
            },
            "limits": {
                "max_steps": 3,
                "max_tool_output_chars": 500,
                "event_output_chars": 100,
                "event_structured_bytes": 1000,
                "max_conversations": 4,
                "max_message_chars": 200,
                "max_line_bytes": 65536,
                "client_tool_timeout_s": 5,
                "core_request_timeout_s": 10,
            },
            "audit": {"max_bytes": 4096},
        }
    )
    assert cfg.local.url == "http://gpu-box:11434"
    assert (cfg.local.temperature, cfg.local.keep_alive, cfg.local.history_messages) == (
        0.7,
        "1h",
        10,
    )
    assert cfg.cloud.server_fallback is False and cfg.cloud.max_retries == 0
    assert cfg.policy.deny == frozenset({"power_action"})
    assert cfg.limits.max_conversations == 4 and cfg.limits.client_tool_timeout_s == 5
    assert cfg.audit.max_bytes == 4096


@pytest.mark.parametrize(
    ("raw", "message"),
    [
        ({"local": {"temperature": 3}}, "local.temperature must be a number from 0 to 2"),
        ({"local": {"history_messages": 1}}, "local.history_messages"),
        ({"local": {"url": 5}}, "local.url must be a string"),
        ({"cloud": {"enabled": "yes"}}, "cloud.enabled must be true or false"),
        ({"cloud": {"effort": "turbo"}}, "cloud.effort must be one of"),
        ({"cloud": {"max_retries": -1}}, "cloud.max_retries"),
        ({"cloud": {"max_tokens": True}}, "cloud.max_tokens"),
        ({"policy": {"confirm": "never"}}, "policy.confirm must be one of"),
        ({"policy": {"deny": "power_action"}}, "policy.deny must be a list of strings"),
        ({"limits": {"max_steps": 0}}, "limits.max_steps"),
        ({"limits": {"max_steps": 2.5}}, "limits.max_steps"),
        ({"local": 3}, r"\[local\] must be a table"),
    ],
)
def test_invalid_values_are_rejected_with_the_setting_name(
    raw: dict[str, Any], message: str
) -> None:
    with pytest.raises(ConfigError, match=message):
        from_dict(raw)


def test_typos_are_rejected_with_a_suggestion() -> None:
    with pytest.raises(
        ConfigError, match=r"unknown setting 'local.modle' \(did you mean 'model'\?\)"
    ):
        from_dict({"local": {"modle": "x"}})
    with pytest.raises(ConfigError, match="unknown setting 'limitz'"):
        from_dict({"limitz": {}})
    with pytest.raises(ConfigError, match=r"unknown setting 'limits\.max_step'.*max_steps"):
        from_dict({"limits": {"max_step": 3}})


# ── model ladder ────────────────────────────────────────────────────────────


def test_default_ladder_follows_ram() -> None:
    pick = config.recommend_model
    assert pick(None) == "qwen2.5:3b-instruct"
    assert pick(2 * GIB) == "qwen2.5:1.5b-instruct"
    assert pick(6 * GIB) == "qwen2.5:3b-instruct"
    assert pick(14 * GIB) == "qwen2.5:7b-instruct"


def test_a_custom_ladder_replaces_the_default(monkeypatch: pytest.MonkeyPatch) -> None:
    raw = {
        "local": {
            "model_ladder": [
                {"min_ram_gb": 0, "model": "tiny"},
                {"min_ram_gb": 32, "model": "huge"},
            ]
        }
    }
    monkeypatch.setattr(config, "total_ram_bytes", lambda: 8 * GIB)
    assert from_dict(raw).local.model == "tiny"
    monkeypatch.setattr(config, "total_ram_bytes", lambda: 64 * GIB)
    assert from_dict(raw).local.model == "huge"
    # An explicit model always wins.
    raw["local"]["model"] = "pinned"
    assert from_dict(raw).local.model == "pinned"


@pytest.mark.parametrize(
    "ladder",
    [[], "x", [{"model": "a"}], [{"min_ram_gb": 1, "model": "a", "extra": 1}], [3]],
)
def test_bad_ladders_are_rejected(ladder: Any) -> None:
    with pytest.raises(ConfigError, match="model_ladder"):
        from_dict({"local": {"model_ladder": ladder}})


# ── files, paths and environment ────────────────────────────────────────────


def test_system_prompt_file(tmp_path: Path) -> None:
    (tmp_path / "prompt.txt").write_text("  You are a pirate assistant.  \n")
    cfg = from_dict({"agent": {"system_prompt_file": "prompt.txt"}}, base_dir=tmp_path)
    assert cfg.system_prompt == "You are a pirate assistant."

    with pytest.raises(ConfigError, match="cannot read"):
        from_dict({"agent": {"system_prompt_file": "missing.txt"}}, base_dir=tmp_path)
    (tmp_path / "empty.txt").write_text("\n")
    with pytest.raises(ConfigError, match="is empty"):
        from_dict({"agent": {"system_prompt_file": "empty.txt"}}, base_dir=tmp_path)


def test_audit_path_is_relative_to_the_config_file(tmp_path: Path) -> None:
    path = tmp_path / "agent.toml"
    path.write_text('[audit]\npath = "logs/audit.jsonl"\n')
    assert config.load(path).audit_log == tmp_path / "logs" / "audit.jsonl"
    assert from_dict({}).audit_log.name == "audit.jsonl"


def test_load_errors_name_the_file(tmp_path: Path) -> None:
    path = tmp_path / "agent.toml"
    path.write_text("[limits]\nmax_steps = 99\n")
    with pytest.raises(ConfigError, match=r"agent\.toml: limits\.max_steps"):
        config.load(path)


def test_paths_come_from_the_environment(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("OSAIMA_AGENT_SOCKET", "/run/custom-agent.sock")
    monkeypatch.setenv("OSAIMA_MCP_SOCKET", "/run/custom-mcp.sock")
    monkeypatch.setenv("OSAIMA_AGENT_CONFIG", str(tmp_path / "elsewhere.toml"))
    assert config.agent_socket_path() == Path("/run/custom-agent.sock")
    assert config.mcp_socket_path() == Path("/run/custom-mcp.sock")
    assert config.config_path() == tmp_path / "elsewhere.toml"
    cfg = from_dict({})
    assert cfg.socket_path == Path("/run/custom-agent.sock")


# ── the values reach the code that uses them ────────────────────────────────


async def test_ollama_uses_the_configured_options() -> None:
    sent: list[dict[str, Any]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        sent.append(json.loads(request.content))
        return httpx.Response(200, content=b'{"done": true, "done_reason": "stop"}\n')

    cfg = from_dict(
        {"local": {"model": "m", "temperature": 0.9, "keep_alive": "2h", "context_tokens": 2048}}
    ).local
    provider = OllamaProvider(
        cfg.url,
        cfg.model,
        context_tokens=cfg.context_tokens,
        temperature=cfg.temperature,
        keep_alive=cfg.keep_alive,
        transport=httpx.MockTransport(handler),
    )

    async def sink(_: str) -> None:
        return None

    await provider.complete("s", [Message("user", "hi")], SPECS, sink)
    assert sent[0]["keep_alive"] == "2h"
    assert sent[0]["options"] == {"temperature": 0.9, "num_ctx": 2048}


async def test_claude_fallback_can_be_switched_off() -> None:
    requests: list[dict[str, Any]] = []
    final = SimpleNamespace(
        content=[SimpleNamespace(type="text", text="ok")], stop_reason="end_turn", model="m"
    )

    class Stream:
        async def __aenter__(self) -> Stream:
            return self

        async def __aexit__(self, *_: object) -> None:
            return None

        def __aiter__(self) -> Any:
            async def empty() -> Any:
                return
                yield

            return empty()

        async def get_final_message(self) -> SimpleNamespace:
            return final

    class Messages:
        def stream(self, **request: Any) -> Stream:
            requests.append(request)
            return Stream()

    client = SimpleNamespace(beta=SimpleNamespace(messages=Messages()))

    async def sink(_: str) -> None:
        return None

    for fallback in (True, False):
        provider = ClaudeProvider(server_fallback=fallback, client=client)  # type: ignore[arg-type]
        await provider.complete("s", [Message("user", "hi")], SPECS, sink)
    on, off = requests
    assert on["fallbacks"] == "default" and on["betas"] == [BINDING_BETA, FALLBACK_BETA]
    assert "fallbacks" not in off and off["betas"] == [BINDING_BETA]
