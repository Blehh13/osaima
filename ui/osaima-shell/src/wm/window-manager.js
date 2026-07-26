/**
 * window-manager.js — Interstellar OS window manager engine.
 *
 * A dynamic tiling/floating window manager (dwm/i3-inspired) whose behaviour is
 * driven entirely by a Lua configuration file. This module is the engine; the
 * `wm.*` API surface that the Lua config talks to lives in api.js, and the
 * built-in windows live in apps.js.
 *
 * Layouts: tile (master/stack), monocle (fullscreen stack), float (free).
 * Concepts: workspaces (tags), keybindings, window rules, gaps, master ratio.
 */

let WINDOW_SEQ = 0;

function defaultConfig() {
  return {
    layout: 'tile',
    gaps: 10,
    masterRatio: 0.6,
    masterCount: 1,
    modkey: 'Alt',
    borderWidth: 2,
    borderFocused: '#78b4ff',
    borderUnfocused: 'rgba(100,160,255,0.12)',
    barReserve: 44,          // px reserved at top for the status bar
    workspaceNames: ['1', '2', '3', '4', '5'],
  };
}

export class WindowManager {
  /**
   * @param {HTMLElement} surface  container the windows are rendered into
   * @param {object} hooks         { onFocusChange, onLayoutChange, onWorkspaceChange, onWindowsChange }
   */
  constructor(surface, hooks = {}) {
    this.surface = surface;
    this.hooks = hooks;

    this.windows = [];
    this.focusedId = null;

    // Registries populated by the Lua config (via api.js).
    this.bindings = new Map();      // canonical binding string -> action (string | fn)
    this.rules = [];                // { match, floating, workspace, title }
    this.autostart = [];            // [appId, ...]
    this.appRegistry = new Map();   // appId -> app definition

    // Configurable options (defaults; overridden by Lua).
    this.config = defaultConfig();

    this.workspaces = [];
    this.currentWs = 0;

    this._keyHandler = null;
    this._resizeHandler = () => this.layout();
    window.addEventListener('resize', this._resizeHandler);
  }

  registerApp(app) {
    this.appRegistry.set(app.id, app);
  }

  // Clear config-derived state so a fresh Lua config can be applied cleanly.
  // Windows and the app registry are preserved (live reload keeps windows open).
  resetForReload() {
    this.bindings.clear();
    this.rules = [];
    this.autostart = [];
    this.config = defaultConfig();
  }

  // ── Config application ─────────────────────────────────────────────────────
  applyConfig() {
    // Workspaces
    this.workspaces = this.config.workspaceNames.map((name) => ({ name }));
    if (this.currentWs >= this.workspaces.length) this.currentWs = 0;

    // CSS custom props for borders/gaps
    this.surface.style.setProperty('--wm-border-w', this.config.borderWidth + 'px');

    // Keyboard
    this.bindKeyboard();

    if (this.hooks.onWorkspaceChange) {
      this.hooks.onWorkspaceChange(this.currentWs, this.workspaces);
    }
    if (this.hooks.onLayoutChange) this.hooks.onLayoutChange(this.config.layout);
  }

  bootAutostart() {
    for (const appId of this.autostart) this.spawn(appId);
    this.layout();
  }

  // ── Keyboard handling ──────────────────────────────────────────────────────
  bindKeyboard() {
    if (this._keyHandler) window.removeEventListener('keydown', this._keyHandler, true);
    this._keyHandler = (e) => {
      // Don't hijack typing inside inputs/textareas/editors.
      const tag = e.target && e.target.tagName;
      const editable = e.target && e.target.isContentEditable;
      const typing = tag === 'INPUT' || tag === 'TEXTAREA' || editable;

      const binding = this.eventToBinding(e);
      if (!binding) return;
      const action = this.bindings.get(binding);
      if (!action) return;

      // Allow WM chords even while typing only if a real modifier is held.
      if (typing && !(e.altKey || e.ctrlKey || e.metaKey)) return;

      e.preventDefault();
      e.stopPropagation();
      this.runAction(action);
    };
    window.addEventListener('keydown', this._keyHandler, true);
  }

