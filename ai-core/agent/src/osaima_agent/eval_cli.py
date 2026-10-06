"""Command line for the evaluation harness: `osaima-agent eval` and `eval-tools`."""

from __future__ import annotations

import argparse
import dataclasses
import json
import re
import sys
import time
from pathlib import Path
from typing import Any

from . import config
from .audit import AuditLog
from .evaluate import (
    EVALS_DIR,
    EvalError,
    load_cases,
    load_setup,
    render_markdown,
    run_eval,
    summarize,
    validate_cases,
    write_tool_snapshot,
)
from .llm import Provider
from .llm.claude import ClaudeProvider
from .llm.ollama import OllamaProvider
from .mcp_client import McpClient, McpError


def add_parsers(sub: Any) -> None:
    ev = sub.add_parser("eval", help="measure how well a model chooses tools")
    ev.add_argument("--provider", choices=["local", "cloud"], default="local")
    ev.add_argument("--model", help="model name (default: from the settings)")
    ev.add_argument("--cases", type=Path, default=EVALS_DIR / "cases.jsonl")
    ev.add_argument("--tools", type=Path, default=EVALS_DIR / "tools.snapshot.json")
    ev.add_argument("--shell-tools", type=Path, default=EVALS_DIR / "shell-tools.snapshot.json")
    ev.add_argument("--fixtures", type=Path, default=EVALS_DIR / "fixtures.json")
    ev.add_argument("--category", action="append", help="only these categories (repeatable)")
    ev.add_argument("--limit", type=int, help="only the first N cases")
    ev.add_argument("--out", type=Path, help="results folder (default: evals/results/<run>)")
    ev.add_argument("--validate-only", action="store_true", help="check the data, run nothing")
    ev.add_argument(
        "--confirm-cost",
        action="store_true",
        help="required for --provider cloud: every case is a paid API request",
    )
    tools = sub.add_parser(
        "eval-tools", help="save the AI Core's tool definitions for the evaluation"
    )
    tools.add_argument("--write", type=Path, required=True)


async def run_eval_tools(cfg: config.AgentConfig, path: Path) -> int:
    mcp = McpClient(cfg.mcp_socket_path, cfg.limits.core_request_timeout_s)
    try:
        count = await write_tool_snapshot(mcp.list_tools, path)
    except McpError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    finally:
        await mcp.aclose()
    print(f"wrote {count} tool definitions to {path}")
    return 0


def _provider(cfg: config.AgentConfig, kind: str, model: str | None) -> Provider:
    if kind == "local":
        local = dataclasses.replace(cfg.local, model=model) if model else cfg.local
        return OllamaProvider.from_config(local)
    cloud = dataclasses.replace(cfg.cloud, model=model) if model else cfg.cloud
    return ClaudeProvider.from_config(cloud)


async def run_eval_command(cfg: config.AgentConfig, args: argparse.Namespace) -> int:
    try:
        cases = load_cases(args.cases)
        setup = load_setup(args.tools, args.shell_tools, args.fixtures)
    except (EvalError, OSError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    problems = validate_cases(cases, [*setup.system_specs, *setup.client_specs])
    if problems:
        print("the evaluation data has problems:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    if args.category:
        cases = [c for c in cases if c.category in args.category]
    if args.limit:
        cases = cases[: args.limit]
    if not cases:
        print("error: no cases selected", file=sys.stderr)
        return 2
    if args.validate_only:
        print(f"{len(cases)} cases, all consistent with {len(setup.known_tools)} tools")
        return 0
    if args.provider == "cloud" and not args.confirm_cost:
        print(
            f"error: this sends {len(cases)} requests to the Claude API, which costs money. "
            "Add --confirm-cost to go ahead.",
            file=sys.stderr,
        )
        return 2

    provider = _provider(cfg, args.provider, args.model)
    if not await provider.available():
        print(
            f"error: the {args.provider} model {provider.model} is not available", file=sys.stderr
        )
        return 1

    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", f"{args.provider}-{provider.model}").strip("-")
    out = args.out or EVALS_DIR / "results" / f"{time.strftime('%Y%m%d-%H%M%S')}-{slug}"
    out.mkdir(parents=True, exist_ok=True)
    audit = AuditLog(out / "audit.jsonl")

    def progress(done: int, total: int, result: dict[str, Any]) -> None:
        mark = "ok  " if result["passed"] else "FAIL"
        print(
            f"[{done:>3}/{total}] {mark} {result['id']:<12} {result['latency_s']:>6.1f}s",
            flush=True,
        )

    results = await run_eval(
        cases, setup, provider, args.provider, audit, progress, limits=cfg.limits
    )
    summary = summarize(results)
    meta = {
        "provider": args.provider,
        "model": provider.model,
        "date": time.strftime("%Y-%m-%d"),
    }
    with (out / "results.jsonl").open("w", encoding="utf-8") as handle:
        for result in results:
            handle.write(json.dumps(result, ensure_ascii=False) + "\n")
    (out / "summary.json").write_text(
        json.dumps({"meta": meta, "summary": summary}, indent=2) + "\n", encoding="utf-8"
    )
    report = render_markdown(summary, results, meta)
    (out / "summary.md").write_text(report, encoding="utf-8")
    print()
    print(report)
    print(f"results saved in {out}")
    return 0
