"""Microphone capture and speaker playback through the system's audio tools.

Capture and playback are done by external programs (PipeWire, ALSA or
PulseAudio command line tools) instead of a Python audio library, so the agent
needs no extra packages and no access to the sound hardware itself. The command
for each is configurable; with none configured, the first tool found is used.
"""

from __future__ import annotations

import asyncio
import contextlib
import signal
from collections.abc import Callable, Sequence
from pathlib import Path

# (program, command). Recording is 16 kHz mono, which is what speech models expect.
RECORDERS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("pw-record", ("pw-record", "--rate", "16000", "--channels", "1", "{output}")),
    (
        "arecord",
        ("arecord", "-q", "-t", "wav", "-f", "S16_LE", "-r", "16000", "-c", "1", "{output}"),
    ),
)
PLAYERS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("pw-play", ("pw-play", "{input}")),
    ("paplay", ("paplay", "{input}")),
    ("aplay", ("aplay", "-q", "{input}")),
)

WAV_HEADER_BYTES = 44
BYTES_PER_SECOND = 16000 * 2  # 16 kHz, 16-bit, mono

Which = Callable[[str], str | None]


class AudioError(Exception):
    """A capture or playback tool is missing or failed."""


def resolve_command(
    configured: Sequence[str],
    candidates: Sequence[tuple[str, tuple[str, ...]]],
    which: Which,
) -> tuple[str, ...] | None:
    """The command to use: the configured one, else the first installed candidate."""
    if configured:
        return tuple(configured) if which(configured[0]) else None
    for program, command in candidates:
        if which(program):
            return command
    return None


def fill(command: Sequence[str], **values: str) -> list[str]:
    """Substitute `{name}` placeholders (only the given names; other braces are kept)."""
    out = []
    for part in command:
        for key, value in values.items():
            part = part.replace("{" + key + "}", value)
        out.append(part)
    return out


def recorded_seconds(path: Path) -> float:
    """Length of a 16 kHz mono 16-bit WAV file, judged by its size."""
    try:
        size = path.stat().st_size
    except OSError:
        return 0.0
    return max(0, size - WAV_HEADER_BYTES) / BYTES_PER_SECOND


async def _stderr_text(proc: asyncio.subprocess.Process) -> str:
    if proc.stderr is None:
        return ""
    try:
        data = await asyncio.wait_for(proc.stderr.read(2000), 1)
    except TimeoutError:
        return ""
    return data.decode(errors="replace").strip()


class Recording:
    """A running capture. `stop()` ends it cleanly so the WAV header is finished."""

    def __init__(self, proc: asyncio.subprocess.Process, path: Path, stop_grace_s: float) -> None:
        self._proc = proc
        self.path = path
        self._grace = stop_grace_s

    @classmethod
    async def start(
        cls, command: Sequence[str], path: Path, startup_s: float = 0.3, stop_grace_s: float = 3.0
    ) -> Recording:
        try:
            proc = await asyncio.create_subprocess_exec(
                *fill(command, output=str(path)),
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.PIPE,
            )
        except OSError as err:
            raise AudioError(f"could not start {command[0]}: {err}") from err
        try:
            await asyncio.wait_for(proc.wait(), startup_s)
        except TimeoutError:
            return cls(proc, path, stop_grace_s)  # still running: recording
        detail = await _stderr_text(proc)
        raise AudioError(f"{command[0]} stopped right away" + (f": {detail}" if detail else ""))

    async def stop(self) -> Path:
        if self._proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                self._proc.send_signal(signal.SIGINT)  # finishes the WAV file
            try:
                await asyncio.wait_for(self._proc.wait(), self._grace)
            except TimeoutError:
                with contextlib.suppress(ProcessLookupError):
                    self._proc.kill()
                await self._proc.wait()
        return self.path

    async def abort(self) -> None:
        await self.stop()
        with contextlib.suppress(OSError):
            self.path.unlink()


class Playback:
    """A running playback that can be stopped."""

    def __init__(self, proc: asyncio.subprocess.Process, command: str) -> None:
        self._proc = proc
        self._command = command

    @classmethod
    async def start(cls, command: Sequence[str], path: Path) -> Playback:
        try:
            proc = await asyncio.create_subprocess_exec(
                *fill(command, input=str(path)),
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.PIPE,
            )
        except OSError as err:
            raise AudioError(f"could not start {command[0]}: {err}") from err
        return cls(proc, command[0])

    async def wait(self) -> None:
        code = await self._proc.wait()
        if code not in (0, -signal.SIGTERM):
            detail = await _stderr_text(self._proc)
            raise AudioError(f"{self._command} failed" + (f": {detail}" if detail else ""))

    async def stop(self) -> None:
        if self._proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                self._proc.terminate()
            try:
                await asyncio.wait_for(self._proc.wait(), 2)
            except TimeoutError:
                with contextlib.suppress(ProcessLookupError):
                    self._proc.kill()
                await self._proc.wait()
