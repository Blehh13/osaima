"""Provider-neutral conversation types and the interface every LLM backend implements.

Histories are append-only: a message is never edited after it is added. Cloud
models bind their reasoning to the exact conversation prefix, so editing an
earlier turn would invalidate it (and the prompt cache).
"""

from __future__ import annotations

import uuid
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Literal, Protocol

Role = Literal["user", "assistant", "tool"]
StopReason = Literal["end", "tool_calls", "max_tokens", "refusal"]
TextSink = Callable[[str], Awaitable[None]]


def new_call_id() -> str:
    """An id valid for every provider (`[A-Za-z0-9_-]+`)."""
    return f"call_{uuid.uuid4().hex[:24]}"


@dataclass(frozen=True)
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]


@dataclass(frozen=True)
class ToolSpec:
    """A tool as offered to the model. `annotations` follow MCP's tool hints."""

    name: str
    description: str
    input_schema: dict[str, Any]
    annotations: dict[str, Any] = field(default_factory=dict)
    title: str = ""

    @property
    def read_only(self) -> bool:
        return bool(self.annotations.get("readOnlyHint", False))

    @property
    def destructive(self) -> bool:
        # MCP's default for a non-read-only tool without the hint is "destructive".
        if self.read_only:
            return False
        return bool(self.annotations.get("destructiveHint", True))


@dataclass
class Message:
    role: Role
    content: str = ""
    tool_calls: list[ToolCall] = field(default_factory=list)
    # For role == "tool":
    tool_call_id: str | None = None
    tool_name: str | None = None
    is_error: bool = False
    # Provider-native assistant content (e.g. Claude's thinking blocks) that
    # must be replayed verbatim to the same provider.
    native: NativeContent | None = None


@dataclass(frozen=True)
class NativeContent:
    provider: str
    content: Any


@dataclass
class Completion:
    text: str
    tool_calls: list[ToolCall]
    stop: StopReason
    provider: str
    model: str
    native: NativeContent | None = None
    detail: str = ""


class ProviderUnavailable(RuntimeError):
    """The backend can't be reached or isn't configured (try another one)."""


class ProviderError(RuntimeError):
    """The backend was reached but the request failed."""


class Provider(Protocol):
    name: str
    model: str

    async def available(self) -> bool:
        """Cheap readiness check."""
        ...

    async def complete(
        self,
        system: str,
        messages: list[Message],
        tools: list[ToolSpec],
        on_text: TextSink,
    ) -> Completion:
        """Run one model turn, streaming visible text to `on_text`."""
        ...
