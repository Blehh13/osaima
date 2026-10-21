/**
 * app-main.js — runs one OSAIMA app as its own desktop window.
 *
 * app.html?app=assistant (or notes, calendar, monitor, settings, ...) mounts
 * that single app full-window. The real window manager (sway) arranges these
 * windows next to ordinary Linux apps; window intents from the assistant go
 * to sway through the daemon (services.desktopMode).
 */
import { invoke } from './bridge.js';
import { registerApps } from './wm/apps.js';
import { makeSettings } from './wm/settings-app.js';
import { makeHistory } from './wm/history-app.js';
import { makeLauncher } from './wm/launcher-app.js';
import { WidgetManager } from './wm/widget-manager.js';
import { RagEngine, SEED_KNOWLEDGE } from './wm/rag.js';
import { BehaviorStore } from './wm/behavior.js';

const appId = new URLSearchParams(location.search).get('app') || 'assistant';

// Toasts inside this window (system-wide notifications go through mako).
const toasts = document.getElementById('notifications');
function notify(message, kind = 'info') {
  const t = document.createElement('div');
  t.className = 'toast toast-' + kind;
  t.textContent = message;
  toasts.append(t);
  requestAnimationFrame(() => t.classList.add('show'));
  setTimeout(() => { t.classList.remove('show'); setTimeout(() => t.remove(), 300); }, 3200);
}

const launch = (app) => invoke('os_action', { action: 'app', op: 'launch', app, approved: false })
  .catch((e) => notify(String(e.message || e), 'err'));

// Stands in for the in-page window manager the apps were written against:
// opening an app starts a real window instead.
const desktop = {
  windows: [],
  appRegistry: new Map(),
  workspaces: [],
  currentWs: 0,
  focusedId: null,
  config: { gaps: 0 },
  spawn: (id) => { launch(id); return null; },
  notify,
  focus() {}, close() {}, closeFocused() {}, minimize() {}, restore() {}, layout() {},
  getWindow: () => null,
  windowsOn: () => [],
  setLayout: (l) => invoke('os_action', { action: 'layout', op: l }).catch(() => {}),
  switchWorkspace: (i) => invoke('os_action', { action: 'workspace', op: 'switch', value: i + 1 }).catch(() => {}),
};

const rag = new RagEngine();
for (const doc of SEED_KNOWLEDGE) rag.ingest(doc);
rag.buildIndex();

const services = {
  invoke,
  rag,
  behavior: new BehaviorStore(),
  widgets: new WidgetManager({ invoke }),
  desktopMode: true,
  getConfigSource: () => '',
  setConfigSource() {},
  reloadConfig: async () => ({ ok: false, error: 'The window layout is set by the desktop session.' }),
  saveConfig: async () => ({ ok: false, error: 'The window layout is set by the desktop session.' }),
  resetConfig: async () => ({ ok: false }),
};

const apps = new Map();
registerApps({ registerApp: (a) => apps.set(a.id, a) }, services);
apps.set('settings', makeSettings(services));
apps.set('history', makeHistory(services));
apps.set('launcher', makeLauncher(services));
apps.set('config', apps.get('settings'));

const app = apps.get(appId) || apps.get('assistant');
document.title = `OSAIMA ${app.title}`;
document.documentElement.dataset.app = app.id;
const root = document.getElementById('app-root');
try {
  app.mount(root, { wm: desktop, win: { id: 1, setTitle: (t) => { document.title = `OSAIMA ${t}`; } } });
} catch (err) {
  root.textContent = `${app.title} failed to start: ${err.message}`;
}

// First start: the assistant opens Settings so the user picks Fast or Private.
if (app.id === 'assistant') {
  let welcomed = false;
  try { welcomed = !!localStorage.getItem('osaima_welcomed'); } catch {}
  if (!welcomed) launch('settings');
}
