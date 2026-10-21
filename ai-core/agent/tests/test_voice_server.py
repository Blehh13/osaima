"""The standalone voice service (`osaima-voice`) over its socket."""

from __future__ import annotations

import asyncio
import json
import os
import stat
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest

# Fixtures and fakes shared with the voice tests (the helper fixture needs re-exporting).
from test_voice import FakeStt, FakeTts, helpers, make, socket_dir  # noqa: F401

from osaima_agent import config
from osaima_agent.voice import VoiceService
from osaima_agent.voice.server import VoiceRpcServer


class Peer:
    """A client that makes one call at a time, like the shell does."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.ids = 0

    async def call(self, method: str, **params: Any) -> dict[str, Any]:
        reader, writer = await asyncio.open_unix_connection(str(self.path))
        self.ids += 1
        request = {"jsonrpc": "2.0", "id": self.ids, "method": method, "params": params}
        writer.write(json.dumps(request).encode() + b"\n")
        await writer.drain()
        reply = json.loads(await asyncio.wait_for(reader.readline(), 5))
        writer.close()
        assert reply["id"] == self.ids
        return reply

    async def result(self, method: str, **params: Any) -> Any:
        reply = await self.call(method, **params)
        assert "error" not in reply, reply
        return reply["result"]

    async def wait_for_result(self, after: int) -> dict[str, Any]:
        for _ in range(300):
            last = (await self.result("voice.status"))["last"]
            if last and last["seq"] > after:
                return last
            await asyncio.sleep(0.05)
        raise AssertionError("no new result")


async def serve(
    socket_dir: Path, voice: VoiceService
) -> AsyncIterator[tuple[Peer, asyncio.Task[None]]]:
    path = socket_dir / "voice.sock"
    task = asyncio.create_task(VoiceRpcServer(voice).serve(path))
    for _ in range(100):
        if path.exists():
            break
        await asyncio.sleep(0.01)
    try:
        yield Peer(path), task
    finally:
        task.cancel()


@pytest.fixture
async def peer(
    socket_dir: Path, tmp_path: Path, helpers: dict[str, Any]  # noqa: F811
) -> AsyncIterator[Peer]:
    voice = make(tmp_path, helpers, stt=FakeStt("open the terminal"), tts=FakeTts())
    async for client, _task in serve(socket_dir, voice):
        yield client


async def test_the_socket_is_private_and_status_says_what_works(peer: Peer) -> None:
    assert stat.S_IMODE(os.stat(peer.path).st_mode) == 0o600
    status = await peer.result("voice.status")
    assert status["can_listen"] is True and status["can_speak"] is True
    assert status["state"] == "idle"
    assert status["last"] is None


async def test_listen_stop_then_read_the_words_from_status(peer: Peer) -> None:
    await peer.result("voice.listen")
    assert (await peer.result("voice.status"))["state"] == "listening"
    await peer.result("voice.stop")
    first = await peer.wait_for_result(after=0)
    assert first == {"seq": 1, "text": "open the terminal"}

    await peer.result("voice.listen")  # a second recording gets the next number
    await peer.result("voice.stop")
    assert (await peer.wait_for_result(after=1))["seq"] == 2


async def test_problems_are_reported_in_last(
    socket_dir: Path, tmp_path: Path, helpers: dict[str, Any]  # noqa: F811
) -> None:
    voice = make(tmp_path, helpers, stt=FakeStt(RuntimeError("model missing")))
    async for client, _task in serve(socket_dir, voice):
        await client.result("voice.listen")
        await client.result("voice.stop")
        last = await client.wait_for_result(after=0)
        assert "model missing" in last["error"]
        assert "text" not in last


async def test_speak_silence_and_cancel(peer: Peer) -> None:
    await peer.result("voice.speak", text="Hello there")
    for _ in range(100):
        if (await peer.result("voice.status"))["state"] == "idle":
            break
        await asyncio.sleep(0.05)
    await peer.result("voice.silence")  # nothing playing: harmless
    await peer.result("voice.listen")
    await peer.result("voice.cancel")
    assert (await peer.result("voice.status"))["state"] == "idle"


async def test_bad_requests_get_proper_errors(peer: Peer) -> None:
    assert (await peer.call("voice.dance"))["error"]["code"] == -32601
    assert (await peer.call("voice.speak"))["error"]["code"] == -32602
    assert (await peer.call("voice.stop"))["error"]["code"] == -32002  # not listening
    await peer.result("voice.listen")
    assert (await peer.call("voice.listen"))["error"]["code"] == -32001  # busy
    await peer.result("voice.cancel")


async def test_garbage_does_not_kill_the_service(peer: Peer) -> None:
    reader, writer = await asyncio.open_unix_connection(str(peer.path))
    writer.write(b"not json\n")
    await writer.drain()
    reply = json.loads(await asyncio.wait_for(reader.readline(), 5))
    assert reply["error"]["code"] == -32700
    writer.close()
    assert (await peer.result("voice.status"))["state"] == "idle"


def test_the_socket_location_can_be_overridden(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("OSAIMA_VOICE_SOCKET", "/run/custom-voice.sock")
    assert config.voice_socket_path() == Path("/run/custom-voice.sock")
    monkeypatch.delenv("OSAIMA_VOICE_SOCKET")
    assert config.voice_socket_path().name == "voice.sock"
