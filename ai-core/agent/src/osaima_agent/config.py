"""Agent settings from `~/.config/osaima/agent.toml`, with safe defaults.

Example file::

    [local]
    url = "http://127.0.0.1:11434"
    model = "qwen2.5:7b-instruct"

    [cloud]
    enabled = true            # off by default: nothing leaves the machine
    model = "claude-opus-5-5"
    effort = "medium"

    [policy]
    confirm = "destructive"   # or "all_changes"
    deny = ["power_action"]

The Claude API key comes from the environment (`ANTHROPIC_API_KEY`) or an
`ant auth login` profile, never from this file.
"""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

ConfirmMode = Literal["destructive", "all_changes"]
EFFORTS = ("low", "medium", "high", "xhigh", "max")


@dataclass(frozen=True)
class LocalConfig:
    enabled: bool = True
    url: str = "http://127.0.0.1:11434"
    model: str = "qwen2.5:7b-instruct"
    context_tokens: int = 8192
    timeout_s: float = 120.0


@dataclass(frozen=True)
class CloudConfig:
    enabled: bool = False
    model: str = "claude-opus-5-5"
    effort: str = "medium"
    max_tokens: int = 16000


@dataclass(frozen=True)
class PolicyConfig:
    confirm: ConfirmMode = "destructive"
    deny: frozenset[str] = frozenset()
    approval_timeout_s: float = 120.0


@dataclass(frozen=True)
class AgentConfig:
    local: LocalConfig = field(default_factory=LocalConfig)
    cloud: CloudConfig = field(default_factory=CloudConfig)
    policy: PolicyConfig = field(default_factory=PolicyConfig)
    max_steps: int = 6
    socket_path: Path = field(default_factory=lambda: runtime_dir() / "agent.sock")
    mcp_socket_path: Path = field(default_factory=lambda: mcp_socket_path())
    audit_log: Path = field(default_factory=lambda: state_dir() / "audit.jsonl")


class ConfigError(ValueError):
    """The configuration file is invalid."""


def runtime_dir() -> Path:
    """`$XDG_RUNTIME_DIR/osaima`, else `/tmp/osaima-<uid>` (matches the daemon)."""
    if xdg := os.environ.get("XDG_RUNTIME_DIR"):
        return Path(xdg) / "osaima"
    return Path(f"/tmp/osaima-{os.geteuid()}")


def mcp_socket_path() -> Path:
    """Same lookup as the AI Core daemon (`ai-core/mcp-daemon/src/paths.rs`)."""
    if override := os.environ.get("OSAIMA_MCP_SOCKET"):
        return Path(override)
    return runtime_dir() / "mcp.sock"


def state_dir() -> Path:
    base = os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")
    return Path(base) / "osaima"


def config_path() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    return Path(base) / "osaima" / "agent.toml"


def load(path: Path | None = None) -> AgentConfig:
    """Load settings; a missing file means all defaults."""
    path = path or config_path()
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return AgentConfig()
    except tomllib.TOMLDecodeError as err:
        raise ConfigError(f"{path}: {err}") from err
    return from_dict(raw)


def from_dict(raw: dict[str, Any]) -> AgentConfig:
    local = _section(raw, "local")
    cloud = _section(raw, "cloud")
    policy = _section(raw, "policy")
    defaults = AgentConfig()

    effort = str(cloud.get("effort", defaults.cloud.effort))
    if effort not in EFFORTS:
        raise ConfigError(f"cloud.effort must be one of {EFFORTS}, got {effort!r}")
    confirm = policy.get("confirm", defaults.policy.confirm)
    if confirm not in ("destructive", "all_changes"):
        raise ConfigError("policy.confirm must be 'destructive' or 'all_changes'")
    max_steps = int(raw.get("agent", {}).get("max_steps", defaults.max_steps))
    if not 1 <= max_steps <= 20:
        raise ConfigError("agent.max_steps must be between 1 and 20")

    return AgentConfig(
        local=LocalConfig(
            enabled=bool(local.get("enabled", defaults.local.enabled)),
            url=str(local.get("url", defaults.local.url)).rstrip("/"),
            model=str(local.get("model", defaults.local.model)),
            context_tokens=int(local.get("context_tokens", defaults.local.context_tokens)),
            timeout_s=float(local.get("timeout_s", defaults.local.timeout_s)),
        ),
        cloud=CloudConfig(
            enabled=bool(cloud.get("enabled", defaults.cloud.enabled)),
            model=str(cloud.get("model", defaults.cloud.model)),
            effort=effort,
            max_tokens=int(cloud.get("max_tokens", defaults.cloud.max_tokens)),
        ),
        policy=PolicyConfig(
            confirm=confirm,
            deny=frozenset(str(name) for name in policy.get("deny", [])),
            approval_timeout_s=float(
                policy.get("approval_timeout_s", defaults.policy.approval_timeout_s)
            ),
        ),
        max_steps=max_steps,
    )


def _section(raw: dict[str, Any], name: str) -> dict[str, Any]:
    value = raw.get(name, {})
    if not isinstance(value, dict):
        raise ConfigError(f"[{name}] must be a table")
    return value