  eventToBinding(e) {
    const mods = [];
    if (e.ctrlKey) mods.push('Ctrl');
    if (e.altKey) mods.push('Alt');
    if (e.shiftKey) mods.push('Shift');
    if (e.metaKey) mods.push('Super');

    let key = e.key;
    if (key === ' ') key = 'Space';
    else if (key === 'Enter') key = 'Return';
    else if (key === 'Escape') key = 'Escape';
    else if (key === 'ArrowLeft') key = 'Left';
    else if (key === 'ArrowRight') key = 'Right';
    else if (key === 'ArrowUp') key = 'Up';
    else if (key === 'ArrowDown') key = 'Down';
    else if (key === 'Tab') key = 'Tab';
    else if (key.length === 1) key = key.toUpperCase();
    else return null; // ignore lone modifier presses etc.

    // A bare key with no modifier is never a WM binding (avoids stealing input).
    if (mods.length === 0) return null;
    return [...mods, key].join('+');
  }

  // Normalize a config binding string ("shift+alt+c", "Mod+Return") to canonical.
  canonicalizeBinding(str) {
    const parts = String(str).split('+').map((p) => p.trim()).filter(Boolean);
    const mods = new Set();
    let key = '';
    for (const p of parts) {
      const low = p.toLowerCase();
      if (low === 'ctrl' || low === 'control') mods.add('Ctrl');
      else if (low === 'alt') mods.add('Alt');
      else if (low === 'shift') mods.add('Shift');
      else if (low === 'super' || low === 'meta' || low === 'win' || low === 'cmd') mods.add('Super');
      else if (low === 'mod') {
        // "Mod" resolves to the configured modkey.
        const mk = this.config.modkey.toLowerCase();
        if (mk === 'ctrl' || mk === 'control') mods.add('Ctrl');
        else if (mk === 'alt') mods.add('Alt');
        else if (mk === 'super' || mk === 'meta' || mk === 'win') mods.add('Super');
        else mods.add('Alt');
      } else {
        key = normalizeKeyName(p);
      }
    }
    const order = ['Ctrl', 'Alt', 'Shift', 'Super'];
    const orderedMods = order.filter((m) => mods.has(m));
    return [...orderedMods, key].join('+');
  }

  addBinding(binding, action) {
    this.bindings.set(this.canonicalizeBinding(binding), action);
  }

  // ── Action dispatch ────────────────────────────────────────────────────────
  runAction(action) {
    if (typeof action === 'function') {
      // Lua-defined action; call it (via the VM's callFunction wrapper if attached).
      try { action(); } catch (err) { console.error('binding action error:', err); }
      return;
    }
    const [cmd, arg] = String(action).split(':');
    switch (cmd) {
      case 'focus_next': this.focusRelative(1); break;
      case 'focus_prev': this.focusRelative(-1); break;
      case 'close_window': this.closeFocused(); break;
      case 'toggle_floating': this.toggleFloating(); break;
      case 'cycle_layout': this.cycleLayout(); break;
      case 'set_layout': this.setLayout(arg); break;
      case 'spawn': this.spawn(arg); break;
      case 'workspace': this.switchWorkspace(parseInt(arg, 10) - 1); break;
      case 'workspace_next': this.switchWorkspace(this.currentWs + 1); break;
      case 'workspace_prev': this.switchWorkspace(this.currentWs - 1); break;
      case 'move_to_workspace': this.moveFocusedToWorkspace(parseInt(arg, 10) - 1); break;
      case 'increase_master': this.setMasterRatio(this.config.masterRatio + 0.05); break;
      case 'decrease_master': this.setMasterRatio(this.config.masterRatio - 0.05); break;
      case 'increase_master_count': this.config.masterCount++; this.layout(); break;
      case 'decrease_master_count': this.config.masterCount = Math.max(1, this.config.masterCount - 1); this.layout(); break;
      case 'open_launcher': if (window.openLauncher) window.openLauncher(); break;
      case 'quit': this.minimizeAll(); break;
      default: console.warn('unknown WM action:', action);
    }
  }

