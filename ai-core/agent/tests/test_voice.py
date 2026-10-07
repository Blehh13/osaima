"""Voice: the recording / transcribing / speaking flow, with real helper processes
standing in for the audio tools and fakes for the speech engines."""

from __future__ import annotations

import asyncio
import json
import sys
import tempfile
import textwrap
import wave
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest
from conftest import FakeTools, ScriptedProvider, reply

from osaima_agent import config
from osaima_agent.agent import Agent
from osaima_agent.audit import AuditLog
from osaima_agent.config import ConfigError, VoiceConfig
from osaima_agent.policy import Policy
from osaima_agent.server import AgentServer
from osaima_agent.voice import VoiceError, VoiceService
from osaima_agent.voice.audio import AudioError, fill, recorded_seconds, resolve_command
from osaima_agent.voice.engines import CommandSynthesizer, load_audio
from osaima_agent.voice.service import speakable

RECORDER = textwrap.dedent(
    """
    import signal, sys, time
    stop = []
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    seconds = float(sys.argv[2])
    with open(sys.argv[1], "wb") as f:
        f.write(b"RIFF" + bytes(40) + bytes(int(seconds * 32000)))
    while not stop:
        time.sleep(0.02)
    """
)


class FakeStt:
    def __init__(self, text: str | Exception = "open the terminal") -> None:
        self.text = text
        self.heard: list[Path] = []

    def problem(self) -> str:
        return ""

    def transcribe(self, wav: Path) -> str:
        self.heard.append(wav)
        assert wav.exists()
        if isinstance(self.text, Exception):
            raise self.text
        return self.text


class FakeTts:
    def __init__(self) -> None:
        self.said: list[str] = []

    def problem(self) -> str:
        return ""

    async def synthesize(self, text: str, out: Path) -> None:
        self.said.append(text)
        await asyncio.to_thread(out.write_bytes, b"RIFF" + bytes(40))


class Events:
    """Collects what the service reports."""

    def __init__(self) -> None:
        self.items: list[dict[str, Any]] = []

    async def __call__(self, event: dict[str, Any]) -> None:
        self.items.append(event)

    def states(self) -> list[str]:
        return [e["state"] for e in self.items if e["type"] == "voice_state"]

    async def wait_for(self, kind: str) -> dict[str, Any]:
        for _ in range(300):
            for event in self.items:
                if event["type"] == kind:
                    return event
            await asyncio.sleep(0.02)
        raise AssertionError(f"no {kind} event; got {self.items}")

    async def wait_idle(self) -> None:
        for _ in range(300):
            if self.states()[-1:] == ["idle"]:
                return
            await asyncio.sleep(0.02)
        raise AssertionError(f"never went idle; got {self.items}")


@pytest.fixture
def helpers(tmp_path: Path) -> dict[str, Any]:
    script = tmp_path / "recorder.py"
    script.write_text(RECORDER)
    return {
        "record": (sys.executable, str(script), "{output}", "1.0"),
        "play": (sys.executable, "-c", "import time; time.sleep(0.05)", "{input}"),
        "slow_play": (sys.executable, "-c", "import time; time.sleep(30)", "{input}"),
    }


def make(
    tmp_path: Path,
    helpers: dict[str, Any],
    *,
    stt: Any = None,
    tts: Any = None,
    play: str = "play",
    **overrides: Any,
) -> VoiceService:
    cfg = VoiceConfig(
        record_command=helpers["record"],
        play_command=helpers[play],
        min_record_s=0.2,
        **overrides,
    )
    return VoiceService(
        cfg,
        tmp_path / "models",
        tmp_path / "work",
        transcriber=stt or FakeStt(),
        synthesizer=tts or FakeTts(),
        which=lambda program: program,
    )


def leftovers(tmp_path: Path) -> list[Path]:
    work = tmp_path / "work"
    return list(work.iterdir()) if work.exists() else []


# ── pure helpers ─────────────────────────────────────────────────────────────


def test_speakable_cleans_and_trims_on_a_sentence() -> None:
    assert speakable("  Hello\n\n  there\x07 ", 100) == "Hello there"
    long = "A fairly long first sentence here. Second sentence goes on."
    assert speakable(long, 45) == "A fairly long first sentence here."
    assert speakable("word " * 50, 22) == "word word word word"
    assert speakable("\x00\x01  ", 10) == ""


