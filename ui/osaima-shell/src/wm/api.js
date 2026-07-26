/**
 * api.js — the `wm` API surface exposed to the Lua configuration.
 *
 * When the shell loads `config/wm.lua`, this table is injected as the global
 * `wm`. Every call the config makes (wm.gaps, wm.bind, wm.layout, …) mutates the
 * WindowManager engine's config and registries. After the config finishes
 * running, the engine calls applyConfig() to make it all live.
 *
 * Because it's a real Lua environment, configs can compute values, loop to
 * generate per-workspace keybindings, define inline action functions, etc.
 */

import { LuaTable, luaToJS } from './lua-vm.js';

/**
 * Build the `wm` LuaTable bound to a WindowManager engine.
 * @param {import('./window-manager.js').WindowManager} engine
 * @returns {LuaTable}
 */
export function createWmApi(engine) {
  const api = new LuaTable();

  // Generic setter: wm.set("gaps", 12)
  api.set('set', (key, value) => {
    if (key in engine.config) engine.config[key] = coerce(value);
    else engine.config[key] = coerce(value);
  });

  api.set('layout', (name) => { engine.config.layout = String(name); });
  api.set('gaps', (px) => { engine.config.gaps = num(px, engine.config.gaps); });
  api.set('master_ratio', (r) => { engine.config.masterRatio = num(r, engine.config.masterRatio); });
  api.set('master_count', (n) => { engine.config.masterCount = Math.max(1, Math.floor(num(n, 1))); });
  api.set('modkey', (name) => { engine.config.modkey = String(name); });
  api.set('bar_reserve', (px) => { engine.config.barReserve = num(px, engine.config.barReserve); });

  // wm.border(2, "#78b4ff", "rgba(...)")  OR  wm.border{ width=, focused=, unfocused= }
  api.set('border', (a, b, c) => {
    if (a instanceof LuaTable) {
      const t = luaToJS(a);
      if (t.width !== undefined) engine.config.borderWidth = t.width;
      if (t.focused) engine.config.borderFocused = t.focused;
      if (t.unfocused) engine.config.borderUnfocused = t.unfocused;
    } else {
      if (a !== undefined) engine.config.borderWidth = num(a, engine.config.borderWidth);
      if (b !== undefined) engine.config.borderFocused = String(b);
      if (c !== undefined) engine.config.borderUnfocused = String(c);
    }
  });

  // wm.workspaces({"web","code","chat"})  OR  wm.workspaces("1","2","3")
  api.set('workspaces', (...args) => {
    let names;
    if (args.length === 1 && args[0] instanceof LuaTable) {
      names = luaToJS(args[0]).map(String);
    } else {
      names = args.map(String);
    }
    if (names.length) engine.config.workspaceNames = names;
  });

  // wm.bind("Mod+Return", "spawn:terminal")  OR  wm.bind("Mod+r", function() ... end)
  api.set('bind', (binding, action) => {
    engine.addBinding(binding, action);
  });

  // wm.rule{ match="terminal", floating=true, workspace=2 }
  api.set('rule', (t) => {
    if (!(t instanceof LuaTable)) return;
    const r = luaToJS(t);
    engine.rules.push({
      match: r.match,
      floating: r.floating,
      workspace: r.workspace !== undefined ? r.workspace - 1 : undefined, // Lua is 1-based
      title: r.title,
    });
  });

  // wm.autostart("terminal")  OR  wm.autostart{"terminal","monitor"}
  api.set('autostart', (...args) => {
    let ids;
    if (args.length === 1 && args[0] instanceof LuaTable) ids = luaToJS(args[0]).map(String);
    else ids = args.map(String);
    for (const id of ids) engine.autostart.push(id);
  });

  // Runtime helpers usable from action functions.
  api.set('spawn', (appId, tbl) => {
    const opts = tbl instanceof LuaTable ? luaToJS(tbl) : {};
    if (opts.workspace !== undefined) opts.workspace -= 1;
    return engine.spawn(String(appId), opts);
  });
  api.set('close', () => engine.closeFocused());
  api.set('focus_next', () => engine.focusRelative(1));
  api.set('focus_prev', () => engine.focusRelative(-1));
  api.set('cycle_layout', () => engine.cycleLayout());
  api.set('set_layout', (name) => engine.setLayout(String(name)));
  api.set('toggle_floating', () => engine.toggleFloating());
  api.set('workspace', (n) => engine.switchWorkspace(Math.floor(num(n, 1)) - 1));
  api.set('log', (...a) => console.log('[wm.lua]', ...a.map(String)));

  return api;
}

function num(v, fallback) {
  const n = typeof v === 'number' ? v : Number(v);
  return Number.isFinite(n) ? n : fallback;
}

function coerce(v) {
  if (v instanceof LuaTable) return luaToJS(v);
  return v;
}