  // ── Window lifecycle ───────────────────────────────────────────────────────
  spawn(appId, opts = {}) {
    const app = this.appRegistry.get(appId);
    if (!app) { console.warn('no such app:', appId); return null; }

    const rule = this.matchRule(app);
    const id = ++WINDOW_SEQ;

    const el = document.createElement('div');
    el.className = 'wm-window';
    el.dataset.wid = id;
    el.innerHTML = `
      <div class="wm-titlebar">
        <span class="wm-title-icon">${app.icon || '▣'}</span>
        <span class="wm-title-text"></span>
        <div class="wm-title-actions">
          <button class="wm-btn wm-btn-min" title="Minimize">–</button>
          <button class="wm-btn wm-btn-close" title="Close">✕</button>
        </div>
      </div>
      <div class="wm-content"></div>
      <div class="wm-resize-handle"></div>`;

    const titleText = el.querySelector('.wm-title-text');
    const contentEl = el.querySelector('.wm-content');

    const win = {
      id,
      appId,
      title: opts.title || rule.title || app.title,
      el,
      contentEl,
      titleEl: titleText,
      floating: opts.floating ?? rule.floating ?? app.floating ?? (this.config.layout === 'float'),
      minimized: false,
      workspace: opts.workspace ?? rule.workspace ?? this.currentWs,
      // Free-floating geometry (used in float layout / floating windows).
      x: 0, y: 0, w: app.width || 520, h: app.height || 360,
    };
    win.setTitle = (t) => { win.title = t; titleText.textContent = t; if (this.hooks.onWindowsChange) this.hooks.onWindowsChange(); };
    win.setTitle(win.title);

    // Cascade initial floating position.
    const floatCount = this.windows.filter((w) => w.floating).length;
    win.x = 80 + floatCount * 28;
    win.y = this.config.barReserve + 40 + floatCount * 28;

    this.surface.appendChild(el);
    this.windows.push(win);

    // Interactions
    el.addEventListener('mousedown', () => this.focus(id), true);
    el.querySelector('.wm-btn-close').addEventListener('click', (e) => { e.stopPropagation(); this.close(id); });
    el.querySelector('.wm-btn-min').addEventListener('click', (e) => { e.stopPropagation(); this.minimize(id); });
    this.makeDraggable(win);
    this.makeResizable(win);

    // Mount app content.
    try {
      app.mount(contentEl, { wm: this, win });
    } catch (err) {
      contentEl.innerHTML = `<div class="wm-error">App failed to start: ${err.message}</div>`;
    }

    this.focus(id);
    this.layout();
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
    return win;
  }

  close(id) {
    const idx = this.windows.findIndex((w) => w.id === id);
    if (idx === -1) return;
    const [win] = this.windows.splice(idx, 1);
    const app = this.appRegistry.get(win.appId);
    if (app && app.unmount) { try { app.unmount(win); } catch {} }
    win.el.classList.add('closing');
    setTimeout(() => win.el.remove(), 160);
    if (this.focusedId === id) {
      const remaining = this.visibleWindows();
      this.focusedId = remaining.length ? remaining[remaining.length - 1].id : null;
    }
    this.layout();
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
  }

  closeFocused() { if (this.focusedId) this.close(this.focusedId); }

  minimize(id) {
    const win = this.getWindow(id);
    if (!win) return;
    win.minimized = true;
    if (this.focusedId === id) {
      const vis = this.visibleWindows();
      this.focusedId = vis.length ? vis[vis.length - 1].id : null;
    }
    this.layout();
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
  }

  restore(id) {
    const win = this.getWindow(id);
    if (!win) return;
    win.minimized = false;
    win.workspace = this.currentWs;
    this.focus(id);
    this.layout();
  }

  minimizeAll() {
    for (const w of this.windows) if (w.workspace === this.currentWs) w.minimized = true;
    this.focusedId = null;
    this.layout();
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
  }

  // ── Focus ──────────────────────────────────────────────────────────────────
  focus(id) {
    this.focusedId = id;
    for (const w of this.windows) {
      const active = w.id === id;
      w.el.classList.toggle('focused', active);
      if (active) w.el.style.zIndex = String(1000 + (this._zTop = (this._zTop || 1000) + 1));
    }
    if (this.hooks.onFocusChange) this.hooks.onFocusChange(this.getWindow(id));
  }

  focusRelative(dir) {
    const vis = this.visibleWindows();
    if (vis.length === 0) return;
    let idx = vis.findIndex((w) => w.id === this.focusedId);
    if (idx === -1) idx = 0;
    else idx = (idx + dir + vis.length) % vis.length;
    this.focus(vis[idx].id);
  }

