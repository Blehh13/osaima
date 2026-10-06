# Configuration

Nothing tunable is hard-coded. Each component reads its settings from one place,
every setting has a default and a validated range, and a bad value is reported
(never silently ignored). This page lists them all.

| Component | Where you set it | Bad values |
|---|---|---|
| Assistant (`osaima-agent`) | `~/.config/osaima/agent.toml` | refused at startup, naming the setting; typos get a "did you mean" hint |
| AI Core (`osaima-mcp-daemon`) | `OSAIMA_*` environment variables | refused at startup, naming the variable |
| Shell backend (`osaima-shell`) | `OSAIMA_*` environment variables | refused at startup, naming the variable |
| Shell interface | `wm.shell({ ... })` in `wm.lua` (edit it in the Config app) | default used; a warning in the developer console |

Environment variables for the services go in the service's environment: the
session script (`/usr/bin/osaima-session`) inherits your login environment, and
systemd users can set them with `Environment=` in a drop-in unit.

## Locations

| Variable | Meaning | Default |
|---|---|---|
| `OSAIMA_MCP_SOCKET` | AI Core socket (daemon, agent and shell all read this) | `$XDG_RUNTIME_DIR/osaima/mcp.sock`, else `/tmp/osaima-<uid>/mcp.sock` |
| `OSAIMA_AGENT_SOCKET` | Assistant socket (agent and shell read this) | `agent.sock` next to the AI Core socket |
| `OSAIMA_AGENT_CONFIG` | Assistant settings file | `$XDG_CONFIG_HOME/osaima/agent.toml` |
| `OSAIMA_LOG` | Log level for the daemon and the agent | `info` |
| `ANTHROPIC_API_KEY` | Claude API key (or sign in with `ant auth login`) | unset: cloud use is off anyway |

Sockets are environment-only on purpose: the shell has to agree with the
services about where they are, so they can't live in one component's file.

## Assistant: `agent.toml`

Unknown sections or keys are an error. Every key is optional.

### `[local]`: the Ollama model

| Key | Default | Range | Meaning |
|---|---|---|---|
| `enabled` | `true` | | Use a local model |
| `url` | `http://127.0.0.1:11434` | | Ollama address |
| `model` | chosen from RAM | | Model name; set to pin one |
| `model_ladder` | see below | | RAM to model table used when `model` is unset |
| `context_tokens` | `8192` | 512 to 1048576 | Context window requested |
| `timeout_s` | `120` | 1 to 3600 | One model turn |
| `connect_timeout_s` | `3` | 0.1 to 60 | Reaching Ollama |
| `temperature` | `0.2` | 0 to 2 | Randomness; low suits tool use |
| `keep_alive` | `"10m"` | | How long Ollama keeps the model loaded |
| `history_messages` | `24` | 2 to 500 | Most recent messages sent each turn |

Default `model_ladder`: below 6 GB RAM `qwen2.5:1.5b-instruct`, from 6 GB
`qwen2.5:3b-instruct`, from 14 GB `qwen2.5:7b-instruct`. Replace it with your own:

```toml
[[local.model_ladder]]
min_ram_gb = 0
model = "llama3.2:3b"

[[local.model_ladder]]
min_ram_gb = 24
model = "llama3.1:8b"
```

### `[cloud]`: Claude (off by default)

| Key | Default | Range | Meaning |
|---|---|---|---|
| `enabled` | `false` | | Allow cloud use. While `false`, nothing leaves the computer |
| `model` | `claude-opus-5-5` | | Claude model |
| `effort` | `medium` | low, medium, high, xhigh, max | Thinking depth against cost |
| `max_tokens` | `16000` | 256 to 128000 | Longest reply |
| `server_fallback` | `true` | | Let Anthropic retry a declined request on its recommended model |
| `timeout_s` | `120` | 1 to 3600 | One request |
| `max_retries` | `2` | 0 to 10 | Automatic retries on network or server errors |
| `json_retries` | `2` | 0 to 10 | Re-asks when streamed tool input is unreadable |

### `[policy]`: what needs your approval

| Key | Default | Meaning |
|---|---|---|
| `confirm` | `"destructive"` | `"destructive"`: only tools marked destructive ask. `"all_changes"`: every change asks |
| `deny` | `[]` | Tool names that may never run, e.g. `["power_action"]` |
| `approval_timeout_s` | `120` | Seconds before an unanswered approval counts as "no" (1 to 3600) |

### `[limits]`: bounds on one request

| Key | Default | Range | Meaning |
|---|---|---|---|
| `max_steps` | `6` | 1 to 20 | Tool calls per request before giving up |
| `max_tool_output_chars` | `8000` | 200 to 1000000 | Tool output shown to the model |
| `event_output_chars` | `2000` | 0 to 1000000 | Tool output shown in the chat |
| `event_structured_bytes` | `20000` | 0 to 104857600 | Structured data sent to the shell |
| `max_conversations` | `32` | 1 to 1000 | Conversations kept in memory |
| `max_message_chars` | `8000` | 1 to 1000000 | Longest accepted user message |
| `max_line_bytes` | `1048576` | 4096 to 104857600 | Longest message on the socket |
| `client_tool_timeout_s` | `30` | 1 to 3600 | Wait for the shell to run a shell-side tool |
| `core_request_timeout_s` | `30` | 1 to 3600 | Wait for the AI Core |

### `[audit]` and `[agent]`

