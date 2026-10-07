# OSAIMA agent (`osaima-agent`)

Turns a request like "what's using my memory?" or "close Firefox" into tool
calls on the [AI Core](../mcp-daemon/README.md), asks the user before anything
risky, and streams the answer back to the shell.

```
shell ── agent.sock (JSON-RPC) ──► osaima-agent ── mcp.sock (MCP) ──► mcp-daemon
                                      │
                                      ├─► Ollama (local model, default)
                                      └─► Claude API (optional fallback)
```

## Running

```bash
pip install -e .                       # Python 3.11+
osaima-agent                           # serve on $XDG_RUNTIME_DIR/osaima/agent.sock
osaima-agent ask "how much disk space do I have?"
osaima-agent ask --model cloud "why is my laptop slow?"
```

The AI Core daemon must be running. For the local model, install
[Ollama](https://ollama.com), then let the agent check everything and download
the model it recommends for this computer:

```bash
osaima-agent doctor            # what's working, and how to fix what isn't
osaima-agent doctor --pull     # also downloads the local model, with progress
```

Unless `local.model` is set, the model is chosen from the installed RAM:
`qwen2.5:7b-instruct` from 14 GB, `qwen2.5:3b-instruct` from 6 GB,
`qwen2.5:1.5b-instruct` below that. All three support tool calling; the smaller
ones follow instructions less reliably, which is what the evaluation measures.

### On Interstellar OS

`emerge sys-apps/osaima-agent` installs it; `app-misc/interstellar-meta` pulls
it in together with Ollama (`ollama` USE flag). The desktop session
(`osaima-session`, shipped with the shell) starts the AI Core, this service and
Ollama, restarts any of them that crash, and stops them at logout. systemd
users get equivalent user units instead.

## Settings (`~/.config/osaima/agent.toml`)

```toml
[local]
# model = "qwen2.5:7b-instruct"   # default: chosen from this computer's RAM
url = "http://127.0.0.1:11434"

[cloud]
enabled = false          # nothing leaves the computer unless this is true
model = "claude-opus-5-5"
effort = "medium"        # low | medium | high | xhigh | max

[policy]
confirm = "destructive"  # or "all_changes" to approve every change
deny = []                # tool names that may never run, e.g. ["power_action"]

[limits]
max_steps = 6            # tool calls per request

[agent]
# system_prompt_file = "prompt.txt"   # replace the assistant's instructions
```

Every value (temperature, timeouts, retries, limits, the model-by-RAM table,
the audit log...) has a default and a validated range; unknown keys are
rejected with a hint. The full list is in
[docs/configuration.md](../../docs/configuration.md).

The Claude API key is read from `ANTHROPIC_API_KEY` or an `ant auth login`
profile. It is never stored in the settings file.

## How a request is handled

1. The shell sends `agent.chat`. The agent sends the conversation to the local
   model with the AI Core's tool list.
2. For each tool call the model makes, the policy decides:
   - **read-only** tools (stats, process list, file search) run immediately;
   - **reversible** tools (volume, brightness, opening apps, focusing windows)
     run immediately unless `confirm = "all_changes"`;
   - **destructive** tools (ending processes, closing windows, power actions)
     wait for the user to approve. No answer within 2 minutes counts as "no".
3. Results go back to the model as data, until it answers or hits `max_steps`.
4. Every attempted action is appended to `~/.local/state/osaima/audit.jsonl`.

Model selection (`model` in `agent.chat`): `auto` tries the local model first
and falls back to Claude (if enabled) when the local model is unavailable,
errors, or makes two invalid tool calls in a row. `local` and `cloud` force one.

### Using Claude

Requests to Claude keep each conversation append-only and replay Claude's own
replies verbatim, so its reasoning stays valid between turns and the prompt
cache stays warm. They also opt into Anthropic's server-side refusal fallback
(`fallbacks: "default"`): if a safety classifier declines a request, Anthropic
retries it on its recommended fallback model instead of failing.

## Shell protocol

Newline-delimited JSON-RPC 2.0 on the socket. Only the same user can connect.

| Method | Params | Result |
|---|---|---|
| `agent.status` | | configured models and whether they're reachable |
| `agent.chat` | `message`, `conversation_id?`, `model?`, `client_tools?` | `{turn_id, conversation_id}` |
| `agent.approve` | `turn_id`, `call_id`, `approved` | `{}` |
| `agent.client_tool_result` | `turn_id`, `call_id`, `ok`, `output` | `{}` |
| `agent.cancel` | `turn_id` | `{cancelled}` |
| `agent.reset` | `conversation_id` | `{}` |
| `agent.audit` | `limit?` | `{entries}` |
| `agent.voice.status` | | what works (`can_listen`, `can_speak`, `problems`, `state`) |
| `agent.voice.listen` / `stop` / `cancel` | | `{}`: start, finish (and transcribe) or discard a recording |
| `agent.voice.speak` | `text` | `{}`: read it aloud |
| `agent.voice.silence` | | `{}`: stop speaking |

Progress arrives as `agent.event` notifications with a `turn_id` and a `type`:
`text` (`delta`), `tool_call`, `approval_required`, `client_tool_call`,
`tool_result`, `notice`, `error`, `cancelled`, and finally `done` (`text`,
`provider`, `model`, `steps`).

Voice progress arrives as `agent.event` notifications without a `turn_id`:
`voice_state` (`state`: `idle`, `listening`, `transcribing`, `speaking`),
`voice_transcript` (`text`) and `voice_error` (`message`).

`client_tools` (sent with a conversation's first message) are tools the shell
implements itself, such as opening a built-in app. When the model calls one,
the agent emits `client_tool_call` and waits for `agent.client_tool_result`.

## Voice

Push-to-talk, entirely on this computer: the shell's microphone button calls
`agent.voice.listen`, then `agent.voice.stop`; the agent transcribes the
recording with [faster-whisper](https://github.com/SYSTRAN/faster-whisper) and
returns the words as a `voice_transcript` event, which the chat sends like any
typed message. Replies can be read aloud with [Piper](https://github.com/OHF-Voice/piper1-gpl).
Recordings are made with PipeWire (`pw-record`) or ALSA (`arecord`), kept in a
private temporary folder, and deleted as soon as they are transcribed. Nothing
is sent over the network, except the one-time download of the models.

Set up (text chat works without any of this):

```bash
pip install "osaima-agent[voice]"            # faster-whisper and Piper
python -m piper.download_voices en_US-lessac-medium --data-dir ~/.local/share/osaima/voice
osaima-agent doctor                          # shows what is missing
```

faster-whisper downloads its speech model (`base.en`, about 150 MB) the first
time it is used. Every setting is in the `[voice]` table of `agent.toml`; see
`docs/configuration.md`. Voice is verified in CI with fake engines and real
helper processes; it has not yet been tried with a real microphone.

## Development

```bash
pip install -e ".[dev]"
pytest                     # set OSAIMA_DAEMON_BIN to also test against the real AI Core
ruff check . && ruff format --check .
mypy
```

CI runs all of these. If the lint/format step fails, download the
`ruff-patch-agent` artifact and apply it with `git apply`.
