/**
 * boot.js — brings the Lua-driven window manager online.
 *
 * Wires together: the WM engine, the embedded Lua VM, the `wm` API surface, the
 * built-in apps, and the live config (fetched from config/wm.lua with an
 * embedded fallback). Exposes a reload path so the config editor can hot-swap
 * the Lua config without restarting the shell.
 */

import { WindowManager } from './window-manager.js';
import { LuaVM } from './lua-vm.js';
import { createWmApi } from './api.js';
import { registerApps } from './apps.js';
import { DEFAULT_WM_LUA } from './default-config.js';

// Candidate locations for the Lua config, tried in order. Works whether the
// shell is served from the project root (tauri/static server) or elsewhere.
const CONFIG_URLS = ['./src/config/wm.lua', './config/wm.lua', 'src/config/wm.lua'];

async function loadConfigSource() {
  for (const url of CONFIG_URLS) {
    try {
      const res = await fetch(url, { cache: 'no-store' });
      if (res.ok) {
        const text = await res.text();
        if (text && text.trim()) return text;
      }
    } catch {
      // fetch blocked (file://) or not found — try next / fall back.
    }
  }
  return DEFAULT_WM_LUA;
}

/**
 * @param {object} opts
 * @param {HTMLElement} opts.surface  container to render windows into
 * @param {Function} opts.invoke      Tauri-style invoke(cmd) for MCP data
 * @param {object} [opts.hooks]       engine hooks (focus/layout/workspace/windows)
 * @returns {Promise<{ engine: WindowManager, reload: Function }>}
 */
export async function initWindowManager({ surface, invoke, hooks = {} }) {
  const engine = new WindowManager(surface, hooks);

  let currentSource = await loadConfigSource();

  const services = {
    invoke,
    getConfigSource: () => currentSource,
    setConfigSource: (s) => { currentSource = s; },
    reloadConfig: async (source) => applyLua(source, { boot: false }),
  };

  registerApps(engine, services);

  // Apply a Lua config string against the engine. On boot we also run autostart.
  function applyLua(source, { boot }) {
    try {
      engine.resetForReload();
      const vm = new LuaVM();
      vm.setGlobal('wm', createWmApi(engine));
      vm.run(source, 'wm.lua');
      currentSource = source;
      engine.applyConfig();
      if (boot) engine.bootAutostart();
      else engine.layout();
      return { ok: true };
    } catch (err) {
      console.error('[wm.lua] config error:', err);
      return { ok: false, error: err.message || String(err) };
    }
  }

  const result = applyLua(currentSource, { boot: true });
  if (!result.ok) {
    // If the user's config is broken at boot, fall back to the shipped default
    // so the desktop still comes up.
    console.warn('[wm.lua] falling back to default config');
    applyLua(DEFAULT_WM_LUA, { boot: true });
  }

  return { engine, reload: (src) => applyLua(src, { boot: false }) };
}
