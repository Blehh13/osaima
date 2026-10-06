"""Append-only JSON Lines record of every action the agent attempts."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Any


class AuditLog:
    def __init__(self, path: Path, max_bytes: int = 5 * 1024 * 1024) -> None:
        self._path = path
        self._max_bytes = max_bytes

    def record(self, **entry: Any) -> None:
        """Write one entry. Logging must never break the agent, so I/O errors are ignored."""
        entry = {"ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"), **entry}
        try:
            self._path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            if self._path.exists() and self._path.stat().st_size > self._max_bytes:
                os.replace(self._path, self._path.with_suffix(".jsonl.1"))
            fd = os.open(self._path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
            with os.fdopen(fd, "a", encoding="utf-8") as f:
                f.write(json.dumps(entry, default=str, ensure_ascii=False) + "\n")
        except OSError:
            pass

    def tail(self, limit: int = 50) -> list[dict[str, Any]]:
        try:
            lines = self._path.read_text(encoding="utf-8").splitlines()
        except OSError:
            return []
        out = []
        for line in lines[-limit:]:
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        return out
