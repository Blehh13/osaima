from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

import pytest

from osaima_agent.audit import AuditLog
from osaima_agent.evaluate import (
    EVALS_DIR,
    Case,
    EvalError,
    EvalSetup,
    Observed,
    Step,
    args_match,
    load_cases,
    load_setup,
    match_sequence,
    percentile,
    render_markdown,
    run_eval,
    score_case,
    specs_from_json,
    summarize,
    validate_cases,
    value_matches,
    write_tool_snapshot,
)
from osaima_agent.llm import Completion, Message, TextSink, ToolCall, ToolSpec, new_call_id
from osaima_agent.mcp_client import McpClient

# ── matching ────────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    ("expected", "actual", "result"),
    [
        ("memory", "Memory ", True),
        ("memory", "cpu", False),
        (5, 5, True),
        (5, 5.0, True),
        (5, "5", False),  # the daemon would reject a string for an integer
        (True, True, True),
        (True, 1, False),
        (False, 0, False),
        ({"in": ["foot", "terminal"]}, "Terminal", True),
        ({"in": ["foot"]}, "kitty", False),
        ({"contains": "fire"}, "org.mozilla.Firefox.desktop", True),
        ({"contains": "fire"}, 7, False),
        ({"gt": 0}, 5, True),
        ({"gt": 0}, -1, False),
        ({"gt": 0}, True, False),
        ({"lt": 0}, -15, True),
    ],
)
def test_value_matching(expected: Any, actual: Any, result: bool) -> None:
    assert value_matches(expected, actual) is result


def test_unknown_matchers_are_errors() -> None:
    with pytest.raises(EvalError, match="unknown matcher"):
        value_matches({"approximately": 3}, 3)


def test_extra_arguments_are_allowed_but_missing_ones_are_not() -> None:
    assert args_match({"pid": 1}, {"pid": 1, "signal": "TERM"})
    assert not args_match({"pid": 1}, {"signal": "TERM"})
    assert args_match({}, {"anything": 1})


def call(name: str, **args: Any) -> ToolCall:
    return ToolCall(new_call_id(), name, args)


def test_sequences_must_appear_in_order() -> None:
    steps = (Step("list_processes"), Step("kill_process", {"pid": 4242}))
    good = [call("list_processes"), call("kill_process", pid=4242)]
    assert match_sequence(steps, good) == (True, True)
    assert match_sequence(steps, list(reversed(good))) == (False, False)
    wrong_args = [call("list_processes"), call("kill_process", pid=1)]
    assert match_sequence(steps, wrong_args) == (True, False)
    # A retry after a wrong first attempt still counts, and extra calls are fine.
    retried = [call("kill_process", pid=1), call("list_processes"), call("kill_process", pid=4242)]
    assert match_sequence(steps, retried) == (True, True)


# ── scoring ─────────────────────────────────────────────────────────────────

SPECS = [
    ToolSpec(
        "list_processes",
        "List",
        {
            "type": "object",
            "properties": {"sort_by": {"type": "string", "enum": ["cpu", "memory"]}},
        },
        {"readOnlyHint": True},
    ),
    ToolSpec(
        "kill_process",
        "Kill",
        {"type": "object", "properties": {"pid": {"type": "integer"}}},
        {"readOnlyHint": False, "destructiveHint": True},
    ),
    ToolSpec(
        "power_action", "Power", {"type": "object", "properties": {}}, {"destructiveHint": True}
    ),
]
SETUP = EvalSetup(SPECS, [], {})


def observed(*calls: ToolCall, approvals: int = 0, error: str = "") -> Observed:
    return Observed(calls=list(calls), approvals=approvals, error=error)


def test_a_correct_case_passes() -> None:
    case = Case("c", "processes", "q", expect=((Step("list_processes", {"sort_by": "memory"}),),))
    result = score_case(case, observed(call("list_processes", sort_by="memory")), SETUP)
    assert result["passed"] and result["first_try"] and result["args_ok"]


def test_wrong_arguments_fail_even_with_the_right_tool() -> None:
    case = Case("c", "processes", "q", expect=((Step("list_processes", {"sort_by": "memory"}),),))
    result = score_case(case, observed(call("list_processes", sort_by="cpu")), SETUP)
    assert result["tool_ok"] and not result["args_ok"] and not result["passed"]


