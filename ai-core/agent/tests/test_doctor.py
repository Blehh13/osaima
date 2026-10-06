from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pytest

from osaima_agent import config, doctor
from osaima_agent.mcp_client import McpClient, McpError

GIB = 1024**3


def client(handler: Callable[[httpx.Request], httpx.Response]) -> httpx.AsyncClient:
    return httpx.AsyncClient(base_url="http://ollama.test", transport=httpx.MockTransport(handler))


class FakeMcp(McpClient):
    def __init__(self, tools: int | Exception) -> None:
        super().__init__(Path("/nonexistent"))
        self._count = tools

    async def list_tools(self) -> list[Any]:
        if isinstance(self._count, Exception):
            raise self._count
        return [object()] * self._count


def cfg(**overrides: Any) -> config.AgentConfig:
    return config.from_dict(overrides)


@pytest.mark.parametrize(
    ("ram_gib", "model"),
    [
        (None, "qwen2.5:3b-instruct"),
        (2, "qwen2.5:1.5b-instruct"),
        (5.9, "qwen2.5:1.5b-instruct"),
        (6, "qwen2.5:3b-instruct"),
        (8, "qwen2.5:3b-instruct"),
        (16, "qwen2.5:7b-instruct"),
        (64, "qwen2.5:7b-instruct"),
    ],
)
def test_model_recommendation_follows_ram(ram_gib: float | None, model: str) -> None:
    ram = None if ram_gib is None else int(ram_gib * GIB)
    assert config.recommend_model(ram) == model


def test_an_explicit_model_beats_the_recommendation() -> None:
    assert cfg(local={"model": "llama3.2:3b"}).local.model == "llama3.2:3b"


async def test_everything_ready() -> None:
    tags = {"models": [{"name": "qwen2.5:7b-instruct"}]}
    checks = await doctor.run_checks(
        cfg(local={"model": "qwen2.5:7b-instruct"}),
        FakeMcp(18),
        client(lambda r: httpx.Response(200, json=tags)),
    )
    by_name = {c.name: c for c in checks}
    assert by_name["AI Core"].status == "ok"
    assert by_name["AI Core"].detail == "18 tools available"
    assert by_name["Ollama"].status == "ok"
    assert by_name["Local model"].status == "ok"
    assert by_name["Cloud model"].status == "off"
    assert doctor.verdict(checks)


async def test_each_failure_comes_with_a_fix() -> None:
    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    checks = await doctor.run_checks(
        cfg(), FakeMcp(McpError("AI Core is not running")), client(refuse)
    )
    failures = [c for c in checks if c.status == "fail"]
    assert {c.name for c in failures} == {"AI Core", "Ollama"}
    assert all(c.fix for c in failures)
    assert not doctor.verdict(checks)
    text = doctor.render(checks)
    assert "✗ AI Core" in text and "→ start it with" in text


async def test_missing_model_points_to_pull() -> None:
    checks = await doctor.run_checks(
        cfg(local={"model": "qwen2.5:7b-instruct"}),
        FakeMcp(3),
        client(lambda r: httpx.Response(200, json={"models": [{"name": "llama3.2:3b"}]})),
    )
    local = next(c for c in checks if c.name == "Local model")
    assert local.status == "fail"
    assert "--pull" in local.fix
    assert not doctor.verdict(checks)


async def test_cloud_alone_is_enough(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")

    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    checks = await doctor.run_checks(cfg(cloud={"enabled": True}), FakeMcp(3), client(refuse))
    assert next(c for c in checks if c.name == "Cloud model").status == "ok"
    assert doctor.verdict(checks)


def test_cloud_without_a_key_warns(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("ANTHROPIC_AUTH_TOKEN", raising=False)
    check = doctor.check_cloud(cfg(cloud={"enabled": True}))
    assert check.status == "warn"
    assert "ant auth login" in check.fix


async def test_pull_reports_progress_once_per_change() -> None:
    lines = [
        {"status": "pulling manifest"},
        {"status": "pulling abc", "total": 4_000_000_000, "completed": 1_000_000_000},
        {"status": "pulling abc", "total": 4_000_000_000, "completed": 1_000_000_001},
        {"status": "pulling abc", "total": 4_000_000_000, "completed": 4_000_000_000},
        {"status": "success"},
    ]
    seen: list[str] = []
    body = b"".join(json.dumps(e).encode() + b"\n" for e in lines)
    async with client(lambda r: httpx.Response(200, content=body)) as http:
        await doctor.pull_model(http, "qwen2.5:3b-instruct", seen.append)
    assert seen == [
        "pulling manifest",
        "pulling abc 25% (1.0 of 4.0 GB)",
        "pulling abc 100% (4.0 of 4.0 GB)",
        "success",
    ]


async def test_pull_failures_raise() -> None:
    error = json.dumps({"error": "pull model manifest: file does not exist"}).encode()
    async with client(lambda r: httpx.Response(200, content=error + b"\n")) as http:
        with pytest.raises(RuntimeError, match="file does not exist"):
            await doctor.pull_model(http, "nope", lambda _: None)
    async with client(lambda r: httpx.Response(500, text="boom")) as http:
        with pytest.raises(RuntimeError, match="500"):
            await doctor.pull_model(http, "x", lambda _: None)

    def refuse(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("refused", request=request)

    async with client(refuse) as http:
        with pytest.raises(RuntimeError, match="could not reach Ollama"):
            await doctor.pull_model(http, "x", lambda _: None)
