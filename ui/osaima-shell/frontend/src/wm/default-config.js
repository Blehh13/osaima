/**
 * default-config.js — fallback copy of config/wm.lua embedded as a string.
 *
 * The canonical, human-edited config lives in `src/config/wm.lua`. At boot the
 * shell tries to `fetch()` that file; when it can't (e.g. opened over file://
 * where fetch is blocked), it falls back to this embedded copy so the WM always
 * boots. Keep this in sync with wm.lua — they should be identical.
 */

export const DEFAULT_WM_LUA = `-- ═══════════════════════════════════════════════════════════════════════
--  Interstellar OS — Window Manager configuration
--  This is a REAL Lua file, executed by the shell's embedded Lua interpreter.
--  Edit it live from the "wm.lua — Config" app and press Reload (or Ctrl+Enter).
-- ═══════════════════════════════════════════════════════════════════════

-- Modifier key used for all window-manager chords.
-- Use "Alt" for the in-browser demo (the OS eats the Super/Windows key).
local mod = "Alt"
wm.modkey(mod)

-- ── Appearance ──────────────────────────────────────────────────────────
wm.layout("tile")          -- "tile" | "monocle" | "grid" | "spiral" | "float"
wm.gaps(12)                -- pixels between tiled windows
wm.master_ratio(0.58)      -- master column takes 58% of the width
wm.master_count(1)         -- windows in the master area
wm.border(2, "#78b4ff", "rgba(100,160,255,0.12)")

-- ── Workspaces (tags) ───────────────────────────────────────────────────
wm.workspaces({ "main", "code", "web", "chat", "sys" })

-- ── Keybindings ─────────────────────────────────────────────────────────
-- Window focus & management
wm.bind(mod .. "+j",        "focus_next")
wm.bind(mod .. "+k",        "focus_prev")
wm.bind(mod .. "+q",        "close_window")
wm.bind(mod .. "+Space",    "toggle_floating")

-- Layout control
wm.bind(mod .. "+t",        "set_layout:tile")
wm.bind(mod .. "+m",        "set_layout:monocle")
wm.bind(mod .. "+f",        "set_layout:float")
wm.bind(mod .. "+Tab",      "cycle_layout")
wm.bind(mod .. "+l",        "increase_master")
wm.bind(mod .. "+h",        "decrease_master")

-- Launch applications
wm.bind(mod .. "+Return",   "spawn:terminal")
wm.bind(mod .. "+a",        "spawn:assistant")
wm.bind(mod .. "+e",        "spawn:files")
wm.bind(mod .. "+p",        "spawn:monitor")
wm.bind(mod .. "+z",        "spawn:taskmanager")
wm.bind(mod .. "+r",        "spawn:knowledge")
wm.bind(mod .. "+b",        "spawn:behavior")
wm.bind(mod .. "+c",        "spawn:config")
wm.bind(mod .. "+slash",    "open_launcher")

-- Generate one workspace-switch binding per workspace, in a loop —
-- this is why a real Lua config beats a static table.
for i = 1, 5 do
  wm.bind(mod .. "+" .. i, "workspace:" .. i)
  wm.bind(mod .. "+Shift+" .. i, "move_to_workspace:" .. i)
end

-- An inline Lua action function: greet from the terminal.
wm.bind(mod .. "+g", function()
  local w = wm.spawn("terminal")
  wm.log("spawned a greeter terminal")
end)

-- ── Window rules ────────────────────────────────────────────────────────
-- Float utility windows; pin the monitor to the "sys" workspace.
wm.rule({ match = "config", floating = true })
wm.rule({ match = "about",  floating = true })
wm.rule({ match = "assistant", floating = true })
wm.rule({ match = "knowledge", floating = true })
wm.rule({ match = "behavior", floating = true })
wm.rule({ match = "monitor", workspace = 5 })

-- ── Autostart ───────────────────────────────────────────────────────────
wm.autostart("terminal")
wm.autostart("assistant")
wm.autostart("monitor")

-- Greet the user once the desktop is ready.
wm.notify("Interstellar OS ready — press Alt+a to talk to your agent", "ok")
`;
