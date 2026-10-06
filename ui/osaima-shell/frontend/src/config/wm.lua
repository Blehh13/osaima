-- ═══════════════════════════════════════════════════════════════════════
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

-- ── Shell settings ──────────────────────────────────────────────────────
-- Everything here is optional; the commented values are the defaults. Remove the
-- leading "--" from a line to change it. A bad value falls back to its default
-- and is reported in the developer console.
wm.shell({
  -- How often things refresh, in milliseconds
  -- clock_ms = 1000,                -- clocks              (250 to 60000)
  -- stats_poll_ms = 3000,           -- CPU / memory pills  (500 to 60000)
  -- monitor_poll_ms = 1500,         -- System Monitor app  (250 to 60000)
  -- task_poll_ms = 2000,            -- Task Manager app    (500 to 60000)

  -- Notifications and the proactive assistant ("AI Core" suggestions)
  -- notify_ms = 3200,               -- how long a notice stays    (500 to 60000)
  -- notify_action_ms = 7000,        -- ...when it has a button    (500 to 120000)
  -- proactive_enabled = true,       -- set false to stop suggestions
  -- proactive_interval_ms = 8000,   -- how often it looks around  (1000 to 600000)
  -- proactive_first_ms = 4000,      -- wait after startup         (0 to 600000)
  -- proactive_quiet_ms = 12000,     -- minimum gap between suggestions
  -- cpu_alert_percent = 70,         -- suggest Task Manager above this (1 to 100)
  -- memory_alert_percent = 85,      -- suggest Monitor above this      (1 to 100)
  -- grid_suggest_windows = 4,       -- suggest the grid layout at this many windows

  -- Look
  -- star_count = 320,               -- background stars (0 to 2000)

  -- Apps and shortcuts
  -- dock_apps = { "assistant", "browser", "terminal", "files", "monitor", "taskmanager", "knowledge", "behavior", "config", "about" },
  -- search_url = "https://en.wikipedia.org/w/index.php?search=%s",  -- %s = the search words
  -- bookmarks = { { name = "Gentoo", url = "https://www.gentoo.org" }, { name = "MDN Web Docs", url = "https://developer.mozilla.org" } },
  -- assistant_suggestions = { "What is using my memory?", "Open the terminal" },
  -- launcher_suggestions = { "Show system stats", "Open terminal" },
})

-- Greet the user once the desktop is ready.
wm.notify("Interstellar OS ready — press Alt+a to talk to your agent", "ok")