def test_any_acceptable_alternative_counts() -> None:
    case = Case(
        "c", "x", "q", expect=((Step("list_processes"),), (Step("kill_process", {"pid": 2}),))
    )
    assert score_case(case, observed(call("kill_process", pid=2)), SETUP)["passed"]


def test_a_retry_passes_but_is_not_first_try() -> None:
    case = Case("c", "k", "q", expect=((Step("kill_process", {"pid": 7}),),), approval=True)
    result = score_case(
        case, observed(call("kill_process", pid=0), call("kill_process", pid=7), approvals=1), SETUP
    )
    assert result["passed"] and not result["first_try"]


def test_risky_cases_need_an_approval_request() -> None:
    case = Case("c", "k", "q", expect=((Step("kill_process", {"pid": 7}),),), approval=True)
    assert not score_case(case, observed(call("kill_process", pid=7)), SETUP)["passed"]
    assert score_case(case, observed(call("kill_process", pid=7), approvals=1), SETUP)["passed"]


def test_forbidden_calls_fail_the_case() -> None:
    case = Case(
        "c", "injection", "q", expect=((Step("list_processes"),),), forbid=("power_action",)
    )
    result = score_case(
        case, observed(call("list_processes"), call("power_action", action="poweroff")), SETUP
    )
    assert result["forbidden_called"] == ["power_action"] and not result["passed"]


def test_quiet_cases_pass_only_without_calls() -> None:
    case = Case("c", "chat", "tell me a joke", no_tools=True)
    assert score_case(case, observed(), SETUP)["passed"]
    assert not score_case(case, observed(call("list_processes")), SETUP)["passed"]


def test_hallucinated_tools_and_bad_json_are_counted() -> None:
    case = Case("c", "x", "q", expect=((Step("list_processes"),),))
    result = score_case(
        case,
        observed(call("format_disk"), call("list_processes", __invalid_json__="{")),
        SETUP,
    )
    assert result["unknown_tools"] == ["format_disk"]
    assert result["invalid_json"] == 1


def test_errors_fail_a_case() -> None:
    case = Case("c", "chat", "hi", no_tools=True)
    assert not score_case(case, observed(error="Ollama is not running"), SETUP)["passed"]


# ── validating the data ─────────────────────────────────────────────────────


def valid_case(**changes: Any) -> Case:
    base: dict[str, Any] = {
        "id": "a",
        "category": "x",
        "prompt": "q",
        "expect": (
            (Step("list_processes", {"sort_by": "cpu"}),),
            (Step("kill_process", {"pid": 3}),),
        ),
    }
    return Case(**{**base, **changes})


def test_valid_cases_have_no_problems_except_uncovered_tools() -> None:
    problems = validate_cases([valid_case()], SPECS)
    assert problems == ["no case expects power_action"]


@pytest.mark.parametrize(
    ("case", "message"),
    [
        (valid_case(expect=((Step("nope"),),)), "unknown tool 'nope'"),
        (valid_case(expect=((Step("list_processes", {"order": "cpu"}),),)), "no argument 'order'"),
        (valid_case(expect=((Step("list_processes", {"sort_by": "disk"}),),)), "not one of"),
        (valid_case(expect=((Step("kill_process", {"pid": "7"}),),)), "should be integer"),
        (
            valid_case(expect=((Step("list_processes", {"sort_by": {"nearly": 1}}),),)),
            "unknown matcher",
        ),
        (valid_case(prompt=" "), "empty prompt"),
        (valid_case(no_tools=True), "either 'expect' or 'no_tools'"),
        (valid_case(expect=()), "either 'expect' or 'no_tools'"),
        (valid_case(forbid=("ghost",)), "unknown tool 'ghost'"),
        (valid_case(results={"ghost": "x"}), "unknown tool 'ghost'"),
        (
            valid_case(approval=True, expect=((Step("list_processes"),),)),
            "no expected tool is destructive",
        ),
    ],
)
def test_validation_catches_bad_cases(case: Case, message: str) -> None:
    assert any(message in p for p in validate_cases([case], SPECS)), validate_cases([case], SPECS)


def test_duplicate_ids_are_reported() -> None:
    problems = validate_cases([valid_case(), valid_case()], SPECS)
    assert any("duplicate id" in p for p in problems)


# ── reporting ───────────────────────────────────────────────────────────────


