from __future__ import annotations

from pathlib import Path

import pytest
from conftest import SPECS

from osaima_agent import config
from osaima_agent.audit import AuditLog
from osaima_agent.llm import ToolSpec
from osaima_agent.policy import Decision, Policy

STATS, VOLUME, KILL = SPECS


def test_defaults_keep_everything_local(tmp_path: Path) -> None:
    cfg = config.load(tmp_path / "missing.toml")
    assert cfg.local.enabled
    assert not cfg.cloud.enabled
    assert cfg.cloud.model == "claude-opus-5-5"
    assert cfg.policy.confirm == "destructive"


def test_loads_a_settings_file(tmp_path: Path) -> None:
    path = tmp_path / "agent.toml"
    path.write_text(
        '[local]\nmodel = "llama3.2:3b"\nurl = "http://box:11434/"\n'
        '[cloud]\nenabled = true\neffort = "low"\n'
        '[policy]\nconfirm = "all_changes"\ndeny = ["power_action"]\n'
        "[limits]\nmax_steps = 4\n"
    )
    cfg = config.load(path)
    assert cfg.local.model == "llama3.2:3b"
    assert cfg.local.url == "http://box:11434"
    assert cfg.cloud.enabled and cfg.cloud.effort == "low"
    assert cfg.policy.deny == frozenset({"power_action"})
    assert cfg.max_steps == 4


@pytest.mark.parametrize(
    "text",
    [
        '[cloud]\neffort = "turbo"\n',
        '[policy]\nconfirm = "never"\n',
        "[limits]\nmax_steps = 0\n",
        "local = 3\n",
        "[local\n",
    ],
)
def test_rejects_invalid_settings(tmp_path: Path, text: str) -> None:
    path = tmp_path / "agent.toml"
    path.write_text(text)
    with pytest.raises(config.ConfigError):
        config.load(path)


def test_socket_paths_follow_xdg(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.delenv("OSAIMA_MCP_SOCKET", raising=False)
    assert config.mcp_socket_path() == Path("/run/user/1000/osaima/mcp.sock")
    monkeypatch.setenv("OSAIMA_MCP_SOCKET", "/x.sock")
    assert config.mcp_socket_path() == Path("/x.sock")


def test_policy_decisions() -> None:
    policy = Policy()
    assert policy.decide(STATS) is Decision.ALLOW
    assert policy.decide(VOLUME) is Decision.ALLOW
    assert policy.decide(KILL) is Decision.CONFIRM
    assert Policy("all_changes").decide(VOLUME) is Decision.CONFIRM
    assert Policy(deny=["get_system_stats"]).decide(STATS) is Decision.DENY


def test_unannotated_tools_are_treated_as_destructive() -> None:
    bare = ToolSpec(name="mystery", description="?", input_schema={"type": "object"})
    assert bare.destructive
    assert Policy().decide(bare) is Decision.CONFIRM


def test_audit_log_rotates_and_tails(tmp_path: Path) -> None:
    log = AuditLog(tmp_path / "state" / "audit.jsonl", max_bytes=200)
    for i in range(20):
        log.record(tool="t", n=i)
    assert (tmp_path / "state" / "audit.jsonl.1").exists()
    entries = log.tail(3)
    assert [e["n"] for e in entries] == [17, 18, 19]
    assert oct((tmp_path / "state" / "audit.jsonl").stat().st_mode & 0o777) == "0o600"