  // ── Layout / workspace ─────────────────────────────────────────────────────
  setLayout(name) {
    if (!['tile', 'float', 'monocle'].includes(name)) return;
    this.config.layout = name;
    this.layout();
    if (this.hooks.onLayoutChange) this.hooks.onLayoutChange(name);
  }

  cycleLayout() {
    const order = ['tile', 'monocle', 'float'];
    const next = order[(order.indexOf(this.config.layout) + 1) % order.length];
    this.setLayout(next);
  }

  setMasterRatio(r) {
    this.config.masterRatio = Math.max(0.2, Math.min(0.8, r));
    this.layout();
  }

  toggleFloating() {
    const win = this.getWindow(this.focusedId);
    if (!win) return;
    win.floating = !win.floating;
    this.layout();
  }

  switchWorkspace(idx) {
    if (idx < 0 || idx >= this.workspaces.length) return;
    this.currentWs = idx;
    // Focus a window on the new workspace, if any.
    const vis = this.visibleWindows();
    this.focusedId = vis.length ? vis[vis.length - 1].id : null;
    this.layout();
    if (this.hooks.onWorkspaceChange) this.hooks.onWorkspaceChange(this.currentWs, this.workspaces);
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
  }

  moveFocusedToWorkspace(idx) {
    const win = this.getWindow(this.focusedId);
    if (!win || idx < 0 || idx >= this.workspaces.length) return;
    win.workspace = idx;
    this.focusedId = null;
    this.layout();
    if (this.hooks.onWindowsChange) this.hooks.onWindowsChange();
  }

  // ── The layout algorithm ───────────────────────────────────────────────────
  layout() {
    const rect = this.surface.getBoundingClientRect();
    const gap = this.config.gaps;
    const top = this.config.barReserve + gap;
    const area = {
      x: gap,
      y: top,
      w: rect.width - gap * 2,
      h: rect.height - top - gap,
    };

    // Hide windows not on the current workspace or minimized.
    for (const w of this.windows) {
      const onWs = w.workspace === this.currentWs && !w.minimized;
      w.el.style.display = onWs ? 'flex' : 'none';
    }

    const tiled = this.windows.filter(
      (w) => w.workspace === this.currentWs && !w.minimized && !w.floating,
    );
    const floating = this.windows.filter(
      (w) => w.workspace === this.currentWs && !w.minimized && w.floating,
    );

    // Position floating windows at their free geometry.
    for (const w of floating) {
      this.placeFree(w);
      w.el.classList.add('floating');
    }

    if (this.config.layout === 'float') {
      // Everything free-floats.
      for (const w of tiled) { w.el.classList.add('floating'); this.placeFree(w); }
    } else if (this.config.layout === 'monocle') {
      for (const w of tiled) {
        w.el.classList.remove('floating');
        this.place(w, area.x, area.y, area.w, area.h);
      }
    } else {
      // tile: master + stack
      for (const w of tiled) w.el.classList.remove('floating');
      this.tileMasterStack(tiled, area, gap);
    }
  }

  tileMasterStack(wins, area, gap) {
    const n = wins.length;
    if (n === 0) return;
    const mc = Math.min(this.config.masterCount, n);

    if (n <= mc) {
      // Single column, split evenly.
      const cellH = (area.h - gap * (n - 1)) / n;
      wins.forEach((w, i) => {
        this.place(w, area.x, area.y + i * (cellH + gap), area.w, cellH);
      });
      return;
    }

    const masterW = Math.round((area.w - gap) * this.config.masterRatio);
    const stackW = area.w - gap - masterW;

    const masters = wins.slice(0, mc);
    const stack = wins.slice(mc);

    const mCellH = (area.h - gap * (masters.length - 1)) / masters.length;
    masters.forEach((w, i) => {
      this.place(w, area.x, area.y + i * (mCellH + gap), masterW, mCellH);
    });

    const sCellH = (area.h - gap * (stack.length - 1)) / stack.length;
    stack.forEach((w, i) => {
      this.place(w, area.x + masterW + gap, area.y + i * (sCellH + gap), stackW, sCellH);
    });
  }

