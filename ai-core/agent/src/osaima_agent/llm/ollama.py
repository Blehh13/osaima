"""Local models through Ollama's chat API (`POST /api/chat`), streamed."""

from __future__ import annotations

import json
from typing import Any

import httpx

from ..config import LocalConfig
from . import (
    Completion,
    Message,
    ProviderError,
    ProviderUnavailable,
    StopReason,
    TextSink,
    ToolCall,
    ToolSpec,
    new_call_id,
)


class OllamaProvider:
    name = "ollama"

    def __init__(
        self,
        url: str,
        model: str,
        *,
        context_tokens: int = LocalConfig.context_tokens,
        timeout_s: float = LocalConfig.timeout_s,
        connect_timeout_s: float = LocalConfig.connect_timeout_s,
        temperature: float = LocalConfig.temperature,
        keep_alive: str = LocalConfig.keep_alive,
        history_messages: int = LocalConfig.history_messages,
        transport: httpx.AsyncBaseTransport | None = None,
    ) -> None:
        self.model = model
        self._context_tokens = context_tokens
        self._temperature = temperature
        self._keep_alive = keep_alive
        self._history_messages = history_messages
        self._connect_timeout = connect_timeout_s
        self._client = httpx.AsyncClient(
            base_url=url,
            timeout=httpx.Timeout(timeout_s, connect=connect_timeout_s),
            transport=transport,
        )

    @classmethod
    def from_config(cls, cfg: LocalConfig) -> OllamaProvider:
        return cls(
            cfg.url,
            cfg.model,
            context_tokens=cfg.context_tokens,
            timeout_s=cfg.timeout_s,
            connect_timeout_s=cfg.connect_timeout_s,
            temperature=cfg.temperature,
            keep_alive=cfg.keep_alive,
            history_messages=cfg.history_messages,
        )

    async def available(self) -> bool:
        try:
            response = await self._client.get("/api/tags", timeout=self._connect_timeout)
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
            "messages": [
                {"role": "system", "content": system},
                *to_ollama(messages, self._history_messages),
            ],
            "tools": [ollama_tool(t) for t in tools],
            "stream": True,
            "keep_alive": self._keep_alive,
            "options": {"temperature": self._temperature, "num_ctx": self._context_tokens},
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

        stop: StopReason
        if calls:
            stop = "tool_calls"
        elif done_reason == "length":
            stop = "max_tokens"
        else:
            stop = "end"
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


def to_ollama(
    messages: list[Message], history_messages: int = LocalConfig.history_messages
) -> list[dict[str, Any]]:
    """Convert the recent part of a conversation, starting at a user turn.

    Small local models have short context windows, so only the last
    `history_messages` messages are sent.
    """
    recent = messages[-history_messages:]
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
