"""Push-to-talk voice: one microphone and one speaker, shared by every connection.

States: `idle` -> `listening` -> `transcribing` -> `idle`, and `idle` -> `speaking`
-> `idle`. Progress is reported to the caller that started the work as
`voice_state`, `voice_transcript` and `voice_error` events. Recorded audio lives
in a private temporary folder and is deleted as soon as it has been transcribed;
nothing is sent over the network.
"""

from __future__ import annotations

import asyncio
import contextlib
import re
import shutil
import tempfile
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

from ..config import VoiceConfig
from .audio import (
    PLAYERS,
    RECORDERS,
    AudioError,
    Playback,
    Recording,
    Which,
    recorded_seconds,
    resolve_command,
)
from .engines import CommandSynthesizer, Synthesizer, Transcriber, WhisperTranscriber

Emit = Callable[[dict[str, Any]], Awaitable[None]]

_CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")
_SPACE = re.compile(r"\s+")


class VoiceError(Exception):
    def __init__(self, message: str, *, busy: bool = False) -> None:
        super().__init__(message)
        self.busy = busy


def speakable(text: str, limit: int) -> str:
    """Plain text for the speaker: control characters and extra spacing removed, length capped."""
    text = _SPACE.sub(" ", _CONTROL.sub(" ", text)).strip()
    if len(text) <= limit:
        return text
    cut = text[:limit]
    end = max(cut.rfind(". "), cut.rfind("! "), cut.rfind("? "))
    return (cut[: end + 1] if end > limit // 2 else cut.rsplit(" ", 1)[0]).rstrip()


class VoiceService:
    def __init__(
        self,
        cfg: VoiceConfig,
        models_dir: Path,
        work_dir: Path,
        *,
        transcriber: Transcriber | None = None,
        synthesizer: Synthesizer | None = None,
        which: Which = shutil.which,
    ) -> None:
        self._cfg = cfg
        self._work_root = work_dir
        self._record_command = resolve_command(cfg.record_command, RECORDERS, which)
        self._play_command = resolve_command(cfg.play_command, PLAYERS, which)
        self._stt: Transcriber = transcriber or WhisperTranscriber(cfg, models_dir)
        self._tts: Synthesizer = synthesizer or CommandSynthesizer(
            cfg.tts_command, cfg.tts_voice, models_dir, which, cfg.command_timeout_s
        )
        self.state = "idle"
        self._recording: Recording | None = None
        self._work: Path | None = None
        self._limit: asyncio.Task[None] | None = None
        self._job: asyncio.Task[None] | None = None
        self._playback: Playback | None = None

    # ── what works ───────────────────────────────────────────────────────────

    def listen_problem(self) -> str:
        if not self._cfg.enabled:
            return "voice is turned off in the settings"
        if self._record_command is None:
            return "no microphone tool found (install pipewire or alsa-utils)"
        return self._stt.problem()

    def speak_problem(self) -> str:
        if not self._cfg.enabled:
            return "voice is turned off in the settings"
        if self._play_command is None:
            return "no audio player found (install pipewire or alsa-utils)"
        return self._tts.problem()

    def status(self) -> dict[str, Any]:
        listen, speak = self.listen_problem(), self.speak_problem()
        return {
            "enabled": self._cfg.enabled,
            "state": self.state,
            "can_listen": not listen,
            "can_speak": not speak,
            "problems": [p for p in (listen, speak) if p],
            "speak_replies": self._cfg.speak_replies,
            "max_record_s": self._cfg.max_record_s,
            "stt_model": self._cfg.stt_model,
            "tts_voice": self._cfg.tts_voice,
        }

    # ── listening ────────────────────────────────────────────────────────────

    async def start_listening(self, emit: Emit) -> None:
        if problem := self.listen_problem():
            raise VoiceError(problem)
        if self.state == "speaking":
            await self.stop_speaking()
        if self.state != "idle":
            raise VoiceError(f"voice is busy ({self.state})", busy=True)
        assert self._record_command is not None
        self.state = "listening"
        try:
            work = await asyncio.to_thread(self._new_work_dir)
            self._work = work
            self._recording = await Recording.start(self._record_command, work / "speech.wav")
        except (AudioError, OSError) as err:
            await self._cleanup()
            self.state = "idle"
            raise VoiceError(f"could not start recording: {err}") from err
        self._limit = asyncio.create_task(self._stop_at_limit(emit))
        await _send(emit, {"type": "voice_state", "state": "listening"})

    async def stop_listening(self, emit: Emit) -> None:
        recording = self._recording
        if recording is None:
            raise VoiceError("not listening")
        self._recording = None
        self.state = "transcribing"
        if self._limit is not None and self._limit is not asyncio.current_task():
            self._limit.cancel()
        self._limit = None
        path = await recording.stop()
        self._job = asyncio.create_task(self._transcribe(path, emit))
        await _send(emit, {"type": "voice_state", "state": "transcribing"})

    async def cancel_listening(self) -> None:
        recording, self._recording = self._recording, None
        if self._limit is not None and self._limit is not asyncio.current_task():
            self._limit.cancel()
        self._limit = None
        if recording is not None:
            await recording.abort()
            await self._cleanup()
            self.state = "idle"
        elif self.state == "transcribing" and self._job is not None:
            self._job.cancel()

    async def _stop_at_limit(self, emit: Emit) -> None:
        await asyncio.sleep(self._cfg.max_record_s)
        with contextlib.suppress(VoiceError):
            await self.stop_listening(emit)

    async def _transcribe(self, path: Path, emit: Emit) -> None:
        try:
            if recorded_seconds(path) < self._cfg.min_record_s:
                await _send(emit, _error("I didn't hear anything. Hold on a little longer."))
                return
            text = await asyncio.to_thread(self._stt.transcribe, path)
            if text:
                await _send(emit, {"type": "voice_transcript", "text": text})
            else:
                await _send(emit, _error("I didn't catch any words. Try again."))
        except asyncio.CancelledError:
            raise
        except Exception as err:  # a failed recognizer must not take the service down
            await _send(emit, _error(f"Speech recognition failed: {err}"))
        finally:
            await self._cleanup()
            self.state = "idle"
            await _send(emit, {"type": "voice_state", "state": "idle"})

    # ── speaking ─────────────────────────────────────────────────────────────

    async def speak(self, text: str, emit: Emit) -> None:
        if problem := self.speak_problem():
            raise VoiceError(problem)
        text = speakable(text, self._cfg.max_speak_chars)
        if not text:
            raise VoiceError("there is nothing to say")
        if self.state == "speaking":
            await self.stop_speaking()
        if self.state != "idle":
            raise VoiceError(f"voice is busy ({self.state})", busy=True)
        self.state = "speaking"
        self._job = asyncio.create_task(self._speak(text, emit))
        await _send(emit, {"type": "voice_state", "state": "speaking"})

    async def stop_speaking(self) -> None:
        job = self._job
        if self.state != "speaking" or job is None:
            return
        job.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await job

    async def _speak(self, text: str, emit: Emit) -> None:
        try:
            assert self._play_command is not None
            work = await asyncio.to_thread(self._new_work_dir)
            self._work = work
            out = work / "reply.wav"
            await self._tts.synthesize(text, out)
            self._playback = await Playback.start(self._play_command, out)
            await self._playback.wait()
        except asyncio.CancelledError:
            raise
        except (AudioError, OSError) as err:
            await _send(emit, _error(f"Could not speak: {err}"))
        finally:
            if self._playback is not None:
                await self._playback.stop()
                self._playback = None
            await self._cleanup()
            self.state = "idle"
            await _send(emit, {"type": "voice_state", "state": "idle"})

    # ── housekeeping ─────────────────────────────────────────────────────────

    def _new_work_dir(self) -> Path:
        self._work_root.mkdir(parents=True, exist_ok=True, mode=0o700)
        return Path(tempfile.mkdtemp(prefix="voice-", dir=self._work_root))

    async def _cleanup(self) -> None:
        work, self._work = self._work, None
        if work is not None:
            await asyncio.to_thread(shutil.rmtree, work, True)

    async def aclose(self) -> None:
        await self.cancel_listening()
        await self.stop_speaking()
        if self._job is not None:
            self._job.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._job


def _error(message: str) -> dict[str, Any]:
    return {"type": "voice_error", "message": message}


async def _send(emit: Emit, event: dict[str, Any]) -> None:
    with contextlib.suppress(ConnectionError, OSError):
        await emit(event)
