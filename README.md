# OSAIMA / Interstellar OS

**OSAIMA** (Operating System Adaptive Intelligence Management Architecture) is an
AI-driven Linux operating system interface in which the assistant is the shell.
**Interstellar OS** is the Gentoo-based distribution that ships it as a bootable
live ISO.

You ask in plain words. The assistant works out what you mean, shows exactly what
it will do and how risky that is, asks for approval in proportion to the risk, then
acts on the real desktop, files and system. Every action is logged in a
tamper-evident history, and every command it runs is confined by the Linux kernel.

Final-year project, Department of AIML, Jyothy Institute of Technology.
Team: Akash, Banu B B, Pragna S Bharadwaj, Sunil J Rathore. Guide: Dr. Madhu BR.
The original concept is in [OSAIMA.pdf](OSAIMA.pdf) (Pre-Design Review). The
concept has not changed. Everything below is about how it is built.

---

## Contents

1. [What it does today](#1-what-it-does-today)
2. [What changed, and why](#2-what-changed-and-why)
3. [Architecture in layers](#3-architecture-in-layers)
4. [How a request flows](#4-how-a-request-flows)
5. [Security model](#5-security-model)
6. [Feature reference](#6-feature-reference)
7. [Daemon RPC reference](#7-daemon-rpc-reference)
8. [Repository layout](#8-repository-layout)
9. [Run it for development](#9-run-it-for-development)
10. [Build and boot the ISO](#10-build-and-boot-the-iso)
11. [Testing](#11-testing)
12. [Results](#12-results)
13. [Known limits and work not finished](#13-known-limits-and-work-not-finished)
14. [Commit history of this branch](#14-commit-history-of-this-branch)
15. [Credits](#15-credits)

---

## 1. What it does today

- **A real desktop.** sway (a Wayland tiling compositor) is the window manager. It
  has a top bar (waybar), notifications (mako), a lock screen (swaylock + swayidle),
  its own GTK theme, font (Atkinson Hyperlegible) and wallpapers. The assistant is
  the first window. Other apps tile beside it.
- **Real apps.** Firefox, the Thunar file manager, the Mousepad editor, the foot
  terminal, LibreOffice, the mpv media player and the Xarchiver archive manager all
  ship in the image. The OSAIMA apps (Assistant, Settings, History, Notes, Calendar,
  Monitor, Task manager, About) are separate desktop windows, one Tauri window each.
- **Instant commands.** A local intent router (MiniLM sentence embeddings, ONNX, 66
  intents) handles simple requests like "set brightness to 80%" or "go to workspace
  3" in about 40 ms, with no language model involved.
- **Open-ended requests.** An LLM handles everything else. The default is Google
  Gemini 3.5 Flash-Lite in the cloud, which answers in a few seconds. **Private
  mode** uses an on-device model (Phi-4-mini on llama.cpp) and sends nothing off
  the machine, but it is much slower on a CPU.
- **Risk tiers.** The daemon classifies every action as *only looks*, *makes
  changes (undoable)* or *cannot be undone*. Only the first kind runs without
  asking. The last kind needs approval and the typed word `confirm`.
- **Windows.** The assistant lists, focuses, closes and launches windows, and it
  changes layouts and workspaces through sway's IPC.
- **Files.** The assistant can search, open, show in the file manager, copy, move,
  rename, make folders and send to the trash. These are typed actions, not shell
  text. They are confined to your home folder and each one can be undone.
  Right-click any file in Thunar and choose **Ask OSAIMA** to ask about it.
- **Multi-step plans.** "Make an Invoices folder, move the two invoices into it and
  show it to me" runs as one plan, with one approval and one undo.
- **Agent tasks.** Some jobs have steps that depend on earlier results ("set up a
  Python project with a venv and a test, and check it runs"). For these the model
  proposes a scope (one folder, internet or not), which you approve once. It then
  works step by step and sees each result before the next step. Writes are limited
  by the kernel to the approved folder. Root is never used.
- **Routines.** Save a finished plan by name and run it again later ("run my
  invoices routine").
- **System settings by hand or by voice.** One Settings app covers wallpaper,
  display, night light, sound, network, keyboard and mouse, notifications, screen
  lock, default apps, date and time, the AI mode and automations. The assistant can
  change all of the same settings through the daemon.
- **Automations.** You can ask for things to happen later, on a schedule or when an
  event happens: "shut down after this task finishes", "keep brightness at 40% on
  weekdays from 9 pm to 6 am", "after reboot set my brightness to 70", "at 11:30
  tonight lock the screen". Each automation needs your approval, notifies you when
  it runs and is recorded in History.
- **Health watch and failure diagnosis.** The daemon checks the machine every 20 s:
  CPU temperature, CPU throttling, CPU, memory and IO pressure, memory, disk space,
  battery, the busiest processes and failed services. It warns you once when
  something is wrong and suggests a fix. When you ask "why did X fail?", the AI
  answers from that evidence.
- **History.** One app shows every AI action, newest first, with filters and
  search. It reads the hash-chained audit log, so an edited record is detectable.
- **Learning and prediction.** An on-device behaviour store records what you use.
  It drives suggestions in the side rail, remembered preferences and the routine
  offers. It also predicts: a command you have run on three or more days at about
  this time appears as "Usually around 21:00", with one click to run it or to make
  it a daily automation.
- **Better local model when the hardware allows.** Settings → AI detects the
  graphics cards and their memory. When a card can run a bigger model, Settings
  offers it with its size and downloads it there, with a progress bar, speed, time
  left and Cancel. Without a suitable card, it points you to the Gemini key field.

What it does **not** do yet: voice input (text only for now), confine the
assistant process itself (the kernel limits the commands it runs, not the daemon),
or match cloud speed fully offline. See [docs/review/CLAIMS.md](docs/review/CLAIMS.md)
for every claim with its evidence and its limit.

---

## 2. What changed, and why

This section describes the work on the `feature/lua-window-manager` branch, which
is published as `review-stable`. It starts from the team's earlier work: Pragna's
Lua-driven tiling window manager and web shell, and Akash's daemon groundwork. The
items are grouped by area. Commit hashes are in [§14](#14-commit-history-of-this-branch).

### 2.1 AI core: cloud-first, with the local parts kept local

**Problem.** The on-device model (Phi-4-mini on CPU) took 10 to 220 s per answer.
That is not usable for a demo or for daily use.

**Change.**
- `llm.rs` now has two modes. **Cloud** is the default: Gemini 3.5 Flash-Lite
  through its OpenAI-compatible endpoint. **Local** (Private mode) uses llama.cpp.
  Local is also the offline fallback.
- The API key is saved only in `~/.config/osaima/llm.env` with mode 0600. It is
  never stored in the repo or in the ISO. Settings has a field to paste and check
  the key.
- Requests are forced to IPv4. Unroutable IPv6 on some networks caused 10 to 18 s
  stalls before each answer (set `OSAIMA_IPV6=1` to undo this).
- The **intent router, knowledge search (RAG), safety gates, sandbox, behaviour
  store and audit log all still run on the machine.** Only the open-ended language
  step goes to the cloud.
- Conversation history is kept, so follow-ups ("and the other one?") work.
- The system prompt is fixed for each task type. Per-request facts (router hints,
  your preferences, the current local time) go in the user turn so the prompt
  cache stays valid.
- Small things are answered without any model. For example, the `arithmetic()`
  parser handles "what is 12 plus 30".

### 2.2 A real desktop on sway, not a web page pretending to be one

**Problem.** The earlier desktop was one full-screen web page that drew fake
windows. Apps, the file explorer and the window manager did not feel like a real
OS, and real Linux apps could not be managed.

**Change.**
- **sway** is the window manager (`desktop/sway/config`). The Lua WM remains as
  the fallback web shell for machines without sway.
- **waybar** is the top bar, with OSAIMA status scripts (`osaima-status`,
  `osaima-sound`, `osaima-net`). It shows when a task is working or needs your OK.
- **mako** handles notifications, with a do-not-disturb mode. swaylock and
  swayidle (`osaima-lock`, `osaima-idle`) handle locking. The GTK theme is
  `desktop/share/themes/OSAIMA`. The foot terminal has its own config. There are
  three wallpapers (contours, dawn, deep).
- The **Ask bar** (`Super+Space`) is a small launcher window that sends a request
  to the assistant and shows examples when nothing has been learned yet.
- **One app per window.** `osaima-shell --app <name>` opens each OSAIMA app as its
  own Tauri window (`app.html?app=<name>`), so sway tiles, moves and closes them
  like any other app. `--desktop` keeps the old full-screen page.
  `OSAIMA_DEV_URL` makes the app load its pages live from the dev server.
- **Window control.** `desktop.rs` talks to sway IPC. It handles list, focus,
  close, launch, layout and workspace. Apps can be named in natural words ("archive
  manager" → `archives`, "office" → LibreOffice).
- **Productivity apps** are in the image: LibreOffice (Gentoo installs it as
  `libreoffice-bin`), mpv (it runs with `--vo=gpu,wlshm` so it also works with
  software rendering in a VM) and Xarchiver.
- Fixes found by looking at the booted ISO:
  - The top bar did not start on the ISO.
  - An xdg portal backend is now used instead of masking the portal.
  - Launch commands that contain commas are now quoted (sway was splitting them).
  - The terminal cursor colour warning is gone.
  - Fonts and details were polished so nothing looks unfinished.

### 2.3 Files: typed, undoable, confined

**Problem.** The AI handled files with shell commands (`find`, `mv`, `rm`). These
are hard to check, hard to undo and easy to get wrong.

**Change.** `files.rs` provides typed operations: `search` (by name, kind, size,
age, folder), `list`, `open`, `reveal`, `copy`, `move`, `rename`, `mkdir`, `trash`
and `undo`.
- They are confined to the home folder. A **protected list** is never touched:
  `.ssh`, `.gnupg`, `.config/osaima`, keyrings, browser profiles, `.aws` and others.
- Deleting always means moving to the trash, and every change can be undone.
- The model only sees real file names, from the folder contents it is given or
  from a search step. It is never allowed to guess names.
- Thunar has an **Ask OSAIMA** action (`desktop/thunar/uca.xml`). It passes the
  selected files to the assistant as context.

### 2.4 Multi-step plans, agent tasks and routines

- **Plans** (`orchestrate`). The assistant shows the whole plan, takes one
  approval (typed `confirm` if any step cannot be undone), runs the steps in order
  and stops on the first failure. A later step can use the files found by an
  earlier step (`"paths": {"from_step": N}`).
- **Agent tasks** (`task.rs`). Planning → you approve the scope → a tool loop
  (`run_command`, `write_file`, `read_file`, `list_dir`, `open_for_user`) that sees
  each result. The budget is 30 steps and 20 minutes. You can stop a task at any
  time.
- **Routines.** A finished plan can be saved by name and replayed. Every replay
  asks for approval again. If outside content (a web page, a file) influenced the
  plan, it cannot be saved.

### 2.5 Security: from the kernel up

**Problem.** "The AI is the shell" means the AI's mistakes, and any prompt
injection it falls for, become shell commands.

**Change.** Every command the AI runs now passes through two layers:
1. **bubblewrap** namespaces. The environment is empty (allowlist only).
   `/run/user` is hidden, so the session bus, the Wayland socket and the daemon's
   own socket are unreachable. Protected paths are masked.
2. **`osaima-jail`** (`src/bin/osaima-jail.rs`). It applies **Landlock**: the whole
   filesystem is read-only except the approved `--write` folders, and abstract
   sockets and signals are scoped (ABI v6). It adds a **seccomp-bpf** deny list,
   **no_new_privs** and **rlimits** on CPU, memory and processes.

On top of the two layers:
- An **injection guard** (`guard.rs`) fences outside text, plants a canary and
  marks any proposal influenced by outside content as tainted.
- Agent-task file writes (`write_scoped`) also run inside the sandbox, so a
  symlink cannot redirect them outside the approved folder.
- **Audit**: an append-only log in which each record carries the SHA-256 hash of
  the previous one.

The build scripts were also hardened. Chroot bind mounts are now made
`--make-rslave` in the same step, and the scripts refuse to unmount a shared
mount. An earlier version could unmount the host's own `/run/user` and crash the
desktop session.

### 2.6 System settings as a daemon service (`syssettings.rs`)

**Problem.** sway has no settings app, and the old Settings page only covered the AI.

**Change.** The daemon owns the system settings. The Settings app and the AI both
call the same code, with the same risk tiers:

| Setting | Values | Applied through |
|---|---|---|
| `wallpaper` | `contours`, `dawn`, `deep` or your own image path | sway `output * bg` |
| `resolution` | e.g. `1920x1080` (needs approval) | sway output |
| `scale` | e.g. `1.25` | sway output |
| `night_light` | on/off, temperature 2500–5500 K | gammastep (Wayland gamma) |
| `keyboard` | `us`, `gb`, `in`, `in:kan`, `in:tel`, … | sway input |
| `tap`, `natural_scroll` | on/off | sway input (touchpad) |
| `pointer_speed` | −1 … 1 | sway input |
| `repeat_rate` | keys per second and delay | sway input |
| `dnd` | on/off | mako mode |
| `lock_minutes` | 0 (never) … 60 | swayidle |
| `default_app` | browser, files, editor, … | xdg-mime |
| `sound_output` | a device | PipeWire `wpctl` |
| `timezone` | e.g. `Asia/Kolkata` (needs approval) | timedatectl |
| `network` | read only | NetworkManager |

Your choices are saved to `~/.config/osaima/` and re-applied at login by
`osaima-apply-prefs`.

### 2.7 Settings app and History app (new UI)

- **Settings** (`settings-app.js`) was rewritten with a left navigation and these
  sections: AI, Appearance, Display (with night light), Sound, Network, Keyboard &
  mouse, Notifications, Screen lock, Automations, Default apps, Date & time.
  - A display change that could leave the screen unreadable (resolution or scale)
    shows a 15-second **Keep / Revert** bar and reverts by itself.
  - Turning Wi-Fi off asks first, because you may lose your connection.
  - Each control says "Saved", or explains the error, right beside it.
- **History** (`history-app.js`) shows every audit record, newest first, 50 at a
  time, with filter chips (such as Files, Apps & windows and Settings) and text
  search. It is backed by the new `system.audit_query` RPC (`AuditLog::query`).
  The hash chain is checked with `system.audit_verify`.

### 2.8 Health watch and diagnosis (`health.rs`)

**Problem.** When something failed, the AI had no idea what was happening on the
machine. Nothing warned about overheating or a full disk.

**Change.** A background watch runs every 20 s and reads:
- CPU temperature (hwmon `k10temp` / `coretemp`), the CPU throttle count and the
  current/maximum frequency ratio
- pressure stall information (`/proc/pressure/{cpu,memory,io}`)
- memory and swap (`/proc/meminfo`) and free disk space (`statvfs`)
- battery state and level
- the busiest processes. CPU % is sampled twice over one second, so it shows
  current use rather than the lifetime average `ps` reports.
- failed systemd units

Each problem becomes an alert with a level, a plain-language title, detail and
advice, for example "CPU is throttling: close X or plug in the charger". The same
alert notifies at most once every 30 minutes. `system.health` returns the current
state.

**Diagnosis.** Requests like "why did my build fail" or "why is it slow" are
routed (`is_diagnosis`) to the LLM along with that evidence. The prompt has a
safe-fix rule: never suggest stopping OSAIMA's own services, deleting user data or
anything that needs root. An earlier version once suggested `killall mcp-daemon`.

### 2.9 Automations (`automations.rs`)

**Problem.** Requests like "shut down after this finishes" or "keep brightness at
40% on weekdays 9 pm–6 am" are not one action. They are rules that must survive
reboots, run at the right time, undo themselves when a time window ends and tell
you what they did.

**Change.** A rule engine inside the daemon.

**Time triggers**

| Trigger | Meaning | Example |
|---|---|---|
| `at` | once, at a local date and time (also "in 10 minutes") | "at 11:30 tonight lock the screen" |
| `daily` | at a time, on chosen days (every day, weekdays, weekends, mon…sun) | "every Friday at 6 pm move report.pdf to Documents" |
| `window` | holds a setting from one time to another, then **puts back the old value** (may cross midnight) | "night light on weekdays 8 pm–7 am" |
| `every` | repeats every N minutes (5 min … 24 h) | "remind me to drink water every hour" |

**Event triggers** (checked every 20 s; they fire when the event *happens*, not again while it stays true)

| Trigger | Meaning | Example |
|---|---|---|
| `after_task` | the running OSAIMA agent task finishes | "shut down after this is completed" |
| `process_exit` | any running program finishes (a download, a compile, an export) | "when wget finishes, lock the screen" |
| `on_login` | the next start or login (or every one) | "after reboot set brightness to 70" |
| `battery` | the battery drops below / rises above a level | "when the battery drops below 20%, dim the screen" |
| `power` | the charger is plugged in or unplugged | "when I unplug the charger, turn off Bluetooth" |
| `network` | the internet connects or disconnects, optionally a named Wi-Fi | "when I connect to wifi, open the browser" |
| `app_open` / `app_close` | an app's window opens or closes | "when I open the video player, turn on Do Not Disturb" |
| `health` | the health watch raises a warning (heat, CPU, memory, disk, battery) | "if the computer gets too hot, tell me" |

- **While it lasts (`hold`).** "*While* I'm on battery keep brightness at 50%" or
  "Do Not Disturb *until* I close the player" applies the change while the event
  holds, then **puts the old value back**, just like a time window. The daemon
  detects "while / as long as / until" in your words itself, so it doesn't rely
  on the model to notice.
- **Conditions (`only_if`).** Any rule can be limited to times when you are on
  battery or plugged in, online or offline, a given app is open, between two
  times, or on certain days. Example: "every hour remind me to stand up, but only
  between 9 and 6 on weekdays".
- **Putting things back** works for volume, brightness, Wi-Fi, Bluetooth and every
  desktop setting (night light, Do Not Disturb, wallpaper, keyboard layout, lock
  time…). The value is read just before the change and restored afterwards.
- **Allowed actions**: volume, brightness, music (play, pause, next), lock
  screen, power (shut down, restart, sleep), Wi-Fi, Bluetooth, notifications,
  every desktop setting, opening and closing apps, workspaces, layouts, opening a
  web page, and moving, copying or renaming files and making folders. Several
  actions can run in one rule. An automation can never run shell commands or
  delete files.
- **Resilient to a slow AI service.** Creating an automation needs the model.
  When the main Gemini model is slow or busy, the daemon also asks the next model
  after 5 s and takes the first good answer.
- **Approval**: creating a rule needs your OK. A rule with an action that cannot
  be undone (shutdown, restart) also needs the typed `confirm`.
- **Power actions** get a **60-second countdown** with a notification, and you can
  cancel it (Settings → Automations, or `system.automation_cancel`).
- Every rule run sends a notification and is written to the audit log, so it
  appears in History.
- Rules are kept in `~/.config/osaima/automations.json`. The engine checks every
  20 s.
- **Scheduled requests never run immediately.** `is_scheduled()` detects time
  words ("at 11", "tonight", "every", "weekdays", "after reboot", "in 10
  minutes"…) and sends the request to the model instead of the fast path. Without
  this, "at 11:30 lock the screen" would have locked the screen at once.
- The model is told the current local time, so "tonight" becomes a real date.
- The assistant shows the rule in plain words ("Weekdays 21:00–06:00: set
  brightness to 40%; back to before at the end") before you allow it.
  **Settings → Automations** lists the rules, shows whether each one is waiting,
  on now or finished, and lets you remove them.

### 2.10 Prediction, health in the side rail, and the right model for the hardware

- **Predictions** (`behavior.rs: predictions`). A command qualifies when you ran
  it on at least 3 different days of the last 28, within 45 minutes of the current
  time of day, and not in the last 2 hours. Only fast-path commands count, never
  power actions. The side rail shows "Usually around HH:MM" with **Do it now** and
  **Do this every day at HH:MM**. The second becomes an automation, which still
  asks for approval.
- **Health alerts in the side rail.** The "This computer" panel shows the current
  warnings, such as heat, throttling, memory, disk, battery or failed services,
  each with advice and a **Why is this happening?** button that starts a diagnosis.
- **Automations in Activity and History**, in plain words, with an Automations
  filter. Failed runs also appear under Problems.
- **GPU detection and model download** (`models.rs`, RPCs `system.gpu_info`,
  `model_download`, `model_status`, `model_cancel`):
  - Each graphics card is identified by its PCI address. On a laptop with both
    AMD integrated graphics and an NVIDIA card, the two are reported separately
    and correctly.
  - If NVIDIA's own driver isn't loaded (nouveau), memory is estimated from the
    model name, e.g. RTX 4060 → 8 GB, and marked as an estimate.
  - Integrated graphics are ignored for the recommendation.
  - ≥ 7.5 GB of video memory → Qwen2.5-7B-Instruct (Q4_K_M, about 4.4 GB).
    ≥ 5 GB → Phi-4-mini, if it isn't already installed.
  - A download needs your approval and checks free disk space first. It resumes
    an interrupted download and checks the GGUF header before use. The model is
    saved to `~/.local/share/osaima/models/` and recorded in `llm.env`.
  - `start-llm.sh` uses the downloaded model and passes `-ngl 99` (GPU offload)
    only when the installed llama.cpp build lists a GPU device.
- **Night light** uses gammastep (built with its Wayland backend), which is now in the image.

### 2.11 Fixes found by testing like a real user

- A double-click sent a request twice. There is now a 1.5 s duplicate guard.
- With the AI core down, the web bridge pretended to act. It now says so plainly.
- "what is 12 plus 30" showed keyboard shortcuts, because the widget layer was
  missing in standalone windows and an offline note hid the error. Both are fixed.
- Timers, reminders and the calculator now work in the assistant window.
- Small talk now gets a real answer instead of "can't reach the AI core".
- `find -exec` is judged by the command it runs, not by `find`.

### 2.12 Testing without rebuilding the ISO

- `desktop/dev/run-nested.sh` runs the whole OSAIMA desktop nested in a window on
  your own Wayland session. It starts the router, the daemon and the shell server.
- Pages load live from the dev server, so a UI change needs only a reload.
- `run_vm.sh` (QEMU/KVM, SSH on port 2222) and a VM smoke test check the real
  desktop, the kernel jail, the isolation and window control inside the built image.
- `ai-core/evaluation/run_eval.py` regenerates `RESULTS.md`.

### 2.13 Review materials

`docs/review/DEMO.md` is the demo runbook. It includes the Windows laptop +
VirtualBox setup (Option A2). `CLAIMS.md` lists every claim with its evidence and
limit, and `QA.md` lists likely examiner questions.

---

## 3. Architecture in layers

| Layer | What | Where |
|---|---|---|
| 1. UI and desktop | sway session, waybar, mako, swaylock, Thunar "Ask OSAIMA"; OSAIMA apps (assistant, settings, history, notes, calendar, monitor…) as Tauri windows | `desktop/`, `ui/osaima-shell/` |
| 2. NLU and intent router | MiniLM encoder + classifier head (ONNX), slot extraction, a confidence cascade (fast path or LLM), scheduling and diagnosis detection | `ai-core/intent-router/`, `mcp-daemon/src/nlu.rs`, `src/main.rs` |
| 3. AI core / LLM | Gemini (cloud) or llama.cpp (on-device); task-specific prompts (classify, write, propose one command); RAG over documents | `mcp-daemon/src/llm.rs`, `src/prompts/`, `ai-core/rag-engine/`, `ai-core/llm/` |
| 4. Execution and security | risk tiers, injection guard, typed file/desktop/settings actions, agent tasks, automations, bubblewrap + osaima-jail | `mcp-daemon/src/{security,guard,executor,files,desktop,os_control,syssettings,task,automations}.rs`, `src/bin/osaima-jail.rs` |
| 5. Kernel and system | Linux kernel (Landlock, seccomp, namespaces), D-Bus (MPRIS, logind, NetworkManager, BlueZ, notifications), sway IPC, PipeWire, systemd user services, health watch | `kernel/`, `gentoo/`, `build-tools/`, `mcp-daemon/src/health.rs` |
| 6. Feedback and learning | SQLite behaviour store: request log, suggestions, preferences, routines | `mcp-daemon/src/behavior.rs`, `ai-core/behavior/` |
| 7. Audit | SHA-256 hash-chained activity log; read by History, verified in Settings | `mcp-daemon/src/security.rs` |

Diagram and request flows: [ARCHITECTURE.md](ARCHITECTURE.md).

---

## 4. How a request flows

```
 you type ─► Assistant / Ask bar (Tauri window)
                 │  bridge.js → Tauri command → JSON-RPC over $XDG_RUNTIME_DIR/osaima/mcp.sock
                 ▼
            mcp-daemon  ── greeting / arithmetic? ── answered at once
                 │
                 ├─ diagnosis ("why did… fail")? ── health evidence + LLM ── answer with safe fixes
                 │
                 ├─ intent router (≈40 ms) ── confident, not scheduled, not a file/job request?
                 │        └── fast path: intent + params
                 │
                 └─ LLM (Gemini or local) with router hints, preferences, current time,
                    fenced outside content and RAG passages
                          └── intent + params (os_action, files, orchestrate, task, automation, …)
                 ▼
            Assistant shows the card: what will happen + risk tier
                 │  approval (typed "confirm" when it can't be undone)
                 ▼
            daemon acts: sway IPC · D-Bus · files.rs · syssettings · sandboxed command
                 │                                 (bubblewrap → osaima-jail → program)
                 ▼
            result card · notification · audit record (→ History) · behaviour store
```

Automations take the same route at creation time. When they run later, the
automations engine calls the same `os_action` code, sends a notification and
writes to the audit log.

---

## 5. Security model

| Threat | Defence |
|---|---|
| The model proposes something destructive | Risk tiers. Irreversible actions need the typed `confirm`. Files go to the trash and can be undone. |
| Prompt injection from a page, file or message | Outside text is fenced and canaried. Proposals it influenced are marked tainted and shown as such, and they cannot be saved as routines. |
| An AI-run command reads secrets or talks to the session | Empty environment; `/run/user` hidden (no session bus, Wayland or daemon socket); protected paths masked; Landlock scoping of abstract sockets and signals. |
| An AI-run command writes outside its scope | Landlock makes everything read-only except the approved folders. Writes go through the sandbox, so symlinks cannot redirect them. |
| Privilege escalation | `no_new_privs`, a seccomp deny list, no root, rlimits. |
| An automation does something harmful later | Only typed OS actions are allowed (no commands). Creation needs approval, plus `confirm` for power actions. Power actions have a 60 s cancellable countdown. |
| Someone hides what the AI did | Hash-chained audit log, checked by `system.audit_verify`; History shows every record. |
| API keys leak | The key is only in `~/.config/osaima/llm.env` (0600), never in the repo or ISO. AI-run commands cannot read `.config/osaima`. |

Measured: kernel enforcement 6/6, sandbox isolation 5/5, red-team 1/50 by the
strict metric (the one success was a harmless `lsblk`).

---

## 6. Feature reference

### Keyboard (Mod = Super; Alt when nested)

| Keys | Action |
|---|---|
| `Mod+Space` | Ask bar |
| `Mod+a` | Assistant |
| `Mod+,` | Settings |
| `Mod+Return` | Terminal (foot) |
| `Mod+e` | Files (Thunar) |
| `Mod+b` | Browser (Firefox) |
| `Mod+l` | Lock the screen |
| `Mod+q` | Close the window |
| `Mod+f` | Full screen |
| `Mod+Shift+Space` | Float / tile |
| `Mod+t` / `Mod+w` | Split / tabbed layout |
| `Mod+arrows`, `Mod+Shift+arrows` | Focus / move windows |
| `Mod+1..5`, `Mod+Shift+1..5` | Go to / move to workspace |

### Things to try

| You say | What happens |
|---|---|
| set brightness to 60% | fast path, done at once |
| open the archive manager | launches Xarchiver |
| find PDFs bigger than 10 MB in Downloads | file card with the results |
| make a Reports folder in Documents and move both report PDFs into it | a plan with one approval and one undo |
| set up a Python project in ~/projects/demo with a venv and a test, and run it | an agent task with a kernel-enforced scope |
| turn on night light | warmer screen colours |
| change the wallpaper to dawn | wallpaper changes |
| keep my brightness at 40% on weekdays from 9pm to 6am | an automation, which puts the old value back at 6 am |
| shut down my pc after this task is completed | an automation, typed `confirm`, 60 s cancellable warning |
| after reboot set my brightness to 70 | runs at the next login |
| why is my laptop slow? | an answer based on health evidence |
| what is 12 plus 30 | 42, with no model involved |

---

## 7. Daemon RPC reference

JSON-RPC 2.0 over the Unix socket `$XDG_RUNTIME_DIR/osaima/mcp.sock`, one JSON
object per line. (The name "mcp-daemon" is historical. The protocol is plain
JSON-RPC, not the Model Context Protocol.)

| Method | Purpose |
|---|---|
| `system.classify_intent` | Understand a request (fast path or LLM) |
| `system.dispatch_intent` | Run a classified intent |
| `system.os_action` | One typed action: volume, brightness, media, power, wifi, bluetooth, lock_screen, notify, settings, files, window, app, layout, workspace, browser (`dry_run`, `approved`, `typed`) |
| `system.assess_command`, `system.execute_command` | Risk-tier a shell command; run it in the sandbox |
| `system.task_start`, `task_status`, `task_decide`, `task_cancel`, `tasks` | Agent tasks |
| `system.automations`, `automation_add`, `automation_remove`, `automation_cancel` | Automations (`automation_add` takes `rule`, `approved`, `typed`, `dry_run`) |
| `system.health` | Current health readings and alerts |
| `system.routines`, `routine_save`, `routine_delete`, `routine_note` | Routines |
| `system.audit_tail`, `audit_query`, `audit_verify` | History and chain verification |
| `system.suggestions`, `get_suggestion` | Behaviour-store suggestions |
| `system.llm_status`, `set_llm_mode`, `set_api_key`, `validate_api_key` | AI mode and key |
| `system.ask`, `share`, `shared_take` | Ask bar and "Ask OSAIMA" hand-offs to the assistant |
| `system.get_stats` | Live CPU, memory and disk figures |
| `system.gpu_info`, `model_download`, `model_status`, `model_cancel` | Graphics cards, recommended local model, download with progress |

Quick test from a shell:
```bash
printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"system.health","params":{}}' \
  | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/osaima/mcp.sock
```

---

## 8. Repository layout

| Path | Contents |
|---|---|
| `ai-core/mcp-daemon/` | The Rust daemon and the `osaima-jail` helper. Key modules: `main.rs` (RPC + routing), `llm.rs`, `nlu.rs`, `security.rs`, `guard.rs`, `executor.rs`, `files.rs`, `desktop.rs`, `os_control.rs`, `syssettings.rs`, `health.rs`, `task.rs`, `automations.rs`, `models.rs`, `behavior.rs`, `src/prompts/` |
| `ai-core/intent-router/` | Dataset generation, training, ONNX export, router service, slot extraction |
| `ai-core/rag-engine/`, `ai-core/behavior/` | Knowledge indexing and retrieval; behaviour-store schema |
| `ai-core/security/` | Red-team prompts, runner and results; AppArmor profile (stage 1) |
| `ai-core/evaluation/` | `run_eval.py` and **`RESULTS.md`** |
| `ai-core/llm/start-llm.sh` | Starts the on-device llama.cpp server |
| `ai-core/asr/` | Speech benchmark and wake-word tooling (not in the shipped interface) |
| `desktop/` | sway, waybar, mako, foot, GTK theme, fonts, wallpapers, Thunar action, `bin/` helpers (`osaima-app`, `osaima-open`, `osaima-ask`, `osaima-lock`, `osaima-idle`, `osaima-apply-prefs`, status scripts), `dev/run-nested.sh` |
| `ui/osaima-shell/frontend/` | The OSAIMA apps: `app.html` + `src/app-main.js` (one app per window), `src/wm/*-app.js` (assistant, settings, history, launcher, …), `bridge.js` |
| `ui/osaima-shell/src-tauri/` | Tauri host (`--app <name>`, `--desktop`) and its commands |
| `ui/osaima-shell/server.py` | Web bridge for development in a browser |
| `gentoo/overlay/` | Interstellar profiles, branding, `osaima-ai-core` and `osaima-shell` ebuilds |
| `build-tools/scripts/` | `build_vm_image.sh`, `build_iso.sh`, `run_vm.sh` |
| `kernel/` | Linux kernel source (submodule) |
| `docs/review/` | `DEMO.md`, `CLAIMS.md`, `QA.md` |

The distribution keeps Gentoo as its upstream and maintains only its own overlay
(`ID_LIKE=gentoo`). Packages come from the Gentoo binary host. See
[BUILD-DISTRO.md](BUILD-DISTRO.md) for the source-build path.

---

## 9. Run it for development

You do not need an ISO. On a Linux machine with a Wayland session:

```bash
# one-time builds
(cd ai-core/mcp-daemon && cargo build --release)          # mcp-daemon + osaima-jail
(cd ui/osaima-shell/src-tauri && cargo build --release)   # osaima-shell

# run the whole desktop in a window on your own session
desktop/dev/run-nested.sh            # pages load live from the dev server (edit, reload)
desktop/dev/run-nested.sh --built    # use the pages built into the binary
```

`run-nested.sh` starts the intent router, the daemon and the shell server if they
are not already running, then runs sway nested in a 1600x900 window. Inside it the
modifier key is **Alt**. Outside, your own desktop keeps Super. Logs are in
`$XDG_RUNTIME_DIR/osaima-dev/`.

Needs: sway, waybar, mako, foot, thunar, mousepad, firefox, bubblewrap, gammastep (or wlsunset),
poppler-utils (for PDF text) and Python 3 for the router. Private mode also needs
a llama.cpp `llama-server` build and a `.gguf` model; start it with
`ai-core/llm/start-llm.sh`.

**Choose how it thinks.** Open Settings (`Mod+,`) → AI. Pick **Fast** (Gemini) and
paste a Gemini API key (free keys are at aistudio.google.com), or pick
**Private**. The key is saved to `~/.config/osaima/llm.env` (mode 0600), never to
the repo.

> Caution when testing on your own machine: actions are real. Brightness, Wi-Fi
> and power automations act on the host. Use the nested session or a VM for power
> and network tests.

The older all-in-one web shell with the Lua-driven tiling window manager still
works on machines without sway. See [ui/osaima-shell/README.md](ui/osaima-shell/README.md).

---

## 10. Build and boot the ISO

You need a Linux host with root, KVM and the tools listed in the script headers
(squashfs-tools, xorriso, mtools, GRUB tools):

```bash
(cd ai-core/mcp-daemon && cargo build --release)
(cd ui/osaima-shell/src-tauri && cargo build --release)
sudo SKIP_DISK=1 build-tools/scripts/build_vm_image.sh  # Gentoo rootfs from binary packages + this tree
sudo build-tools/scripts/build_iso.sh                   # live ISO (squashfs + dracut, BIOS and UEFI)
ISO=1 build-tools/scripts/run_vm.sh                     # boot out/interstellar-os.iso in QEMU/KVM
```

- `build_vm_image.sh` also needs a `llama-server` build and a `.gguf` model in
  `~/.local/share/osaima/` (see the script header). Leave out `SKIP_DISK=1` to
  also build the qcow2 disk image.
- The output is `out/interstellar-os.iso`, about 4.6 GB. Copy it to an exFAT or
  NTFS USB drive, because FAT32 cannot hold files over 4 GB.
- `run_vm.sh` options: `RES=1920x1080`, `MEM=8G`, `HEADLESS=1`. Port 8778 on the
  host reaches the shell server in the guest, and SSH is on port 2222.
- **VirtualBox on Windows**: see [docs/review/DEMO.md](docs/review/DEMO.md),
  Option A2 (EFI on, 4+ GB RAM, VMSVGA graphics, boot the ISO).

---

## 11. Testing

| What | How |
|---|---|
| Daemon unit tests (35) | `cd ai-core/mcp-daemon && cargo test --release`. Covers automation days/windows and validation, scheduled-request detection, the audit query, desktop actions, the risk tiers and more. |
| Evaluation suite | `python3 ai-core/evaluation/run_eval.py` → `RESULTS.md` (intent accuracy, latency, 25 end-to-end tasks, red-team, kernel enforcement, isolation, agent tasks) |
| UI | `desktop/dev/run-nested.sh`, then edit and reload |
| Whole image | `ISO=1 HEADLESS=1 build-tools/scripts/run_vm.sh`, then SSH on 2222 and the VM smoke test |
| Red team | `ai-core/security/` runner, 50 injection cases |

Checked live against the daemon on 2026-10-21:
- "keep my brightness at 40% on weekdays from 9pm to 6am" became a window rule
  from 21:00 to 06:00 on weekdays that restores the old value.
- "after reboot set my brightness to 70" became an `on_login` rule.
- "at 11:30 tonight lock the screen" became a one-time rule for 23:30 today.
- "shut down my pc after this task is completed" became an `after_task` rule that
  needs the typed `confirm`.
- A rule without approval was refused. A rule that runs a shell command was refused.
- A test notification rule fired on time and was marked finished.

---

## 12. Results

Measured on one laptop (Ryzen 5 3550H, 9.6 GB RAM) with Gemini 3.5 Flash-Lite:

| Metric | PDR target | Result |
|---|---|---|
| Intent accuracy, fast path (held-out) | > 92% | 91.3% on the 64% of requests it answers (labels unreviewed; most errors found were label problems) |
| Fast-path latency | < 200 ms | p50 51.9 ms, p95 70.7 ms |
| Workflow success | > 80% | 25/25 |
| Prompt-injection resistance | 0 successful | 0/50 |
| LLM workflow latency | < 8 s | median 2.6 s |
| Kernel enforcement | — | 6/6 |
| Sandbox isolation | — | 5/5 |
| Agent tasks | — | 3/3 |

Details: [ai-core/evaluation/RESULTS.md](ai-core/evaluation/RESULTS.md) and
[docs/review/CLAIMS.md](docs/review/CLAIMS.md).

---

## 13. Known limits and work not finished

- **Not yet run in the booted ISO:** the new Settings sections, the History app
  and night light have passed build and syntax checks, but they have not been used
  in the real sway session. The ISO needs to be rebuilt from this branch and
  tested in a VM.
- Automations are checked every 20 s, so a rule can run up to 20 s late. An `at`
  rule whose time passed while the computer was off runs at the next start.
- The llama.cpp engine in the image is a CPU build. On a GPU laptop the bigger
  model downloads and is used, but it runs on the processor until a GPU-enabled
  llama.cpp (CUDA or Vulkan) is installed. Settings says this plainly.
  Downloading a multi-GB model has not been tried end to end. The resume and
  header checks are tested against a local server.
- A downloaded model takes effect the next time Private mode starts the local
  engine.
- Voice input is not part of the shipped interface.
- The daemon itself is not confined. Only the commands it runs are.
- Private (local) mode is slow on a CPU. It is meant for privacy and offline use.

---

## 14. Commit history of this branch

Newest first (`git log --oneline master..review-stable`):

| Commit | Change |
|---|---|
| `44bae24` | GPU detection by PCI address; NVIDIA memory estimate; friendly names |
| `edad259` | GPU detection and model download with progress in Settings |
| `e676d39` | History: automation entries and filter |
| `784de94` | Predictions and health alerts in the side rail; wlsunset in the image |
| `3a54a64` | Detailed README |
| `a4d3196` | Scheduled requests reach the model; Automations page and night light in Settings; plain-language rule summaries |
| `6b47397` | Automations: scheduled, windowed and after-task OS actions with approval and notifications |
| `dbffcde` | Redesigned Settings app; new History app; `audit_query` |
| `cf5741a` | System settings, health watch and failure diagnosis in the daemon |
| `9b0defd` | Desktop polish found by looking at the ISO |
| `3b57ee7` | Natural app names; commands with commas launch |
| `8c508df` | LibreOffice is `libreoffice-bin` on Gentoo |
| `64260dd` | LibreOffice, mpv and Xarchiver in the image; the assistant opens them |
| `6e34d77` | Timers, reminders and calculator work in the assistant window |
| `18123e7` | Fixes from real-user testing (double clicks, outages, maths) |
| `77750ae` | Terminal config fix |
| `5256d10` | Top bar starts on the ISO; portal backend |
| `2d89962` | One look across the OS: Ask bar, notifications, lock screen |
| `7b563fc` | Windows laptop + VirtualBox demo setup |
| `7ac331e` | Review materials updated |
| `7eb6817` | VM smoke test: desktop, kernel jail, isolation, window control |
| `f0bfc51` | Top bar shows task state |
| `785e29e` | Agent tasks and isolation in the evaluation |
| `1fe3e5a` | Routine fixes |
| `4aebe0a` | Security: AI-run commands can't reach the session, secrets or the daemon |
| `3125ae9` | Agent tasks inside a kernel-enforced scope |
| `e320a60` | Routines |
| `a6a011f` | Review materials; README and ARCHITECTURE updated |
| `b089b01` | Build: chroot mounts can never unmount the host's |
| `0df11b3` | Image ships the real desktop; eval covers files, plans and the kernel |
| `8d17f60` | "Ask OSAIMA" from the file manager |
| `7006cfa` | Multi-step plans with one approval |
| `54e1fdd` | Typed, undoable file actions |
| `7e8e892` | Kernel-enforced limits (osaima-jail) |
| `4efa4f6` | A real desktop on sway |
| `bf4dc54` | Conversation answers instead of "can't reach the AI core" |
| `62c3464` | `find -exec` judged by its command; backend-aware eval |
| `8172b97` | The assistant is the home screen |
| `cb1aaae` | Real data in the shell (audit, AI mode, key, live stats) |
| `8864406` | Cloud-first Gemini; local model as fallback |
| `4d944ea` | Desktop doesn't starve the on-device LLM |
| `980d851`, `f94fb3b` | VM output and resolution fixes |
| `1497b4a` | Bootable live ISO |
| `0656c91` | Task-specialised LLM, RAG, behaviour store, evaluation, ISO builder |
| `507858e` | Local AI core, intent router, security gate, OS controls, VM image |
| `71fd6b0` | RAG engine and behaviour store in the UI |
| `0450bc8` | Lua-driven tiling window manager and shell apps |

---

## 15. Credits

- **Pragna S Bharadwaj**: the Lua-driven tiling window manager and interpreter,
  and the original shell apps.
- **Akash**: Daemon & Process Architecture, CPU Management, Initial Technical Blueprint & Repository Structure.
- **Sunil J Rathore**: AI core, security, desktop, evaluation and the distribution.
- **Banu B B**: team member (add contributions here).
- **Dr. Madhu BR**: project guide.

Upstream projects: Gentoo, Linux (Landlock, seccomp), sway, waybar, mako,
bubblewrap, Tauri, llama.cpp, ONNX Runtime, sentence-transformers (MiniLM),
wlsunset, PipeWire, NetworkManager and the many packages in the image.
