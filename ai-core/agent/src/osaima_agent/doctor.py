"""`osaima-agent doctor`: check that everything the assistant needs is in
place, say how to fix what isn't, and (with --pull) download the local model."""

from __future__ import annotations

import json
import os
from collections.abc import Callable
from dataclasses import dataclass
from typing import Literal

import httpx

from .config import AgentConfig, config_path
from .mcp_client import McpClient, McpError

Status = Literal["ok", "warn", "fail", "off"]
ICONS: dict[Status, str] = {"ok": "✓", "warn": "!", "fail": "✗", "off": "-"}


@dataclass(frozen=True)
class Check:
    name: str
    status: Status
    detail: str
    fix: str = ""


async def check_ai_core(mcp: McpClient) -> Check:
    try:
        tools = await mcp.list_tools()
    except (McpError, OSError, TimeoutError) as err:
        return Check(
            "AI Core",
            "fail",
            str(err),
            "start it with `osaima-mcp-daemon` (the desktop session does this for you)",
        )
    return Check("AI Core", "ok", f"{len(tools)} tools available")


async def installed_models(client: httpx.AsyncClient) -> list[str]:
    response = await client.get("/api/tags")
    response.raise_for_status()
    return [m.get("name", "") for m in response.json().get("models", [])]


def has_model(installed: list[str], wanted: str) -> bool:
    return wanted in installed or f"{wanted}:latest" in installed


async def check_local(cfg: AgentConfig, client: httpx.AsyncClient) -> list[Check]:
    if not cfg.local.enabled:
        return [Check("Local model", "off", "disabled in the settings")]
    try:
        installed = await installed_models(client)
    except httpx.HTTPError:
        return [
            Check(
                "Ollama",
                "fail",
                f"not reachable at {cfg.local.url}",
                "start it with `ollama serve`, or install it: `emerge sci-ml/ollama`",
            )
        ]
    checks = [Check("Ollama", "ok", f"running at {cfg.local.url}")]
    if has_model(installed, cfg.local.model):
        checks.append(Check("Local model", "ok", cfg.local.model))
    else:
        checks.append(
            Check(
                "Local model",
                "fail",
                f"{cfg.local.model} is not downloaded",
                "run `osaima-agent doctor --pull`",
            )
        )
    return checks


def check_cloud(cfg: AgentConfig) -> Check:
    if not cfg.cloud.enabled:
        return Check("Cloud model", "off", "disabled; nothing leaves this computer")
    if os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_AUTH_TOKEN"):
        return Check("Cloud model", "ok", f"{cfg.cloud.model} (API key found in the environment)")
    return Check(
        "Cloud model",
        "warn",
        f"{cfg.cloud.model} is enabled but no API key is set in the environment",
        "export ANTHROPIC_API_KEY, or sign in with `ant auth login`",
    )


async def run_checks(
    cfg: AgentConfig,
    mcp: McpClient,
    ollama: httpx.AsyncClient,
) -> list[Check]:
    return [
        await check_ai_core(mcp),
        *await check_local(cfg, ollama),
        check_cloud(cfg),
        Check("Settings file", "ok" if config_path().exists() else "off", str(config_path())),
    ]


def verdict(checks: list[Check]) -> bool:
    """True when the assistant can answer: the AI Core is up and some model works."""
    by_name = {c.name: c for c in checks}
    core = by_name.get("AI Core")
    if core is None or core.status != "ok":
        return False
    local_ok = by_name.get("Local model")
    cloud_ok = by_name.get("Cloud model")
    return any(c is not None and c.status == "ok" for c in (local_ok, cloud_ok))


def render(checks: list[Check]) -> str:
    width = max(len(c.name) for c in checks)
    lines = []
    for c in checks:
        lines.append(f"  {ICONS[c.status]} {c.name:<{width}}  {c.detail}")
        if c.fix and c.status in ("fail", "warn"):
            lines.append(f"    {' ' * width}  → {c.fix}")
    return "\n".join(lines)


async def pull_model(
    client: httpx.AsyncClient, model: str, on_progress: Callable[[str], None]
) -> None:
    """Download `model` through Ollama, reporting progress lines. Raises RuntimeError on failure."""
    last = ""
    try:
        async with client.stream(
            "POST", "/api/pull", json={"model": model, "stream": True}, timeout=None
        ) as response:
            if response.status_code >= 400:
                detail = (await response.aread()).decode(errors="replace")
                raise RuntimeError(
                    f"Ollama refused the download ({response.status_code}): {detail}"
                )
            async for line in response.aiter_lines():
                if not line.strip():
                    continue
                event = json.loads(line)
                if error := event.get("error"):
                    raise RuntimeError(f"download failed: {error}")
                text = progress_text(event)
                if text != last:
                    on_progress(text)
                    last = text
    except httpx.HTTPError as err:
        raise RuntimeError(f"could not reach Ollama: {err}") from err


def progress_text(event: dict[str, object]) -> str:
    status = str(event.get("status", ""))
    total, done = event.get("total"), event.get("completed")
    if isinstance(total, int) and isinstance(done, int) and total > 0:
        return f"{status} {done * 100 // total}% ({done / 1e9:.1f} of {total / 1e9:.1f} GB)"
    return status