  place(win, x, y, w, h) {
    win.el.style.left = Math.round(x) + 'px';
    win.el.style.top = Math.round(y) + 'px';
    win.el.style.width = Math.round(w) + 'px';
    win.el.style.height = Math.round(h) + 'px';
  }

  placeFree(win) {
    const rect = this.surface.getBoundingClientRect();
    win.x = Math.max(0, Math.min(win.x, rect.width - 80));
    win.y = Math.max(this.config.barReserve, Math.min(win.y, rect.height - 40));
    this.place(win, win.x, win.y, win.w, win.h);
  }

  // ── Drag & resize (floating windows) ───────────────────────────────────────
  makeDraggable(win) {
    const bar = win.el.querySelector('.wm-titlebar');
    let sx, sy, ox, oy, dragging = false;
    const onMove = (e) => {
      if (!dragging) return;
      win.x = ox + (e.clientX - sx);
      win.y = oy + (e.clientY - sy);
      this.place(win, win.x, win.y, win.w, win.h);
    };
    const onUp = () => {
      dragging = false;
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };
    bar.addEventListener('mousedown', (e) => {
      if (e.target.closest('.wm-btn')) return;
      this.focus(win.id);
      // In tiling layouts, dragging pops the window into floating mode.
      if (!win.floating) { win.floating = true; this.layout(); }
      dragging = true;
      sx = e.clientX; sy = e.clientY; ox = win.x; oy = win.y;
      document.addEventListener('mousemove', onMove);
      document.addEventListener('mouseup', onUp);
      e.preventDefault();
    });
    bar.addEventListener('dblclick', (e) => {
      if (e.target.closest('.wm-btn')) return;
      win.floating = !win.floating;
      this.layout();
    });
  }

  makeResizable(win) {
    const handle = win.el.querySelector('.wm-resize-handle');
    let sx, sy, ow, oh, resizing = false;
    const onMove = (e) => {
      if (!resizing) return;
      win.w = Math.max(240, ow + (e.clientX - sx));
      win.h = Math.max(160, oh + (e.clientY - sy));
      this.place(win, win.x, win.y, win.w, win.h);
    };
    const onUp = () => {
      resizing = false;
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };
    handle.addEventListener('mousedown', (e) => {
      this.focus(win.id);
      if (!win.floating) { win.floating = true; this.layout(); }
      resizing = true;
      sx = e.clientX; sy = e.clientY; ow = win.el.offsetWidth; oh = win.el.offsetHeight;
      win.w = ow; win.h = oh;
      document.addEventListener('mousemove', onMove);
      document.addEventListener('mouseup', onUp);
      e.preventDefault();
      e.stopPropagation();
    });
  }

  // ── Rules ──────────────────────────────────────────────────────────────────
  matchRule(app) {
    for (const rule of this.rules) {
      if (rule.match === app.id || rule.match === app.title) return rule;
    }
    return {};
  }

  // ── Queries ────────────────────────────────────────────────────────────────
  getWindow(id) { return this.windows.find((w) => w.id === id) || null; }

  visibleWindows() {
    return this.windows.filter((w) => w.workspace === this.currentWs && !w.minimized);
  }

  windowsOn(wsIdx) {
    return this.windows.filter((w) => w.workspace === wsIdx);
  }

  destroy() {
    if (this._keyHandler) window.removeEventListener('keydown', this._keyHandler, true);
    window.removeEventListener('resize', this._resizeHandler);
  }
}

function capitalize(s) {
  return s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();
}

// Map friendly key names in config bindings to the tokens eventToBinding emits.
const KEY_NAME_MAP = {
  slash: '/', backslash: '\\', comma: ',', period: '.', dot: '.',
  semicolon: ';', space: 'Space', return: 'Return', enter: 'Return',
  tab: 'Tab', escape: 'Escape', esc: 'Escape', up: 'Up', down: 'Down',
  left: 'Left', right: 'Right', minus: '-', plus: '+', equal: '=',
};
function normalizeKeyName(p) {
  const low = p.toLowerCase();
  if (KEY_NAME_MAP[low]) return KEY_NAME_MAP[low];
  return p.length === 1 ? p.toUpperCase() : capitalize(p);
}