def test_commands_are_resolved_and_filled() -> None:
    have = {"arecord"}
    candidates = (("pw-record", ("pw-record", "{output}")), ("arecord", ("arecord", "{output}")))
    which = lambda p: p if p in have else None  # noqa: E731
    assert resolve_command((), candidates, which) == ("arecord", "{output}")
    assert resolve_command(("custom", "{output}"), candidates, which) is None
    assert resolve_command(("arecord", "-x"), candidates, which) == ("arecord", "-x")
    assert resolve_command((), candidates, lambda p: None) is None
    assert fill(["a", "{output}", "{keep}"], output="/x") == ["a", "/x", "{keep}"]


def test_recorded_length_comes_from_the_file_size(tmp_path: Path) -> None:
    wav = tmp_path / "a.wav"
    wav.write_bytes(bytes(44 + 32000))
    assert recorded_seconds(wav) == pytest.approx(1.0)
    assert recorded_seconds(tmp_path / "missing.wav") == 0.0


def write_wav(path: Path, rate: int, channels: int, seconds: float, width: int = 2) -> None:
    with wave.open(str(path), "wb") as out:
        out.setnchannels(channels)
        out.setsampwidth(width)
        out.setframerate(rate)
        out.writeframes(bytes(int(rate * seconds) * channels * width))


def test_audio_is_loaded_as_16k_mono(tmp_path: Path) -> None:
    np = pytest.importorskip("numpy")
    plain = tmp_path / "plain.wav"
    write_wav(plain, 16000, 1, 1.0)
    samples = load_audio(plain)
    assert samples.dtype == np.float32 and len(samples) == 16000

    other = tmp_path / "other.wav"  # what Piper writes: 22.05 kHz; and a stereo file
    write_wav(other, 22050, 2, 1.0)
    converted = load_audio(other)
    assert len(converted) == 16000
    assert float(np.abs(converted).max()) <= 1.0

    write_wav(tmp_path / "eight.wav", 16000, 1, 0.1, width=1)
    with pytest.raises(AudioError, match="16-bit"):
        load_audio(tmp_path / "eight.wav")
    (tmp_path / "junk.wav").write_bytes(b"not audio")
    with pytest.raises(AudioError, match="not a readable"):
        load_audio(tmp_path / "junk.wav")


# ── settings ─────────────────────────────────────────────────────────────────


def test_voice_settings_are_validated() -> None:
    cfg = config.from_dict(
        {
            "voice": {
                "stt_model": "small.en",
                "max_record_s": 12,
                "record_command": ["rec", "{output}"],
            }
        }
    )
    assert cfg.voice.stt_model == "small.en"
    assert cfg.voice.max_record_s == 12.0
    assert cfg.voice.record_command == ("rec", "{output}")
    assert cfg.voice.tts_command[0] == "piper"
    assert config.from_dict({}).voice == VoiceConfig()
    for bad in ({"max_record_s": 0}, {"stt_beam_size": 99}, {"stt_model": " "}, {"nope": 1}):
        with pytest.raises(ConfigError):
            config.from_dict({"voice": bad})
    with pytest.raises(ConfigError, match="did you mean 'stt_model'"):
        config.from_dict({"voice": {"stt_modle": "x"}})


# ── listening ────────────────────────────────────────────────────────────────


