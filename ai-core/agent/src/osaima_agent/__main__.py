"""Command line: `osaima-agent` (run the service) or `osaima-agent ask "..."`."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from pathlib import Path

import httpx

from . import __version__, config, doctor
from .agent import Agent, Conversation
from .audit import AuditLog
from .llm import Provider, ToolCall, ToolSpec
from .llm.claude import ClaudeProvider
from .llm.ollama import OllamaProvider
from .mcp_client import McpClient, McpError
from .policy import Policy
from .server import AgentServer


def build_agent(cfg: config.AgentConfig, mcp: McpClient) -> tuple[Agent, AuditLog]:
    local: Provider | None = None
    if cfg.local.enabled:
        local = OllamaProvider(
            cfg.local.url,
            cfg.local.model,
            context_tokens=cfg.local.context_tokens,
            timeout_s=cfg.local.timeout_s,
        )
    cloud: Provider | None = None
    if cfg.cloud.enabled:
        cloud = ClaudeProvider(
            cfg.cloud.model, effort=cfg.cloud.effort, max_tokens=cfg.cloud.max_tokens
        )
    audit = AuditLog(cfg.audit_log)
    agent = Agent(
        tools=mcp,
        local=local,
        cloud=cloud,
        policy=Policy(cfg.policy.confirm, cfg.policy.deny),
        audit=audit,
        max_steps=cfg.max_steps,
        approval_timeout_s=cfg.policy.approval_timeout_s,
    )
    return agent, audit


async def serve(cfg: config.AgentConfig, socket_path: Path) -> None:
    mcp = McpClient(cfg.mcp_socket_path)
    agent, audit = build_agent(cfg, mcp)
    try:
        await AgentServer(agent, audit).serve(socket_path)
    finally:
        await mcp.aclose()


async def run_doctor(cfg: config.AgentConfig, pull: bool) -> int:
    mcp = McpClient(cfg.mcp_socket_path)
    async with httpx.AsyncClient(base_url=cfg.local.url, timeout=5.0) as ollama:
        try:
            checks = await doctor.run_checks(cfg, mcp, ollama)
            local = next((c for c in checks if c.name == "Local model"), None)
            if pull and local is not None and local.status == "fail":
                print(f"Downloading {cfg.local.model} (this can take a while)…")
                try:
                    await doctor.pull_model(
                        ollama, cfg.local.model, lambda line: print(f"  {line}", flush=True)
                    )
                except RuntimeError as err:
                    print(f"error: {err}", file=sys.stderr)
                    return 1
                checks = await doctor.run_checks(cfg, mcp, ollama)
        finally:
            await mcp.aclose()
    print(doctor.render(checks))
    ready = doctor.verdict(checks)
    print()
    print("The assistant is ready." if ready else "The assistant is not ready yet.")
    return 0 if ready else 1


async def ask(cfg: config.AgentConfig, question: str, model: str, yes: bool) -> int:
    """One request in the terminal, with approvals asked on stdin."""
    mcp = McpClient(cfg.mcp_socket_path)
    agent, _ = build_agent(cfg, mcp)
    try:
        convo: Conversation = await agent.new_conversation()
    except McpError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    async def emit(event: dict[str, object]) -> None:
        kind = event["type"]
        if kind == "text":
            print(event["delta"], end="", flush=True)
        elif kind == "tool_call":
            print(f"\n  → {event['name']} {json.dumps(event['arguments'])}", flush=True)
        elif kind == "tool_result":
            status = "ok" if event["ok"] else "failed"
            print(f"  ← {status}", flush=True)
        elif kind in ("notice", "error"):
            print(f"\n[{kind}] {event['message']}", file=sys.stderr, flush=True)

    async def approve(call: ToolCall, spec: ToolSpec) -> bool:
        if yes:
            return True
        prompt = f"  Allow {spec.title or call.name} {json.dumps(call.arguments)}? [y/N] "
        answer = await asyncio.to_thread(input, prompt)
        return answer.strip().lower() in ("y", "yes")

    try:
        result = await agent.run_turn(
            convo,
            question,
            emit=emit,
            approve=approve,
            model=model,  # type: ignore[arg-type]
        )
    finally:
        await mcp.aclose()
    print()
    if result.provider:
        print(f"[{result.provider}:{result.model}, {result.steps} step(s)]", file=sys.stderr)
    return 0 if result.stopped in ("answered", "truncated") else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="osaima-agent", description=__doc__)
    parser.add_argument("--version", action="version", version=f"osaima-agent {__version__}")
    parser.add_argument(
        "--config",
        type=Path,
        help="settings file (default: %(default)s)",
        default=config.config_path(),
    )
    parser.add_argument("-v", "--verbose", action="store_true")
    sub = parser.add_subparsers(dest="command")
    serve_cmd = sub.add_parser("serve", help="run the agent service (default)")
    serve_cmd.add_argument("--socket", type=Path, help="socket path")
    doctor_cmd = sub.add_parser("doctor", help="check the setup and fix what's missing")
    doctor_cmd.add_argument("--pull", action="store_true", help="download the local model")
    ask_cmd = sub.add_parser("ask", help="ask one question in the terminal")
    ask_cmd.add_argument("question", nargs="+")
    ask_cmd.add_argument("--model", choices=["auto", "local", "cloud"], default="auto")
    ask_cmd.add_argument("-y", "--yes", action="store_true", help="approve every action")
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else os.environ.get("OSAIMA_LOG", "INFO").upper(),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    try:
        cfg = config.load(args.config)
    except config.ConfigError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    try:
        if args.command == "doctor":
            return asyncio.run(run_doctor(cfg, args.pull))
        if args.command == "ask":
            return asyncio.run(ask(cfg, " ".join(args.question), args.model, args.yes))
        socket_path = getattr(args, "socket", None) or cfg.socket_path
        asyncio.run(serve(cfg, socket_path))
    except KeyboardInterrupt:
        pass
    except RuntimeError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
