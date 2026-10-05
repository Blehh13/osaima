"""Local models through Ollama's chat API (`POST /api/chat`), streamed."""

from __future__ import annotations

import json
from typing import Any

import httpx

from . import (
    Completion,
    Message,
    ProviderError,
    ProviderUnavailable,
    TextSink,
    ToolCall,
    ToolSpec,
    new_call_id,
)

# Small local models have short context windows; keep the recent conversation.
MAX_HISTORY_MESSAGES = 24


class OllamaProvider:
    name = "ollama"

    def __init__(
        self,
        url: str,
        model: str,
        *,
        context_tokens: int = 8192,
        timeout_s: float = 120.0,
        transport: httpx.AsyncBaseTransport | None = None,
    ) -> None:
        self.model = model
        self._context_tokens = context_tokens
        self._client = httpx.AsyncClient(
            base_url=url,
            timeout=httpx.Timeout(timeout_s, connect=3.0),
            transport=transport,
        )

    async def available(self) -> bool:
        try:
            response = await self._client.get("/api/tags", timeout=3.0)
            response.raise_for_status()
        except httpx.HTTPError:
            return False
        names = {m.get("name", "") for m in response.json().get("models", [])}
        return self.model in names or f"{self.model}:latest" in names

    async def complete(
        self,
        system: str,
        messages: list[Message],
        tools: list[ToolSpec],
        on_text: TextSink,
    ) -> Completion:
        body = {
            "model": self.model,
            "messages": [{"role": "system", "content": system}, *to_ollama(messages)],
            "tools": [ollama_tool(t) for t in tools],
            "stream": True,
            "keep_alive": "10m",
            "options": {"temperature": 0.2, "num_ctx": self._context_tokens},
        }
        text_parts: list[str] = []
        calls: list[ToolCall] = []
        done_reason = "stop"
        try:
            async with self._client.stream("POST", "/api/chat", json=body) as response:
                if response.status_code == 404:
                    raise ProviderUnavailable(
                        f"Ollama has no model {self.model!r}; run `ollama pull {self.model}`"
                    )
                if response.status_code >= 400:
                    detail = (await response.aread()).decode(errors="replace")
                    raise ProviderError(f"Ollama returned {response.status_code}: {detail}")
                async for line in response.aiter_lines():
                    if not line.strip():
                        continue
                    chunk = json.loads(line)
                    if error := chunk.get("error"):
                        raise ProviderError(f"Ollama error: {error}")
                    message = chunk.get("message") or {}
                    if delta := message.get("content"):
                        text_parts.append(delta)
                        await on_text(delta)
                    for raw in message.get("tool_calls") or []:
                        calls.append(parse_tool_call(raw))
                    if chunk.get("done"):
                        done_reason = chunk.get("done_reason") or "stop"
        except httpx.ConnectError as err:
            raise ProviderUnavailable(f"Ollama is not running at {self._client.base_url}") from err
        except httpx.TimeoutException as err:
            raise ProviderError("Ollama timed out") from err
        except (httpx.HTTPError, json.JSONDecodeError) as err:
            raise ProviderError(f"Ollama request failed: {err}") from err

        stop = "tool_calls" if calls else ("max_tokens" if done_reason == "length" else "end")
        return Completion(
            text="".join(text_parts),
            tool_calls=calls,
            stop=stop,
            provider=self.name,
            model=self.model,
        )

    async def aclose(self) -> None:
        await self._client.aclose()


def parse_tool_call(raw: dict[str, Any]) -> ToolCall:
    function = raw.get("function") or {}
    arguments = function.get("arguments") or {}
    if isinstance(arguments, str):
        # Some models emit arguments as a JSON string.
        try:
            arguments = json.loads(arguments) if arguments.strip() else {}
        except json.JSONDecodeError:
            arguments = {"__invalid_json__": arguments}
    if not isinstance(arguments, dict):
        arguments = {"__invalid_json__": json.dumps(arguments)}
    return ToolCall(id=new_call_id(), name=str(function.get("name", "")), arguments=arguments)


def ollama_tool(tool: ToolSpec) -> dict[str, Any]:
    return {
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        },
    }


def to_ollama(messages: list[Message]) -> list[dict[str, Any]]:
    """Convert the recent part of a conversation, starting at a user turn."""
    recent = messages[-MAX_HISTORY_MESSAGES:]
    while recent and recent[0].role != "user":
        recent = recent[1:]
    out: list[dict[str, Any]] = []
    for m in recent:
        if m.role == "user":
            out.append({"role": "user", "content": m.content})
        elif m.role == "assistant":
            entry: dict[str, Any] = {"role": "assistant", "content": m.content}
            if m.tool_calls:
                entry["tool_calls"] = [
                    {"function": {"name": c.name, "arguments": c.arguments}} for c in m.tool_calls
                ]
            out.append(entry)
        else:
            out.append({"role": "tool", "content": m.content, "tool_name": m.tool_name or ""})
    return out