async def test_listen_then_stop_gives_a_transcript_and_cleans_up(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    stt = FakeStt("what is using my memory")
    voice = make(tmp_path, helpers, stt=stt)
    events = Events()
    await voice.start_listening(events)
    assert voice.state == "listening"
    await voice.stop_listening(events)
    transcript = await events.wait_for("voice_transcript")
    await events.wait_idle()
    assert transcript["text"] == "what is using my memory"
    assert events.states() == ["listening", "transcribing", "idle"]
    assert len(stt.heard) == 1
    assert leftovers(tmp_path) == []  # the recording is gone


async def test_a_too_short_recording_is_reported_not_transcribed(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    helpers["record"] = (*helpers["record"][:3], "0.0")
    stt = FakeStt()
    voice = make(tmp_path, helpers, stt=stt)
    events = Events()
    await voice.start_listening(events)
    await voice.stop_listening(events)
    error = await events.wait_for("voice_error")
    await events.wait_idle()
    assert "didn't hear" in error["message"]
    assert stt.heard == []
    assert leftovers(tmp_path) == []


async def test_silence_and_recognizer_failures_do_not_break_the_service(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    stt = FakeStt("")
    voice = make(tmp_path, helpers, stt=stt)
    for result, message in (("", "didn't catch"), (RuntimeError("model missing"), "model missing")):
        stt.text = result
        events = Events()
        await voice.start_listening(events)
        await voice.stop_listening(events)
        error = await events.wait_for("voice_error")
        await events.wait_idle()
        assert message in error["message"]
        assert voice.state == "idle"
    stt.text = "works again"
    events = Events()
    await voice.start_listening(events)
    await voice.stop_listening(events)
    assert (await events.wait_for("voice_transcript"))["text"] == "works again"


async def test_listening_twice_is_refused_and_cancel_discards(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    stt = FakeStt()
    voice = make(tmp_path, helpers, stt=stt)
    events = Events()
    await voice.start_listening(events)
    with pytest.raises(VoiceError) as busy:
        await voice.start_listening(events)
    assert busy.value.busy
    await voice.cancel_listening()
    assert voice.state == "idle"
    assert stt.heard == []
    assert leftovers(tmp_path) == []
    with pytest.raises(VoiceError, match="not listening"):
        await voice.stop_listening(events)


async def test_recording_stops_by_itself_at_the_time_limit(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    voice = make(tmp_path, helpers, max_record_s=0.5)
    events = Events()
    await voice.start_listening(events)
    transcript = await events.wait_for("voice_transcript")  # nobody called stop
    await events.wait_idle()
    assert transcript["text"] == "open the terminal"


async def test_a_recorder_that_fails_at_once_is_a_clear_error(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    helpers["record"] = (sys.executable, "-c", "import sys; sys.exit('no microphone')")
    voice = make(tmp_path, helpers)
    with pytest.raises(VoiceError, match="no microphone"):
        await voice.start_listening(Events())
    assert voice.state == "idle"
    assert leftovers(tmp_path) == []


def test_the_piper_voice_must_be_downloaded(tmp_path: Path) -> None:
    command = VoiceConfig().tts_command
    make_tts = lambda cmd: CommandSynthesizer(  # noqa: E731
        cmd, "en_US-test", tmp_path, lambda program: program, 5
    )
    assert "not downloaded" in make_tts(command).problem()
    assert "piper.download_voices en_US-test" in make_tts(command).problem()
    (tmp_path / "en_US-test.onnx").write_bytes(b"x")
    assert make_tts(command).problem() == ""
    # A program that doesn't use a voice name (espeak-ng, say) needs no download.
    other = tmp_path / "elsewhere"
    other.mkdir()
    no_voice = CommandSynthesizer(("espeak-ng", "-w", "{output}"), "x", other, lambda p: p, 5)
    assert no_voice.problem() == ""


async def test_missing_tools_are_reported_in_the_status(tmp_path: Path) -> None:
    voice = VoiceService(
        VoiceConfig(),
        tmp_path / "models",
        tmp_path / "work",
        transcriber=FakeStt(),
        synthesizer=FakeTts(),
        which=lambda program: None,
    )
    status = voice.status()
    assert not status["can_listen"] and not status["can_speak"]
    assert len(status["problems"]) == 2
    with pytest.raises(VoiceError, match="no microphone tool"):
        await voice.start_listening(Events())
    with pytest.raises(VoiceError, match="no audio player"):
        await voice.speak("hello", Events())


async def test_turned_off_in_the_settings(tmp_path: Path, helpers: dict[str, Any]) -> None:
    voice = make(tmp_path, helpers, enabled=False)
    assert voice.status()["enabled"] is False
    with pytest.raises(VoiceError, match="turned off"):
        await voice.start_listening(Events())


# ── speaking ─────────────────────────────────────────────────────────────────


async def test_speak_synthesizes_and_plays_then_goes_idle(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    tts = FakeTts()
    voice = make(tmp_path, helpers, tts=tts)
    events = Events()
    await voice.speak("  Your   memory\nis fine.  ", events)
    await events.wait_idle()
    assert tts.said == ["Your memory is fine."]
    assert events.states() == ["speaking", "idle"]
    assert not any(e["type"] == "voice_error" for e in events.items)
    assert leftovers(tmp_path) == []


async def test_speech_can_be_interrupted(tmp_path: Path, helpers: dict[str, Any]) -> None:
    voice = make(tmp_path, helpers, play="slow_play")
    events = Events()
    await voice.speak("a long answer", events)
    await asyncio.sleep(0.3)  # the player is running
    await asyncio.wait_for(voice.stop_speaking(), 5)
    assert voice.state == "idle"
    assert events.states()[-1] == "idle"
    assert leftovers(tmp_path) == []
    with pytest.raises(VoiceError, match="nothing to say"):
        await voice.speak("\x00 \n", events)


async def test_starting_to_listen_interrupts_speech(
    tmp_path: Path, helpers: dict[str, Any]
) -> None:
    voice = make(tmp_path, helpers, play="slow_play")
    events = Events()
    await voice.speak("talking", events)
    await asyncio.sleep(0.3)
    await asyncio.wait_for(voice.start_listening(events), 5)
    assert voice.state == "listening"
    await voice.cancel_listening()


async def test_a_synthesizer_failure_is_reported(tmp_path: Path, helpers: dict[str, Any]) -> None:
    class Broken(FakeTts):
        async def synthesize(self, text: str, out: Path) -> None:
            raise AudioError("piper failed: no such voice")

    voice = make(tmp_path, helpers, tts=Broken())
    events = Events()
    await voice.speak("hello", events)
    error = await events.wait_for("voice_error")
    await events.wait_idle()
    assert "no such voice" in error["message"]
    assert voice.state == "idle"


# ── over the socket ──────────────────────────────────────────────────────────


@pytest.fixture
async def socket_dir() -> AsyncIterator[Path]:
    with tempfile.TemporaryDirectory(prefix="osa-") as d:
        yield Path(d)


async def rpc(
    reader: asyncio.StreamReader, writer: asyncio.StreamWriter, n: int, method: str, **params: Any
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    writer.write(
        json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}).encode() + b"\n"
    )
    await writer.drain()
    events = []
    while True:
        message = json.loads(await asyncio.wait_for(reader.readline(), 5))
        if message.get("id") == n:
            return message, events
        events.append(message["params"])


async def serve(
    socket_dir: Path, audit: AuditLog, voice: VoiceService | None
) -> tuple[asyncio.Task[None], Path]:
    agent = Agent(
        tools=FakeTools(),
        local=ScriptedProvider("local", [reply("ok")]),
        cloud=None,
        policy=Policy(),
        audit=audit,
    )
    path = socket_dir / "agent.sock"
    task = asyncio.create_task(AgentServer(agent, audit, voice=voice).serve(path))
    for _ in range(100):
        if path.exists():
            break
        await asyncio.sleep(0.01)
    return task, path


async def test_voice_over_the_socket(
    socket_dir: Path, audit: AuditLog, tmp_path: Path, helpers: dict[str, Any]
) -> None:
    voice = make(tmp_path, helpers, stt=FakeStt("close firefox"))
    task, path = await serve(socket_dir, audit, voice)
    try:
        reader, writer = await asyncio.open_unix_connection(str(path))
        status, _ = await rpc(reader, writer, 1, "agent.voice.status")
        assert status["result"]["can_listen"] is True
        assert status["result"]["can_speak"] is True

        _, events = await rpc(reader, writer, 2, "agent.voice.listen")
        _, more = await rpc(reader, writer, 3, "agent.voice.stop")
        seen = [*events, *more]
        while not any(e["type"] == "voice_transcript" for e in seen):
            message = json.loads(await asyncio.wait_for(reader.readline(), 5))
            seen.append(message["params"])
        transcript = next(e for e in seen if e["type"] == "voice_transcript")
        assert transcript["text"] == "close firefox"

        again, _ = await rpc(reader, writer, 4, "agent.voice.stop")
        assert again["error"]["code"] == -32002  # not listening
        unknown, _ = await rpc(reader, writer, 5, "agent.voice.dance")
        assert unknown["error"]["code"] == -32601
        bad, _ = await rpc(reader, writer, 6, "agent.voice.speak")
        assert bad["error"]["code"] == -32602
        writer.close()
    finally:
        task.cancel()


async def test_a_dropped_connection_releases_the_microphone(
    socket_dir: Path, audit: AuditLog, tmp_path: Path, helpers: dict[str, Any]
) -> None:
    voice = make(tmp_path, helpers)
    task, path = await serve(socket_dir, audit, voice)
    try:
        reader, writer = await asyncio.open_unix_connection(str(path))
        await rpc(reader, writer, 1, "agent.voice.listen")
        assert voice.state == "listening"
        writer.close()
        for _ in range(100):
            if voice.state == "idle":
                break
            await asyncio.sleep(0.02)
        assert voice.state == "idle"
        assert leftovers(tmp_path) == []
    finally:
        task.cancel()


async def test_without_a_voice_service_status_says_so_and_calls_fail(
    socket_dir: Path, audit: AuditLog
) -> None:
    task, path = await serve(socket_dir, audit, None)
    try:
        reader, writer = await asyncio.open_unix_connection(str(path))
        status, _ = await rpc(reader, writer, 1, "agent.voice.status")
        assert status["result"]["enabled"] is False
        failed, _ = await rpc(reader, writer, 2, "agent.voice.listen")
        assert failed["error"]["code"] == -32002
        writer.close()
    finally:
        task.cancel()