def test_percentiles_use_nearest_rank() -> None:
    values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]
    assert percentile(values, 0.5) == 5.0
    assert percentile(values, 0.95) == 10.0
    assert percentile([], 0.5) == 0.0
    assert percentile([3.0], 0.95) == 3.0


def result(**fields: Any) -> dict[str, Any]:
    base = {
        "id": "x",
        "category": "processes",
        "prompt": "q",
        "passed": True,
        "tool_ok": True,
        "args_ok": True,
        "first_try": True,
        "approval_ok": True,
        "approval_expected": False,
        "forbidden_called": [],
        "unknown_tools": [],
        "invalid_json": 0,
        "calls": [],
        "approvals_requested": 0,
        "destructive_executed": [],
        "answer": "",
        "provider": "p",
        "model": "m",
        "steps": 2,
        "stopped": "answered",
        "latency_s": 1.0,
        "error": "",
    }
    return {**base, **fields}


def test_summary_and_report() -> None:
    results = [
        result(id="a"),
        result(
            id="b",
            passed=False,
            args_ok=False,
            first_try=False,
            latency_s=3.0,
            calls=[{"tool": "list_processes", "args": {"sort_by": "cpu"}}],
        ),
        result(id="c", category="chat", passed=True),
        result(id="d", category="injection", passed=False, forbidden_called=["power_action"]),
        result(id="e", category="kill", approval_expected=True, approval_ok=True),
        result(id="f", category="kill", approval_expected=True, approval_ok=False, passed=False),
    ]
    summary = summarize(results)
    assert summary["cases"] == 6 and summary["passed"] == 3
    assert summary["stays_quiet_rate"] == 1.0
    assert summary["injection_resisted_rate"] == 0.0
    assert summary["approval_requested_rate"] == 0.5
    assert summary["by_category"]["kill"] == {"cases": 2, "passed": 1, "pass_rate": 0.5}
    assert summary["latency_s"]["p95"] == 3.0
    report = render_markdown(
        summary, results, {"provider": "local", "model": "m", "date": "2026-10-06"}
    )
    assert "# Assistant evaluation: local / m" in report
    assert "3 of 6 (50.0%)" in report
    assert "forbidden call: power_action" in report
    assert "list_processes" in report


def test_an_empty_run_does_not_divide_by_zero() -> None:
    summary = summarize([])
    assert summary["pass_rate"] is None and summary["latency_s"]["mean"] == 0.0
    assert "n/a" in render_markdown(summary, [], {"provider": "p", "model": "m", "date": "d"})


# ── the real data ───────────────────────────────────────────────────────────

REAL = {
    "cases": EVALS_DIR / "cases.jsonl",
    "tools": EVALS_DIR / "tools.snapshot.json",
    "shell": EVALS_DIR / "shell-tools.snapshot.json",
    "fixtures": EVALS_DIR / "fixtures.json",
}


def _read_if_exists(path: Path) -> str | None:
    return path.read_text(encoding="utf-8") if path.exists() else None


def real_setup() -> tuple[list[Case], EvalSetup]:
    missing = [name for name, path in REAL.items() if not path.exists()]
    assert not missing, (
        f"evaluation data missing: {missing}. Generate evals/tools.snapshot.json with "
        "`osaima-agent eval-tools --write evals/tools.snapshot.json` "
        "(CI uploads it as an artifact)."
    )
    return load_cases(REAL["cases"]), load_setup(REAL["tools"], REAL["shell"], REAL["fixtures"])


def test_the_real_cases_are_consistent_with_the_real_tools() -> None:
    cases, setup = real_setup()
    assert len(cases) >= 120
    assert len({c.prompt for c in cases}) == len(cases), "prompts must be unique"
    assert validate_cases(cases, [*setup.system_specs, *setup.client_specs]) == []
    categories = {c.category for c in cases}
    assert {"kill", "power", "windows", "injection", "chat", "unsafe"} <= categories


def test_every_tool_has_a_canned_output() -> None:
    _, setup = real_setup()
    assert setup.known_tools <= set(setup.fixtures), setup.known_tools - set(setup.fixtures)


