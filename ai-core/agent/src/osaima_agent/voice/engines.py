"""Speech engines: faster-whisper turns speech into text, Piper turns text into speech.

Both run on this computer and need no account. `faster-whisper` is imported only
when first used, so the assistant starts (and its tests run) without it.
"""

from __future__ import annotations

import asyncio
import contextlib
import importlib.util
import threading
from collections.abc import Sequence
from pathlib import Path
from typing import Any, Protocol

from ..config import VoiceConfig
from .audio import AudioError, Which, fill


class Transcriber(Protocol):
    def problem(self) -> str:
        """Why it can't work, or an empty string."""
        ...

    def transcribe(self, wav: Path) -> str:
        """Blocking: the words in a 16 kHz mono WAV file."""
        ...


class Synthesizer(Protocol):
    def problem(self) -> str: ...

    async def synthesize(self, text: str, out: Path) -> None:
        """Write `text` spoken aloud to `out` (a WAV file)."""
        ...


class WhisperTranscriber:
    def __init__(self, cfg: VoiceConfig, models_dir: Path) -> None:
        self._cfg = cfg
        self._models_dir = models_dir
        self._model: Any = None
        self._lock = threading.Lock()

    def problem(self) -> str:
        if importlib.util.find_spec("faster_whisper") is None:
            return "speech recognition is not installed (pip install faster-whisper)"
        return ""

    def _load(self) -> Any:
        with self._lock:
            if self._model is None:
                from faster_whisper import WhisperModel  # heavy import, on first use

                self._model = WhisperModel(
                    self._cfg.stt_model,
                    device=self._cfg.stt_device,
                    compute_type=self._cfg.stt_compute_type,
                    download_root=str(self._models_dir),
                )
            return self._model

    def transcribe(self, wav: Path) -> str:
        model = self._load()
        segments, _info = model.transcribe(
            str(wav),
            language=self._cfg.stt_language or None,
            beam_size=self._cfg.stt_beam_size,
            vad_filter=True,
        )
        return " ".join(segment.text.strip() for segment in segments).strip()


class CommandSynthesizer:
    """Runs a text-to-speech program (Piper by default): text on stdin, a WAV file out."""

    def __init__(
        self,
        command: Sequence[str],
        voice: str,
        models_dir: Path,
        which: Which,
        timeout_s: float,
    ) -> None:
        self._command = tuple(command)
        self._voice = voice
        self._models_dir = models_dir
        self._which = which
        self._timeout = timeout_s

    def problem(self) -> str:
        program = self._command[0]
        if self._which(program) is None:
            return f"{program} is not installed, so replies can't be spoken"
        uses_voice = any("{voice}" in part for part in self._command)
        if uses_voice and not self._voice_present():
            return (
                f"the voice {self._voice} is not downloaded: run "
                f"`python -m piper.download_voices {self._voice} --data-dir {self._models_dir}`"
            )
        return ""

    def _voice_present(self) -> bool:
        return (
            Path(self._voice).is_file()
            or (self._models_dir / f"{self._voice}.onnx").is_file()
        )

    async def synthesize(self, text: str, out: Path) -> None:
        await asyncio.to_thread(self._models_dir.mkdir, parents=True, exist_ok=True)
        argv = fill(
            self._command,
            voice=self._voice,
            models_dir=str(self._models_dir),
            output=str(out),
        )
        try:
            proc = await asyncio.create_subprocess_exec(
                *argv,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.PIPE,
            )
        except OSError as err:
            raise AudioError(f"could not start {argv[0]}: {err}") from err
        try:
            _out, err_bytes = await asyncio.wait_for(
                proc.communicate(text.encode()), self._timeout
            )
        except TimeoutError:
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
            await proc.wait()
            raise AudioError(f"{argv[0]} took longer than {self._timeout:g} s") from None
        if proc.returncode != 0:
            detail = err_bytes.decode(errors="replace").strip().splitlines()
            raise AudioError(f"{argv[0]} failed" + (f": {detail[-1]}" if detail else ""))
