"""The voice service as its own program: `osaima-voice`.

The desktop shell talks to it over a per-user Unix socket (`voice.sock`) with
newline-delimited JSON-RPC 2.0, one request and one reply per call, so the shell
needs nothing but a plain socket. There are no pushed events: what happened is
read back with `voice.status`, whose `last` field holds the most recent result
and a counter (`seq`) that goes up each time there is a new one.

Methods:

- ``voice.status``   what works, the current state, and ``last``
- ``voice.listen``   start recording (a recording also stops by itself after
  ``max_record_s``)
- ``voice.stop``     stop recording; the words appear in ``last.text``
- ``voice.cancel``   stop recording and throw the audio away
- ``voice.speak``    ``{text}``: read it aloud
- ``voice.silence``  stop speaking

``last`` is ``{"seq": n, "text": "..."}`` for a transcript or
``{"seq": n, "error": "..."}`` for a failure.

Only processes of the same user may connect (checked with SO_PEERCRED).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
from pathlib import Path
from typing import Any

from .. import __version__, config
from ..server import _is_live, _prepare_socket_dir, _remove_socket, _same_user
from .service import VoiceError, VoiceService

log = logging.getLogger(__name__)

INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
BUSY = -32001
UNAVAILABLE = -32002
MAX_LINE_BYTES = 64 * 1024


class VoiceRpcServer:
    def __init__(self, voice: VoiceService) -> None:
        self._voice = voice
        self._seq = 0
        self._pending: dict[str, Any] | None = None  # a result the service is still wrapping up
        self._last: dict[str, Any] | None = None

    async def serve(self, path: Path) -> None:
        if await asyncio.to_thread(_prepare_socket_dir, path) and await _is_live(path):
            raise RuntimeError(f"another voice service is already listening on {path}")
        await asyncio.to_thread(_remove_socket, path)
        old_umask = os.umask(0o177)  # socket created as 0600
        try:
            server = await asyncio.start_unix_server(
                self._handle_connection, path=str(path), limit=MAX_LINE_BYTES
            )
        finally:
            os.umask(old_umask)
        log.info("voice listening on %s", path)
        try:
            async with server:
                await server.serve_forever()
        finally:
            _remove_socket(path)

    async def _handle_connection(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        sock = writer.get_extra_info("socket")
        if sock is not None and not _same_user(sock):
            log.warning("rejected connection from another user")
            writer.close()
            return
        try:
            while line := await reader.readline():
                if line.strip():
                    writer.write(await self._answer(line))
                    await writer.drain()
        except (ConnectionError, asyncio.LimitOverrunError, ValueError) as err:
            log.debug("connection closed: %s", err)
        finally:
            writer.close()

    async def _answer(self, line: bytes) -> bytes:
        request_id: Any = None
        try:
            try:
                message = json.loads(line)
            except json.JSONDecodeError as err:
                raise _Error(-32700, f"parse error: {err}") from err
            if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
                raise _Error(INVALID_REQUEST, "expected a JSON-RPC 2.0 object")
            request_id = message.get("id")
            params = message.get("params") or {}
            if not isinstance(params, dict):
                raise _Error(INVALID_REQUEST, "params must be an object")
            result = await self._dispatch(str(message.get("method")), params)
            reply: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "result": result}
        except _Error as err:
            reply = {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": err.code, "message": str(err)},
            }
        return json.dumps(reply, default=str).encode() + b"\n"

    async def _collect(self, event: dict[str, Any]) -> None:
        """Remember the result of a recording; ignore state changes (they are in `status`)."""
        if event.get("type") == "voice_transcript":
            self._seq += 1
            self._pending = {"seq": self._seq, "text": str(event.get("text", ""))}
        elif event.get("type") == "voice_error":
            self._seq += 1
            self._pending = {"seq": self._seq, "error": str(event.get("message", ""))}

    def _publish(self) -> dict[str, Any] | None:
        """The latest result, shown only once the service is idle again, so a caller
        that reacts to it can start the next recording at once."""
        if self._pending is not None and self._voice.state != "transcribing":
            self._last, self._pending = self._pending, None
        return self._last

    async def _dispatch(self, method: str, params: dict[str, Any]) -> Any:
        voice = self._voice
        try:
            match method:
                case "initialize" | "ping":
                    return {"name": "osaima-voice", "version": __version__}
                case "voice.status":
                    return {**voice.status(), "last": self._publish()}
                case "voice.listen":
                    await voice.start_listening(self._collect)
                case "voice.stop":
                    await voice.stop_listening(self._collect)
                case "voice.cancel":
                    await voice.cancel_listening()
                case "voice.speak":
                    text = params.get("text")
                    if not isinstance(text, str):
                        raise _Error(INVALID_PARAMS, "text must be a string")
                    await voice.speak(text, self._collect)
                case "voice.silence":
                    await voice.stop_speaking()
                case _:
                    raise _Error(METHOD_NOT_FOUND, f"method not found: {method}")
        except VoiceError as err:
            raise _Error(BUSY if err.busy else UNAVAILABLE, str(err)) from err
        return {}


class _Error(Exception):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


async def serve(cfg: config.AgentConfig, socket_path: Path | None = None) -> None:
    voice = VoiceService(cfg.voice, cfg.voice_models_dir, config.runtime_dir() / "voice")
    try:
        await VoiceRpcServer(voice).serve(socket_path or cfg.voice_socket_path)
    finally:
        await voice.aclose()


def main() -> int:
    """Entry point of `osaima-voice`."""
    logging.basicConfig(level=logging.INFO, format="%(name)s: %(message)s")
    try:
        cfg = config.load()
    except config.ConfigError as err:
        print(f"error: {err}")
        return 2
    if not cfg.voice.enabled:
        print("voice is turned off in the settings")
        return 0
    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(serve(cfg))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
