"""The real speech engines, end to end, with no microphone.

Piper says a sentence, faster-whisper must hear it back, and the whole
`VoiceService` is driven with a "microphone" that plays the synthesized file.
Needs the `voice` extra and a downloaded Piper voice, plus network for the
speech model, so it only runs when `OSAIMA_VOICE_SMOKE` is set (the
`voice-smoke` workflow does that).
"""

from __future__ import annotations

import asyncio
import os
import shutil
import sys
import textwrap
from pathlib import Path
from typing import Any

import pytest

from osaima_agent.config import VoiceConfig
from osaima_agent.voice import VoiceService
from osaima_agent.voice.engines import CommandSynthesizer, WhisperTranscriber

pytestmark = pytest.mark.skipif(
    not os.environ.get("OSAIMA_VOICE_SMOKE"),
    reason="set OSAIMA_VOICE_SMOKE=1 (needs faster-whisper, piper and downloaded models)",
)

SENTENCE = "Please open the terminal."

# A stand-in microphone: "records" a prepared file, then waits to be stopped.
FAKE_MIC = textwrap.dedent(
    """
    import shutil, signal, sys, time
    stop = []
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    shutil.copyfile(sys.argv[1], sys.argv[2])
    while not stop:
        time.sleep(0.02)
    """
)


def models_dir() -> Path:
    return Path(os.environ.get("OSAIMA_VOICE_MODELS", Path.home() / ".local/share/osaima/voice"))


def config() -> VoiceConfig:
    return VoiceConfig(stt_model=os.environ.get("OSAIMA_VOICE_STT", "tiny.en"), min_record_s=0.2)


def synthesizer(cfg: VoiceConfig) -> CommandSynthesizer:
    return CommandSynthesizer(
        cfg.tts_command, cfg.tts_voice, models_dir(), shutil.which, cfg.command_timeout_s
    )


async def test_piper_speech_is_understood_by_whisper(tmp_path: Path) -> None:
    cfg = config()
    tts = synthesizer(cfg)
    assert tts.problem() == "", tts.problem()
    wav = tmp_path / "speech.wav"
    await tts.synthesize(SENTENCE, wav)
    size = (await asyncio.to_thread(wav.stat)).st_size
    assert size > 10_000  # real audio, not an empty file

    stt = WhisperTranscriber(cfg, models_dir())
    assert stt.problem() == ""
    text = await asyncio.to_thread(stt.transcribe, wav)
    assert "terminal" in text.lower(), f"heard {text!r}"


async def test_the_voice_service_with_the_real_engines(tmp_path: Path) -> None:
    cfg = config()
    spoken = tmp_path / "spoken.wav"
    await synthesizer(cfg).synthesize(SENTENCE, spoken)
    mic = tmp_path / "mic.py"
    await asyncio.to_thread(mic.write_text, FAKE_MIC)

    service = VoiceService(
        VoiceConfig(
            stt_model=cfg.stt_model,
            min_record_s=0.2,
            record_command=(sys.executable, str(mic), str(spoken), "{output}"),
            play_command=(sys.executable, "-c", "pass", "{input}"),
        ),
        models_dir(),
        tmp_path / "work",
        which=lambda program: program,
    )
    events: list[dict[str, Any]] = []

    async def collect(event: dict[str, Any]) -> None:
        events.append(event)

    await service.start_listening(collect)
    await service.stop_listening(collect)
    for _ in range(600):  # the first run loads the model
        if any(e["type"] in ("voice_transcript", "voice_error") for e in events):
            break
        await asyncio.sleep(0.1)
    result = next(e for e in events if e["type"] in ("voice_transcript", "voice_error"))
    assert result["type"] == "voice_transcript", result
    assert "terminal" in result["text"].lower(), result

    await service.speak("Opening the terminal.", collect)  # real Piper, stub player
    for _ in range(300):
        if events[-1:] and events[-1] == {"type": "voice_state", "state": "idle"}:
            break
        await asyncio.sleep(0.1)
    assert not any(e["type"] == "voice_error" for e in events), events
