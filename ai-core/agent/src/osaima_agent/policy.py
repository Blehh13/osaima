"""Which tool calls run automatically, which need the user's approval, and
which are never allowed. Decisions use MCP tool annotations, so tools added to
the AI Core later are handled without changes here."""

from __future__ import annotations

import enum
from collections.abc import Iterable

from .config import ConfirmMode
from .llm import ToolSpec


class Decision(enum.Enum):
    ALLOW = "allow"
    CONFIRM = "confirm"
    DENY = "deny"


class Policy:
    def __init__(self, confirm: ConfirmMode = "destructive", deny: Iterable[str] = ()) -> None:
        self._confirm = confirm
        self._deny = frozenset(deny)

    def decide(self, tool: ToolSpec) -> Decision:
        if tool.name in self._deny:
            return Decision.DENY
        if tool.read_only:
            return Decision.ALLOW
        if tool.destructive or self._confirm == "all_changes":
            return Decision.CONFIRM
        return Decision.ALLOW
