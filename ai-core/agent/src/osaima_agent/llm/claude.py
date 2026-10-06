"""Claude through the Anthropic Messages API, streamed.

Used only when the user has enabled the cloud fallback. Requests:

- keep the conversation append-only and replay Claude's own assistant content
  (including thinking blocks) verbatim, so reasoning stays valid across turns;
- opt into server-side refusal fallbacks (`fallbacks: "default"`), so a
  safety-classifier decline is retried on Anthropic's recommended model;
- ask the API to drop, rather than reject, any thinking block whose
  conversation prefix no longer matches (defence in depth for the above).
"""

from __future__ import annotations

import json
from typing import Any

import anthropic

from ..config import CloudConfig
from . import (
    Completion,
    Message,
    NativeContent,
    ProviderError,
    ProviderUnavailable,
    TextSink,
    ToolCall,
    ToolSpec,
)

# Anthropic API feature identifiers (protocol constants, not settings).
FALLBACK_BETA = "server-side-fallback-2026-07-01"
BINDING_BETA = "thinking-binding-controls-2026-08-01"


class ClaudeProvider:
    name = "claude"

    def __init__(
        self,
        model: str = "claude-opus-5-5",
        *,
        effort: str = CloudConfig.effort,
        max_tokens: int = CloudConfig.max_tokens,
        server_fallback: bool = CloudConfig.server_fallback,
        timeout_s: float = CloudConfig.timeout_s,
        max_retries: int = CloudConfig.max_retries,
        json_retries: int = CloudConfig.json_retries,
        client: anthropic.AsyncAnthropic | None = None,
    ) -> None:
        self.model = model
        self._effort = effort
        self._max_tokens = max_tokens
        self._server_fallback = server_fallback
        self._timeout_s = timeout_s
        self._max_retries = max_retries
        self._json_retries = json_retries
        self._client = client

    @classmethod
    def from_config(cls, cfg: CloudConfig) -> ClaudeProvider:
        return cls(
            cfg.model,
            effort=cfg.effort,
            max_tokens=cfg.max_tokens,
            server_fallback=cfg.server_fallback,
            timeout_s=cfg.timeout_s,
            max_retries=cfg.max_retries,
            json_retries=cfg.json_retries,
        )

    def _get_client(self) -> anthropic.AsyncAnthropic:
        if self._client is None:
            # Credentials come from ANTHROPIC_API_KEY or an `ant auth login` profile.
            self._client = anthropic.AsyncAnthropic(
                max_retries=self._max_retries, timeout=self._timeout_s
            )
        return self._client

    async def available(self) -> bool:
        try:
            self._get_client()
        except anthropic.AnthropicError:
            return False
        return True

    async def complete(
        self,
        system: str,
        messages: list[Message],
        tools: list[ToolSpec],
        on_text: TextSink,
    ) -> Completion:
        try:
            client = self._get_client()
        except anthropic.AnthropicError as err:
            raise ProviderUnavailable(f"Claude is not configured: {err}") from err

        request: dict[str, Any] = {
            "model": self.model,
            "max_tokens": self._max_tokens,
            "system": system,
            "messages": to_claude(messages),
            "tools": [claude_tool(t) for t in tools],
            "output_config": {"effort": self._effort},
            "thinking": {
                "type": "adaptive",
                "block_binding": {"prefix_mismatch_behavior": "drop_block"},
            },
            "cache_control": {"type": "ephemeral"},
            "betas": [BINDING_BETA],
        }
        if self._server_fallback:
            request["fallbacks"] = "default"
            request["betas"].append(FALLBACK_BETA)
        for _ in range(self._json_retries + 1):
            try:
                final = await self._stream(client, request, on_text)
                return completion_from(final, self.name)
            except ValueError:
                # Tool input JSON the SDK couldn't parse at all: there is no
                # tool_use id to answer, so re-issue the turn (bounded).
                continue
            except anthropic.AuthenticationError as err:
                raise ProviderUnavailable("Claude API key is missing or invalid") from err
            except anthropic.PermissionDeniedError as err:
                raise ProviderUnavailable(f"Claude access denied: {err.message}") from err
            except anthropic.RateLimitError as err:
                raise ProviderError("Claude rate limit reached; try again shortly") from err
            except anthropic.APIStatusError as err:
                raise ProviderError(f"Claude API error {err.status_code}: {err.message}") from err
            except anthropic.APIConnectionError as err:
                raise ProviderUnavailable("can't reach the Claude API (offline?)") from err
        raise ProviderError("Claude produced unreadable tool input")

    async def _stream(
        self, client: anthropic.AsyncAnthropic, request: dict[str, Any], on_text: TextSink
    ) -> Any:
        async with client.beta.messages.stream(**request) as stream:
            async for event in stream:
                if event.type == "text":
                    await on_text(event.text)
            return await stream.get_final_message()


def completion_from(final: Any, provider: str) -> Completion:
    """Map a final Claude message to a provider-neutral completion."""
    text = "".join(b.text for b in final.content if b.type == "text")
    calls = [
        ToolCall(id=b.id, name=b.name, arguments=_validated_input(b.input))
        for b in final.content
        if b.type == "tool_use"
    ]
    native = NativeContent(provider=provider, content=list(final.content))
    if final.stop_reason == "refusal":
        details = getattr(final, "stop_details", None)
        category = getattr(details, "category", None) if details else None
        return Completion(
            text=text,
            tool_calls=[],
            stop="refusal",
            provider=provider,
            model=final.model,
            native=native,
            detail=f"declined ({category})" if category else "declined",
        )
    if final.stop_reason == "max_tokens":
        # A truncated tool input parses as a valid-looking partial object; never run it.
        return Completion(text, [], "max_tokens", provider, final.model, native)
    return Completion(
        text=text,
        tool_calls=calls,
        stop="tool_calls" if calls else "end",
        provider=provider,
        model=final.model,
        native=native,
    )


def _validated_input(raw: Any) -> dict[str, Any]:
    # With eager input streaming the API doesn't validate inputs; the AI Core
    # validates arguments against each tool's schema before acting.
    if isinstance(raw, dict):
        return raw
    return {"__invalid_json__": json.dumps(raw, default=str)}


def claude_tool(tool: ToolSpec) -> dict[str, Any]:
    return {
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
        "eager_input_streaming": True,
    }


def to_claude(messages: list[Message]) -> list[dict[str, Any]]:
    """Convert the whole history deterministically, so each request's prefix
    is byte-identical to the previous one (prompt cache + thinking binding)."""
    out: list[dict[str, Any]] = []
    pending_results: list[dict[str, Any]] = []

    def flush() -> None:
        if pending_results:
            out.append({"role": "user", "content": list(pending_results)})
            pending_results.clear()

    for m in messages:
        if m.role == "tool":
            pending_results.append(
                {
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id,
                    "content": m.content,
                    "is_error": m.is_error,
                }
            )
            continue
        flush()
        if m.role == "user":
            out.append({"role": "user", "content": m.content})
        elif m.native is not None and m.native.provider == ClaudeProvider.name:
            out.append({"role": "assistant", "content": m.native.content})
        else:
            blocks: list[dict[str, Any]] = []
            if m.content:
                blocks.append({"type": "text", "text": m.content})
            blocks.extend(
                {"type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments}
                for c in m.tool_calls
            )
            # An empty local reply has nothing to replay; consecutive user turns are allowed.
            if blocks:
                out.append({"role": "assistant", "content": blocks})
    flush()
    return out
