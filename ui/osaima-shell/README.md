# osaima-shell — Interstellar OS Agentic Shell

The desktop shell for Interstellar OS: an ambient, AI-first environment with a
**Lua-driven tiling window manager**, an AI command hub, and live system context
from the MCP daemon.

## What's here

```
src/
  main.js              Shell bootstrap: starfield, clock, launcher, MCP polling, WM boot, dock
  style.css            Design system + window-manager / dock / app styles
  wm/
    lua-vm.js          Embedded Lua 5.x-subset interpreter (lexer → parser → evaluator)
    window-manager.js  WM engine: tile / monocle / float layouts, workspaces, keybindings
    api.js             The `wm.*` API surface exposed to the Lua config
    apps.js            Built-in apps: AI Assistant, Terminal, Monitor, Task Manager, Files, Config, About
    agent-core.js      Proactive "AI Core": watches the system + learned behavior, suggests actions
    rag.js             RAG engine: TF-IDF retrieval over a seeded knowledge base (PDR §3.5)
    behavior.js        Behavior-learning store: learns app/layout/query habits (PDR §3.7)
    boot.js            Wires engine + Lua VM + apps + config together; handles live reload
    default-config.js  Embedded fallback copy of wm.lua (used when fetch() is unavailable)
  config/
    wm.lua             The real, user-editable window-manager configuration (Lua)
src-tauri/             Tauri (Rust) host: MCP socket IPC + system stats commands
```

## The Lua window manager

The window manager's entire behaviour — layouts, gaps, master ratio, workspaces,
keybindings and window rules — is defined in [`src/config/wm.lua`](src/config/wm.lua),
a **real Lua file** executed by a from-scratch Lua interpreter (`src/wm/lua-vm.js`).
No WASM runtime or bundler required: it runs in plain ES modules, even over `file://`.

**Layouts:** `tile` (master/stack), `monocle` (fullscreen stack), `grid` (even
grid), `spiral` (fibonacci split), `float` (free). Cycle with `Alt+Tab`.

**Built-in apps:** AI Assistant (an agent that drives the WM from natural
language), Terminal (wired to live MCP stats), System Monitor, Task Manager
(ML-prioritized process view), Files, live Lua Config editor, and About.

**Two sides of "agentic":**
- *Reactive* — the **AI Assistant** app does what you ask in plain English.
- *Proactive* — the **AI Core** (`agent-core.js`) watches CPU/memory and the
  window layout in the background and surfaces occasional actionable suggestions
  ("CPU is high — open Task Manager?"), one click away.

**Mouse edge-snapping:** drag a window's titlebar to a screen edge or corner to
snap it to a half or quarter of the screen (a live preview shows the target).

**RAG knowledge base (`Alt+r`):** ask a question; a dependency-free TF-IDF
retriever finds the most relevant passages from an indexed corpus and returns an
extractive answer with cited sources. Mirrors the PDR RAG subsystem (ingest →
chunk → embed → retrieve → re-rank → answer); the `embed()` step is isolated so a
real MiniLM model can replace TF-IDF later. The AI Assistant falls back to RAG for
knowledge questions it can't action.

**Behavior learning (`Alt+b`):** the shell records app launches, layout choices,
workspace use, assistant queries and suggestion-acceptance to an on-device profile
(localStorage), then learns your most-used apps, preferred layout and likely next
app — which feeds the proactive AI Core's suggestions. Mirrors the PDR behavior
store; persists across sessions.

Because it's a genuine Lua environment, the config can compute values and loop —
e.g. generating one keybinding per workspace:

```lua
for i = 1, 5 do
  wm.bind(mod .. "+" .. i, "workspace:" .. i)
  wm.bind(mod .. "+Shift+" .. i, "move_to_workspace:" .. i)
end
```

### `wm` API reference (used from wm.lua)

| Call | Effect |
|------|--------|
| `wm.modkey("Alt")` | Set the modifier key for chords |
| `wm.layout("tile"\|"monocle"\|"grid"\|"spiral"\|"float")` | Initial layout |
| `wm.gaps(px)` | Gap between tiled windows |
| `wm.master_ratio(0..1)` | Master column width fraction |
| `wm.master_count(n)` | Windows in the master area |
| `wm.border(width, focused, unfocused)` | Border styling |
| `wm.workspaces({ "a", "b", ... })` | Define workspaces/tags |
| `wm.bind("Mod+Return", action)` | Bind a key chord to an action string or Lua function |
| `wm.rule({ match=, floating=, workspace= })` | Per-app window rules |
| `wm.autostart("terminal")` | Spawn apps at boot |
| `wm.notify(message, "ok"\|"info"\|"err")` | Show a desktop notification |

Actions include: `focus_next`, `focus_prev`, `close_window`, `toggle_floating`,
`set_layout:<name>`, `cycle_layout`, `increase_master` / `decrease_master`,
`spawn:<app>`, `workspace:<n>`, `move_to_workspace:<n>`, `open_launcher`.

### Default keybindings (Alt = mod)

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| `Alt+Return` | Terminal | `Alt+j` / `Alt+k` | Focus next / prev |
| `Alt+a` | AI Assistant | `Alt+q` | Close window |
| `Alt+e` | Files | `Alt+Space` | Toggle floating |
| `Alt+p` | System Monitor | `Alt+t/m/f` | tile / monocle / float |
| `Alt+z` | Task Manager | `Alt+Tab` | Cycle layout |
| `Alt+c` | Config editor | `Alt+l` / `Alt+h` | Grow / shrink master |
| `Alt+1..5` | Switch workspace | `Alt+Shift+1..5` | Move window to workspace |

### Live reload & persistence

Open the **wm.lua — Config** app (`Alt+c`) and edit the Lua:

- **Reload** (`Ctrl+Enter`) — re-executes and applies instantly; open windows are
  preserved. Syntax/runtime errors are reported inline without crashing the shell.
- **Save** — applies *and* persists to `localStorage`, so your tweaks survive a
  restart (the saved config takes precedence over the shipped `wm.lua`).
- **Reset** — discards saved edits and restores the shipped config.

## Running

**In the real OS (Tauri host, provides live MCP data):**

```bash
npm install
npm run dev        # tauri dev
```

**Quick UI-only demo (no Rust toolchain — uses mock system data):**

```bash
python -m http.server 8777      # from this directory
# open http://127.0.0.1:8777/
```

When the Tauri backend / MCP daemon isn't present, the shell automatically falls
back to mock system stats so the UI is always demoable.

You can also drive the WM from the browser console: `window.WM` (the engine) and
`window.reloadWM(luaSource)` are exposed for debugging.