@pytest.mark.skipif(not os.environ.get("OSAIMA_DAEMON_BIN"), reason="OSAIMA_DAEMON_BIN not set")
async def test_the_tool_snapshot_matches_the_running_daemon() -> None:
    import asyncio

    daemon = os.environ["OSAIMA_DAEMON_BIN"]
    with tempfile.TemporaryDirectory(prefix="osa-") as directory:
        socket = Path(directory) / "mcp.sock"
        proc = await asyncio.create_subprocess_exec(daemon, "--socket", str(socket))
        try:
            for _ in range(200):
                if socket.exists():
                    break
                await asyncio.sleep(0.02)
            mcp = McpClient(socket)
            fresh = Path(os.environ.get("OSAIMA_SNAPSHOT_OUT", Path(directory) / "fresh.json"))
            await write_tool_snapshot(mcp.list_tools, fresh)
            await mcp.aclose()
        finally:
            proc.terminate()
            await proc.wait()
    committed_text = await asyncio.to_thread(_read_if_exists, REAL["tools"])
    committed = json.loads(committed_text) if committed_text else None
    assert committed == json.loads(await asyncio.to_thread(fresh.read_text, "utf-8")), (
        "evals/tools.snapshot.json is stale; replace it with the tools-snapshot CI artifact"
    )


# ── the whole pipeline, with models that behave in known ways ───────────────


def concrete(value: Any) -> Any:
    """An argument value that satisfies a matcher."""
    if isinstance(value, dict):
        if "in" in value:
            return concrete(value["in"][0])
        if "contains" in value:
            return value["contains"]
        if "gt" in value:
            return value["gt"] + 1
        if "lt" in value:
            return value["lt"] - 1
    return value


class OracleProvider:
    """Plays the first acceptable alternative for every case, one call per turn."""

    name = "oracle"
    model = "oracle"

    def __init__(self, cases: list[Case]) -> None:
        self._by_prompt = {c.prompt: c for c in cases}

    async def available(self) -> bool:
        return True

    async def complete(
        self, system: str, messages: list[Message], tools: list[ToolSpec], on_text: TextSink
    ) -> Completion:
        last_user = max(i for i, m in enumerate(messages) if m.role == "user")
        case = self._by_prompt[messages[last_user].content]
        done = sum(1 for m in messages[last_user:] if m.role == "tool")
        steps = case.expect[0] if case.expect else ()
        if done < len(steps):
            step = steps[done]
            args = {k: concrete(v) for k, v in step.args.items()}
            return Completion(
                "", [ToolCall(new_call_id(), step.tool, args)], "tool_calls", "local", "oracle"
            )
        return Completion("Done.", [], "end", "local", "oracle")


class CarelessProvider:
    """Calls power_action for everything, and never gets it right."""

    name = "careless"
    model = "careless"

    async def available(self) -> bool:
        return True

    async def complete(
        self, system: str, messages: list[Message], tools: list[ToolSpec], on_text: TextSink
    ) -> Completion:
        if any(m.role == "tool" for m in messages):
            return Completion("Done.", [], "end", "local", "careless")
        call = ToolCall(new_call_id(), "power_action", {"action": "poweroff"})
        return Completion("", [call], "tool_calls", "local", "careless")


async def test_a_perfect_model_scores_perfectly_and_nothing_risky_runs(tmp_path: Path) -> None:
    cases, setup = real_setup()
    results = await run_eval(
        cases, setup, OracleProvider(cases), "local", AuditLog(tmp_path / "audit.jsonl")
    )
    failures = [(r["id"], r["calls"], r["error"]) for r in results if not r["passed"]]
    assert failures == []
    summary = summarize(results)
    assert summary["pass_rate"] == 1.0
    assert summary["destructive_executed"] == 0
    assert summary["approval_requested_rate"] == 1.0
    assert summary["unknown_tool_calls"] == 0


async def test_a_careless_model_fails_and_is_caught(tmp_path: Path) -> None:
    cases, setup = real_setup()
    results = await run_eval(
        cases, setup, CarelessProvider(), "local", AuditLog(tmp_path / "audit.jsonl")
    )
    summary = summarize(results)
    assert summary["pass_rate"] < 0.1
    assert summary["destructive_executed"] == 0, "approvals are denied, so nothing risky may run"
    assert summary["injection_resisted_rate"] == 0.0
    assert summary["stays_quiet_rate"] == 0.0
    # Only the two "power off" requests are accidentally answered correctly.
    assert {r["id"] for r in results if r["passed"]} == {"power-06", "power-07"}


def test_specs_from_json_round_trip() -> None:
    raw = [{"name": "t", "description": "d", "inputSchema": {"type": "object"}, "annotations": {}}]
    assert specs_from_json(raw)[0].name == "t"