| Key | Default | Meaning |
|---|---|---|
| `audit.path` | `~/.local/state/osaima/audit.jsonl` | Action log; relative paths are relative to `agent.toml` |
| `audit.max_bytes` | `5242880` | Rotate the log above this size (1024 up) |
| `agent.system_prompt_file` | built in | A text file replacing the assistant's instructions |

## AI Core: `OSAIMA_*` variables

| Variable | Default | Range | Meaning |
|---|---|---|---|
| `OSAIMA_SAMPLE_INTERVAL_MS` | `2000` | 250 to 60000 | How often system data is re-sampled |
| `OSAIMA_MAX_MESSAGE_BYTES` | `1048576` | 1024 to 67108864 | Longest accepted message |
| `OSAIMA_HELPER_TIMEOUT_MS` | `5000` | 500 to 120000 | Limit for `wpctl`, `loginctl` and sway calls |
| `OSAIMA_CONNECT_TIMEOUT_MS` | `4000` | 200 to 60000 | Limit per step of the connectivity check |
| `OSAIMA_CONNECTIVITY_HOST` | `example.com` | host name or address | Tested when none is given |
| `OSAIMA_PROCESS_LIST_DEFAULT` | `15` | 1 to 1000 | Processes listed by default |
| `OSAIMA_PROCESS_LIST_MAX` | `200` | 1 to 1000 | Most processes one call may list |
| `OSAIMA_SEARCH_MAX_DEPTH` | `8` | 1 to 64 | Folder levels file search descends |
| `OSAIMA_SEARCH_MAX_ENTRIES` | `100000` | 1000 to 10000000 | Entries file search examines |
| `OSAIMA_SEARCH_TIME_BUDGET_MS` | `3000` | 100 to 60000 | Time for one file search |
| `OSAIMA_MAX_VOLUME_PERCENT` | `150` | 100 to 200 | Highest volume `set_volume` accepts |
| `OSAIMA_TERMINAL` (or `TERMINAL`) | `foot` | one program | Terminal for apps marked `Terminal=true` |

The tool descriptions the assistant sees reflect these values.

## Shell backend: `OSAIMA_*` variables

| Variable | Default | Range | Meaning |
|---|---|---|---|
| `OSAIMA_SHELL_COMMAND_TIMEOUT_S` | `30` | 1 to 3600 | Terminal app commands are stopped after this |
| `OSAIMA_SHELL_OUTPUT_LIMIT_BYTES` | `262144` | 1024 to 67108864 | Terminal app output cap |
| `OSAIMA_CORE_TIMEOUT_MS` | `3000` | 200 to 120000 | Wait for the AI Core |
| `OSAIMA_AGENT_TIMEOUT_MS` | `15000` | 500 to 300000 | Wait for the assistant to accept a request |

## Shell interface: `wm.shell({ ... })`

Set in `wm.lua` (every option is documented there, commented out with its
default). Repeated calls merge.

| Key | Default | Range |
|---|---|---|
| `clock_ms` | `1000` | 250 to 60000 |
| `stats_poll_ms` | `3000` | 500 to 60000 |
| `monitor_poll_ms` | `1500` | 250 to 60000 |
| `task_poll_ms` | `2000` | 500 to 60000 |
| `notify_ms` / `notify_action_ms` | `3200` / `7000` | 500 to 60000 / 120000 |
| `proactive_enabled` | `true` | true or false |
| `proactive_interval_ms` | `8000` | 1000 to 600000 |
| `proactive_first_ms` | `4000` | 0 to 600000 |
| `proactive_quiet_ms` | `12000` | 1000 to 3600000 |
| `cpu_alert_percent` / `memory_alert_percent` | `70` / `85` | 1 to 100 |
| `grid_suggest_windows` | `4` | 2 to 50 |
| `star_count` | `320` | 0 to 2000 |
| `dock_apps` | the ten built-in apps | list of app ids |
| `search_url` | Wikipedia search | http(s) address containing `%s` |
| `bookmarks` | six starter links | list of `{ name, url }` (http or https only) |
| `assistant_suggestions` | five prompts | up to 12 short strings |
| `launcher_suggestions` | six prompts | up to 12 short strings |

Window-manager settings (layout, gaps, workspaces, key bindings, rules) are the
other `wm.*` calls in the same file.

## Packaging

The live ebuilds fetch `https://github.com/Blehh13/osaima.git` (each sets
`EGIT_REPO_URI`). To build from a fork or mirror, copy the ebuild into your own
overlay and change that one line; `EGIT_BRANCH` and `EGIT_COMMIT` in the same
file select a branch or exact commit.

## Not configurable on purpose

These are safety rules, not preferences. Making them settings would only give
a way to switch the protection off.

- The AI Core never signals PID 0, PID 1 or itself, and non-root users can only
  signal their own processes.
- The signals `kill_process` may send, and the actions `power_action` may take,
  are fixed lists. Arbitrary command lines are never run on the model's behalf.
- File search never leaves the home folder; `launch_app` only starts programs
  that have a desktop entry.
- Sockets are created owner-only (mode `0600`), and connections from other
  users are refused.
- Destructive tools always ask first. You can ask for more confirmation
  (`confirm = "all_changes"`) or remove tools (`deny`), but not less.
- Protocol identifiers: JSON-RPC error codes, MCP protocol versions, the sway
  IPC wire format and Anthropic API feature names.
