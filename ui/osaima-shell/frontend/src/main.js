/**
 * osaima-shell — main.js
 * OSAIMA Agentic UI Shell
 *
 * Responsibilities:
 *  1. Animated starfield canvas
 *  2. Live clock
 *  3. AI Command Hub (launcher) open/close + keyboard nav
 *  4. MCP Daemon polling via Tauri IPC (with graceful fallback)
 *  5. System stats display
 */

import { initWindowManager } from './wm/boot.js';
import { dispatchIntent } from './wm/apps.js';
import { AgentCore } from './wm/agent-core.js';
import { RagEngine, SEED_KNOWLEDGE } from './wm/rag.js';
import { BehaviorStore } from './wm/behavior.js';
import { initVoxType } from './wm/voxtype.js';

// ── Tauri invoke (safe import for non-Tauri dev environments) ──────────────
let tauriInvoke = null;
if (window.__TAURI__) {
  const raw = window.__TAURI__.invoke || (window.__TAURI__.core && window.__TAURI__.core.invoke) || window.__TAURI__.tauri.invoke;
  // os_action takes the whole request as one argument (see bridge.js)
  tauriInvoke = (cmd, args) => raw(cmd, cmd === 'os_action' ? { request: args } : args);
}
if (!tauriInvoke) {
  try {
    const tauri = await import('@tauri-apps/api/core');
    tauriInvoke = tauri.invoke;
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().setFullscreen(true);
    } catch(err) {
      console.warn('Failed to set fullscreen:', err);
    }
  } catch {
    // Running in browser dev mode — try backend server, with graceful fallback
    tauriInvoke = async (cmd, args = {}) => {
      let data;
      try {
        const resp = await fetch('/api/invoke', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ cmd, args })
        });
        if (resp.ok) data = await resp.json();
      } catch (err) {
        // Backend not available or fetch error; use built-in fallback
      }
      // Reject like Tauri's invoke does, so refusals are never mistaken for success
      if (data && data.ok === false) throw new Error(data.error || `${cmd} failed`);
      if (data && data.result !== undefined) return data.result;

      if (cmd === 'get_system_stats') {
        return {
          os: 'OSAIMA Linux (Agentic Shell)',
          kernel: '7.0.0-31-generic',
          host: 'osaima-agentic',
          memory: { total_bytes: 17179869184, used_bytes: 5368709120 },
          cpu: { usage_percent: 12.8, cores: 8 },
        };
      }
      if (cmd === 'ping_daemon') return true;
      if (cmd === 'run_command') return 'Executed: ' + (args?.cmd || '');
      if (cmd === 'system_get_suggestion') return 'System health is nominal. All services operational.';
      if (cmd === 'list_processes') return [];
      if (cmd === 'read_dir') return [];
      return null;
    };
  }
}

// ── 1. Low-power detection ────────────────────────────────────────────────
// Without a GPU, every animation and glass blur is composited on the CPU, which
// starved the on-device LLM in a VM (0.3 -> 5 tok/s once removed). The desktop
// is now flat, and .low-power (style.css) also stops the remaining animations.
function softwareRendering() {
  try {
    const gl = document.createElement('canvas').getContext('webgl');
    if (!gl) return true;
    const info = gl.getExtension('WEBGL_debug_renderer_info');
    const renderer = String(info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER));
    return /llvmpipe|softpipe|swiftshader|software/i.test(renderer);
  } catch {
    return true;
  }
}
if (matchMedia('(prefers-reduced-motion: reduce)').matches || softwareRendering()) {
  document.documentElement.classList.add('low-power');
}

// ── 2. Live Clock ─────────────────────────────────────────────────────────
function updateClock() {
  const now = new Date();
  document.getElementById('clock-time').textContent = now.toLocaleTimeString([], {
    hour: '2-digit', minute: '2-digit',
  });
  document.getElementById('clock-date').textContent = now.toLocaleDateString([], {
    weekday: 'short', month: 'short', day: 'numeric',
  });
}
updateClock();
setInterval(updateClock, 1000);

// ── 3. Launcher (AI Command Hub) ──────────────────────────────────────────
const launcher = document.getElementById('launcher');
const launcherInput = document.getElementById('launcher-input');

// The assistant is the shell, so "open the launcher" means: bring it up and
// put the cursor in its input.
function openLauncher() {
  if (!window.WM) return;
  const home = window.WM.windows.find((w) => w.appId === 'assistant');
  if (!home) window.WM.spawn('assistant');
  else {
    if (home.workspace !== window.WM.currentWs) window.WM.switchWorkspace(home.workspace);
    if (home.minimized) window.WM.restore(home.id);
    window.WM.focus(home.id);
  }
  window.dispatchEvent(new Event('osaima:focus'));
}

function closeLauncher() {
  launcher.classList.add('hidden');
  launcherInput.value = '';
  document.getElementById('launcher-response').classList.add('hidden');
}

window.openLauncher = openLauncher;
window.closeLauncher = closeLauncher;

// Keyboard shortcuts
window.addEventListener('keydown', (e) => {
  if (e.code === 'Space' && !e.target.matches('input, textarea')) {
    e.preventDefault();
    openLauncher();
  }
  if (e.code === 'Escape') closeLauncher();
});

// ── 4. Launcher input handling ────────────────────────────────────────────
launcherInput.addEventListener('keydown', async (e) => {
  if (e.key !== 'Enter') return;
  const query = launcherInput.value.trim();
  if (!query) return;
  await handleQuery(query);
});

async function handleQuery(query) {
  const responseEl = document.getElementById('launcher-response');
  const contentEl  = document.getElementById('response-content');

  responseEl.classList.remove('hidden');
  contentEl.textContent = '⠋ Processing…';

  await new Promise(r => setTimeout(r, 350)); // small thinking delay for UX

  const lower = query.toLowerCase();

  // Special fast-path for launching the assistant to avoid LLM overhead
  if (lower.includes('assistant') || lower.includes('agent') || lower.includes('ai ')) {
    spawnFromLauncher('assistant', contentEl, 'the AI Assistant');
    return;
  }

  try {
    const intentResult = await tauriInvoke('classify_intent', { query: query });
    if (intentResult && intentResult.intent) {
      // Use the NLP dispatch
      const reply = await dispatchIntent(
        intentResult.intent,
        intentResult.params || {},
        intentResult.reply || '',
        query,
        window.WM,
        window.SERVICES,
        null // no skipWinId
      );
      
      if (reply) {
        contentEl.textContent = reply;
        // Optionally close launcher after a delay if it wasn't a stat request
        if (intentResult.intent !== 'system_status' && intentResult.intent !== 'vitals') {
          setTimeout(closeLauncher, 1500);
        }
        return;
      }
    }
  } catch (err) {
    console.warn("LLM classify_intent failed in launcher", err);
  }

  // ── LEGACY FALLBACK if LLM is offline ──
  if (lower.includes('stat') || lower.includes('system') || lower.includes('cpu') || lower.includes('mem')) {
    try {
      const stats = await tauriInvoke('get_system_stats');
      const memPct = ((stats.memory.used_bytes / stats.memory.total_bytes) * 100).toFixed(1);
      contentEl.textContent =
`HOST    : ${stats.host}
OS      : ${stats.os}
KERNEL  : ${stats.kernel}
CPU     : ${stats.cpu.usage_percent.toFixed(1)}% across ${stats.cpu.cores} cores
MEMORY  : ${memPct}% used (${formatBytes(stats.memory.used_bytes)} / ${formatBytes(stats.memory.total_bytes)})`;
    } catch {
      contentEl.textContent = 'Error: Could not retrieve stats from MCP daemon.';
    }
  } else if (lower.includes('ping') || lower.includes('ai core') || lower.includes('daemon')) {
    const alive = await tauriInvoke('ping_daemon');
    contentEl.textContent = alive
      ? '✓ MCP Daemon is online and responding.'
      : '✗ MCP Daemon is offline. Start it with: systemctl start mcp-daemon';
  } else if (lower.includes('browser') || lower.includes('web') || lower.includes('internet') || lower.includes('chrome')) {
    spawnFromLauncher('browser', contentEl, 'the Browser');
  } else if (lower.includes('terminal') || lower.includes('shell')) {
    spawnFromLauncher('terminal', contentEl, 'Terminal');
  } else if (lower.includes('file') || lower.includes('explorer')) {
    spawnFromLauncher('files', contentEl, 'Files');
  } else if (lower.includes('config') || lower.includes('window manager') || lower.includes('lua')) {
    spawnFromLauncher('config', contentEl, 'the Lua window-manager config');
  } else if (lower.includes('task') || lower.includes('process')) {
    spawnFromLauncher('taskmanager', contentEl, 'Task Manager');
  } else if (lower.includes('monitor')) {
    spawnFromLauncher('monitor', contentEl, 'System Monitor');
  } else if (lower.includes('voxtype') || lower.includes('voice') || lower.includes('speech') || lower.includes('dictat')) {
    closeLauncher();
    if (window.VOXTYPE) window.VOXTYPE.toggle();
    return;
  } else if (lower.includes('about') || lower.includes('keybind') || lower.includes('help')) {
    spawnFromLauncher('about', contentEl, 'About / keybindings');
  } else if (lower.includes('brightness')) {
    contentEl.textContent = '→ Display brightness control:\n(Direct hardware control via udev — planned for Phase 4)';
  } else if (lower.includes('network') || lower.includes('diagnostic')) {
    contentEl.textContent = '→ Network diagnostics:\n(NetworkManager IPC integration — planned for Phase 4)';
  } else {
    contentEl.textContent = `→ "${query}"\n\nAI routing failed. Is your API key valid?`;
  }
}

window.runSuggestion = function(btn) {
  launcherInput.value = btn.textContent;
  handleQuery(btn.textContent);
};

// ── 5. AI status (status bar button) ─────────────────────────────────────
const aiPill = document.getElementById('ai-pill');
const aiPillText = document.getElementById('ai-pill-text');
let lastAiStatus = null;

// One plain-language line for where answers come from right now.
function describeAi(status) {
  if (!status) return ['bad', 'AI core not running'];
  const cloud = (status.cloud_fallbacks || []).length > 0;
  if (status.mode === 'cloud' && cloud) return ['ok', 'Gemini'];
  if (status.local && status.local.reachable) return ['ok', 'On this computer'];
  if (cloud) return ['ok', 'Gemini'];
  return ['wait', 'Model starting…'];
}

async function pollAi() {
  let status = null;
  try { status = await tauriInvoke('ai_status'); } catch { /* daemon down */ }
  lastAiStatus = status;
  const [state, text] = describeAi(status);
  aiPill.dataset.state = state;
  aiPillText.textContent = text;
  const ccAiSub = document.getElementById('cc-ai-sub');
  const ccAiTile = document.getElementById('cc-ai');
  if (ccAiSub) ccAiSub.textContent = text;
  if (ccAiTile) ccAiTile.classList.toggle('active', state === 'ok');
  return status;
}
pollAi();
setInterval(pollAi, 5000);

// ── 5b. Control Center (quick settings flyout) ────────────────────────────
const controlToggle = document.getElementById('control-toggle');
const controlCenter = document.getElementById('control-center');
const brightnessOverlay = document.getElementById('brightness-overlay');
const nightOverlay = document.getElementById('night-overlay');

function toggleControlCenter(force) {
  const show = force !== undefined ? force : controlCenter.classList.contains('cc-hidden');
  controlCenter.classList.toggle('cc-hidden', !show);
}
if (controlToggle) {
  controlToggle.addEventListener('click', (e) => { e.stopPropagation(); toggleControlCenter(); });
  // Close when clicking outside the panel.
  document.addEventListener('click', (e) => {
    if (!controlCenter.classList.contains('cc-hidden') &&
        !controlCenter.contains(e.target) && e.target !== controlToggle) {
      toggleControlCenter(false);
    }
  });
  window.addEventListener('keydown', (e) => { if (e.code === 'Escape') toggleControlCenter(false); });

  // Brightness — genuinely dims the screen via a black overlay.
  const brightness = document.getElementById('cc-brightness');
  const applyBrightness = () => {
    const v = parseInt(brightness.value, 10);
    brightnessOverlay.style.opacity = ((100 - v) / 100 * 0.72).toFixed(3);
  };
  brightness.addEventListener('input', applyBrightness);
  applyBrightness();

  // Volume — cosmetic in a VM (no audio device); still updates the icon.
  const volume = document.getElementById('cc-volume');
  const volIco = document.getElementById('ctl-vol');
  volume.addEventListener('input', () => {
    const v = parseInt(volume.value, 10);
    volIco.textContent = v === 0 ? '🔇' : v < 50 ? '🔉' : '🔊';
  });

  // Night light — warm overlay.
  const nightTile = document.getElementById('cc-night');
  nightTile.addEventListener('click', () => {
    const on = nightTile.classList.toggle('active');
    nightOverlay.classList.toggle('on', on);
    nightTile.querySelector('.cc-tile-sub').textContent = on ? 'On' : 'Off';
  });

  // Airplane / Wi-Fi tiles — cosmetic (VM uses wired NAT) + Live SSID
  const airplane = document.getElementById('cc-airplane');
  const wifi = document.getElementById('cc-wifi');
  airplane.addEventListener('click', () => {
    const on = airplane.classList.toggle('active');
    airplane.querySelector('.cc-tile-sub').textContent = on ? 'On' : 'Off';
    wifi.classList.toggle('active', !on);
    document.getElementById('cc-net-sub').textContent = on ? 'Off' : 'Connected';
    document.getElementById('ctl-net').textContent = on ? '✈' : '🌐';
  });

  // Shutdown button
  const shutdownBtn = document.getElementById('cc-shutdown');
  if (shutdownBtn) {
    shutdownBtn.addEventListener('click', async () => {
      await tauriInvoke('shutdown_os');
    });
  }

  // Fetch real Wi-Fi network periodically
  const fetchWifi = async () => {
    try {
      const ssid = await tauriInvoke('get_wifi_network');
      const netSub = document.getElementById('cc-net-sub');
      if (netSub && !airplane.classList.contains('active')) {
        netSub.textContent = ssid;
      }
    } catch (e) {}
  };
  fetchWifi();
  setInterval(fetchWifi, 5000);

  // Clock in the footer.
  const cc2 = document.getElementById('cc-clock2');
  const tickCC = () => { cc2.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }); };
  tickCC(); setInterval(tickCC, 1000);
}

// ── Utility ───────────────────────────────────────────────────────────────
function formatBytes(bytes) {
  if (bytes >= 1073741824) return (bytes / 1073741824).toFixed(1) + 'GB';
  if (bytes >= 1048576)    return (bytes / 1048576).toFixed(0) + 'MB';
  return (bytes / 1024).toFixed(0) + 'KB';
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Lua-driven Window Manager + Desktop Dock
// ═══════════════════════════════════════════════════════════════════════════

let WM = null;

const dockApps = document.getElementById('dock-apps');
const dockWindows = document.getElementById('dock-windows');
const dockWorkspaces = document.getElementById('dock-workspaces');
const dockLayout = document.getElementById('dock-layout');

const ICONS = {
  assistant: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2L15 8.5L22 9.5L17 14L18.5 21L12 17.5L5.5 21L7 14L2 9.5L9 8.5L12 2Z"/></svg>`,
  browser: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/><path d="M2 12h20"/></svg>`,
  terminal: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>`,
  files: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>`,
  monitor: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="M18.7 8l-5.1 5.2-2.8-2.7L7 14.3"/></svg>`,
  taskmanager: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>`,
  knowledge: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>`,
  behavior: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24 2.5 2.5 0 0 1 1.98-3A2.5 2.5 0 0 1 9.5 2Z"/><path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24 2.5 2.5 0 0 0-1.98-3A2.5 2.5 0 0 0 14.5 2Z"/></svg>`,
  config: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`,
  console: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><polyline points="7 9 10 12 7 15"/><line x1="12" y1="15" x2="17" y2="15"/></svg>`,
  about: `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>`
};

const APP_LAUNCHERS = [
  { id: 'assistant', icon: ICONS.terminal, label: 'Assistant' },
  { id: 'files', icon: ICONS.files, label: 'Files' },
  { id: 'browser', icon: ICONS.browser, label: 'Browser' },
  { id: 'monitor', icon: ICONS.monitor, label: 'System monitor' },
  { id: 'taskmanager', icon: ICONS.taskmanager, label: 'Running programs' },
  { id: 'terminal', icon: ICONS.console, label: 'Terminal' },
  { id: 'knowledge', icon: ICONS.knowledge, label: 'Knowledge' },
  { id: 'behavior', icon: ICONS.behavior, label: 'What OSAIMA has learned' },
  { id: 'config', icon: ICONS.config, label: 'Window rules' },
  { id: 'about', icon: ICONS.about, label: 'About' },
];

// ── Desktop notifications ─────────────────────────────────────────────────
const notificationsEl = document.getElementById('notifications');
// opts: { actionLabel, onAction, timeout }
function pushNotification(message, kind = 'info', opts = {}) {
  if (!notificationsEl) return;
  const toast = document.createElement('div');
  toast.className = 'toast toast-' + kind;

  const text = document.createElement('div');
  text.className = 'toast-text';
  text.textContent = message;
  toast.appendChild(text);

  const dismiss = () => {
    toast.classList.remove('show');
    setTimeout(() => toast.remove(), 300);
  };

  if (opts.actionLabel && opts.onAction) {
    const btn = document.createElement('button');
    btn.className = 'toast-action';
    btn.textContent = opts.actionLabel;
    btn.addEventListener('click', () => { opts.onAction(); dismiss(); });
    toast.appendChild(btn);
  }

  notificationsEl.appendChild(toast);
  requestAnimationFrame(() => toast.classList.add('show'));
  setTimeout(dismiss, opts.timeout || (opts.actionLabel ? 7000 : 3200));
}

function renderDockApps() {
  dockApps.innerHTML = '';
  for (const app of APP_LAUNCHERS) {
    const btn = document.createElement('button');
    btn.className = 'dock-app';
    btn.title = app.label;
    btn.setAttribute('aria-label', app.label);
    btn.dataset.label = app.label;
    btn.innerHTML = `<span class="dock-app-icon">${app.icon}</span>`;
    btn.addEventListener('click', () => WM && WM.spawn(app.id));
    dockApps.appendChild(btn);
  }
}

function renderDockWindows() {
  if (!WM) return;
  dockWindows.innerHTML = '';
  const wins = WM.windows.filter((w) => w.workspace === WM.currentWs);
  for (const w of wins) {
    const pill = document.createElement('button');
    pill.className = 'dock-win' +
      (w.id === WM.focusedId ? ' active' : '') +
      (w.minimized ? ' minimized' : '');
    pill.textContent = w.title;
    pill.addEventListener('click', () => {
      if (w.minimized) WM.restore(w.id);
      else if (w.id === WM.focusedId) WM.minimize(w.id);
      else WM.focus(w.id);
      renderDockWindows();
    });
    dockWindows.appendChild(pill);
  }
}

function renderDockWorkspaces(current, workspaces) {
  dockWorkspaces.innerHTML = '';
  workspaces.forEach((ws, i) => {
    const pill = document.createElement('button');
    const occupied = WM && WM.windowsOn(i).length > 0;
    pill.className = 'dock-ws' +
      (i === current ? ' active' : '') +
      (occupied ? ' occupied' : '');
    pill.textContent = String(i + 1);
    pill.title = `Workspace ${i + 1} (${ws.name})`;
    pill.addEventListener('click', () => WM && WM.switchWorkspace(i));
    dockWorkspaces.appendChild(pill);
  });
}

// Spawn a window from the AI launcher, then close the launcher.
function spawnFromLauncher(appId, contentEl, label) {
  if (WM) {
    WM.spawn(appId);
    contentEl.textContent = `→ Opened ${label}.`;
    setTimeout(closeLauncher, 250);
  } else {
    contentEl.textContent = 'Window manager still starting…';
  }
}

// Behavior-learning store + RAG knowledge engine (PDR subsystems §3.5 / §3.7).
const behavior = new BehaviorStore();
const rag = new RagEngine();
for (const doc of SEED_KNOWLEDGE) rag.ingest(doc);
rag.buildIndex();

(async function bootWM() {
  const surface = document.getElementById('wm-surface');
  const { engine, reload, services } = await initWindowManager({
    surface,
    invoke: tauriInvoke,
    rag,
    behavior,
    hooks: {
      onFocusChange: () => renderDockWindows(),
      onWindowsChange: () => {
        if (!WM) return; // hooks can fire during boot before WM is assigned
        renderDockWindows();
        renderDockWorkspaces(WM.currentWs, WM.workspaces);
      },
      onSpawn: (win) => behavior.recordAppLaunch(win.appId),
      onLayoutChange: (name) => { dockLayout.textContent = name; behavior.recordLayout(name); },
      onWorkspaceChange: (cur, wss) => { renderDockWorkspaces(cur, wss); renderDockWindows(); behavior.recordWorkspace(cur); },
      onNotify: (msg, kind) => pushNotification(msg, kind),
    },
  });
  WM = engine;
  window.WM = engine;       // handy for debugging/demo from the console
  window.SERVICES = services;
  window.reloadWM = reload; // reload the Lua config programmatically

  // Initialize VoxType Speech-to-Text Engine
  const voxtype = initVoxType({
    engine,
    services: { ...services, invoke: tauriInvoke }
  });
  window.VOXTYPE = voxtype;
  services.voxtype = voxtype;

  renderDockApps();
  renderDockWindows();
  renderDockWorkspaces(engine.currentWs, engine.workspaces);
  dockLayout.textContent = engine.config.layout;
  dockLayout.addEventListener('click', () => engine.cycleLayout());

  // Proactive AI Core — watches the system + learned behavior, offers suggestions.
  const agent = new AgentCore({
    engine,
    invoke: tauriInvoke,
    behavior,
    suggest: (msg, opts) => {
      behavior.recordSuggestionShown();
      pushNotification(msg, 'agent', {
        ...opts,
        onAction: () => { behavior.recordSuggestionAccepted(); if (opts.onAction) opts.onAction(); },
      });
    },
  });
  agent.start();
  window.AGENT = agent;
  window.RAG = rag;
  window.BEHAVIOR = behavior;

  // ── API Key Setup Check ──
  const isWindows = navigator.platform.indexOf('Win') > -1 || navigator.userAgent.indexOf('Windows') > -1;
  if (isWindows) {
    const winModal = document.getElementById('windows-mock-modal');
    if (winModal) {
      winModal.classList.remove('hidden');
      const winClose = document.getElementById('windows-mock-close');
      if (winClose) winClose.addEventListener('click', () => winModal.classList.add('hidden'));
    }
  }

  // ── Welcome (first start) and AI settings ──
  // Keys used to live in localStorage; the daemon now keeps them in a private
  // file, so forget any copy the browser still holds.
  localStorage.removeItem('osaima_gemini_api_key');
  localStorage.removeItem('osaima_groq_api_key');
  const modal = document.getElementById('api-key-modal');
  const keyBox = document.getElementById('cloud-key');
  const keyInput = document.getElementById('api-key-input');
  const startBtn = document.getElementById('api-key-close');
  const statusList = document.getElementById('ai-status-list');
  const modeInputs = [...document.querySelectorAll('input[name="ai-mode"]')];
  const chosenMode = () => (modeInputs.find((i) => i.checked) || {}).value;

  const renderStatus = (status) => {
    const local = status?.local || {};
    const cloud = (status?.cloud_fallbacks || []).length > 0;
    const rows = [
      [status ? 'ok' : 'bad', status ? 'AI core is running' : "AI core isn't running"],
      [status?.router?.reachable ? 'ok' : 'bad', status?.router?.reachable ? 'Instant commands are ready' : "Instant commands aren't available"],
      chosenMode() === 'local'
        ? [local.reachable ? 'ok' : 'wait', local.reachable ? 'On-device model is ready' : 'On-device model is starting (about a minute)']
        : [cloud ? 'ok' : 'wait', cloud ? 'Gemini is connected' : 'Add a Gemini key to use Fast mode'],
    ];
    statusList.replaceChildren(...rows.map(([state, label]) => {
      const li = document.createElement('li');
      li.className = state;
      li.textContent = label;
      return li;
    }));
  };
  const syncForm = () => {
    const cloud = (lastAiStatus?.cloud_fallbacks || []).length > 0;
    keyBox.hidden = chosenMode() !== 'cloud' || cloud;
    renderStatus(lastAiStatus);
  };
  modeInputs.forEach((i) => i.addEventListener('change', syncForm));

  async function openSettings() {
    const status = await pollAi();
    const mode = status?.mode || 'cloud';
    modeInputs.forEach((i) => { i.checked = i.value === mode; });
    keyInput.value = '';
    syncForm();
    modal.classList.remove('hidden');
  }
  window.openAiSettings = openSettings;
  aiPill.addEventListener('click', openSettings);

  // Keep the status rows fresh while the dialog is open.
  setInterval(async () => {
    if (!modal.classList.contains('hidden')) { await pollAi(); renderStatus(lastAiStatus); }
  }, 4000);

  startBtn.addEventListener('click', async () => {
    const mode = chosenMode() || 'cloud';
    const key = keyInput.value.trim();
    startBtn.disabled = true;
    try {
      if (mode === 'cloud' && key) {
        startBtn.textContent = 'Checking the key…';
        await tauriInvoke('validate_api_key', { key });
        await tauriInvoke('set_api_key', { key });
      }
      await tauriInvoke('set_llm_mode', { mode });
      localStorage.setItem('osaima_welcomed', '1');
      modal.classList.add('hidden');
      await pollAi();
      window.dispatchEvent(new Event('osaima:focus'));
    } catch (err) {
      pushNotification("That key didn't work: " + (err && err.message ? err.message : err), 'err', { timeout: 7000 });
    }
    startBtn.textContent = 'Start';
    startBtn.disabled = false;
  });
  keyInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') startBtn.click(); });

  if (!localStorage.getItem('osaima_welcomed')) openSettings();

  // ── Test Hub Wiring ──
  document.getElementById('test-btn-daemon').onclick = async () => {
    try {
      const alive = await tauriInvoke('ping_daemon');
      pushNotification(alive ? 'AI Daemon is ALIVE!' : 'AI Daemon is offline.', alive ? 'ok' : 'err');
    } catch (e) {
      pushNotification('Daemon error: ' + e, 'err');
    }
  };
  
  document.getElementById('test-btn-list-dir').onclick = async () => {
    try {
      const data = await tauriInvoke('read_dir', { path: '/' });
      console.log('Dir entries:', data.entries);
      pushNotification(`Success: Found ${data.entries.length} items in root.`, 'ok');
    } catch (e) {
      pushNotification('Read dir failed: ' + e, 'err');
    }
  };

  document.getElementById('test-btn-sys-stats').onclick = async () => {
    try {
      const s = await tauriInvoke('get_system_stats');
      pushNotification(`CPU: ${s.cpu.usage_percent.toFixed(1)}% | Mem: ${s.memory.used_bytes} bytes`, 'ok');
    } catch (e) {
      pushNotification('Failed to fetch stats: ' + e, 'err');
    }
  };

  document.getElementById('test-btn-spawn-term').onclick = () => {
    engine.spawn('terminal');
    pushNotification('Terminal window spawned via Test Hub', 'info');
  };

  document.getElementById('test-btn-notification').onclick = () => {
    pushNotification('This is a test notification from the OS shell!', 'agent', { actionLabel: 'Click Me', onAction: () => console.log('Action clicked!') });
  };

  document.getElementById('test-btn-rag').onclick = () => {
    const res = rag.answer('What layout does the WM use by default?');
    pushNotification(`RAG says: ${res.answer}`, 'info', { timeout: 10000 });
  };

  // Draggable test hub: a developer tool, hidden unless ?dev is in the URL or
  // localStorage.osaima_dev is "1"; Ctrl+Shift+D toggles it.
  const testHub = document.getElementById('test-hub');
  const hubHeader = document.getElementById('test-hub-header');
  const devMode = new URLSearchParams(location.search).has('dev') || localStorage.getItem('osaima_dev') === '1';
  if (!devMode) testHub.style.display = 'none';
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'd') {
      e.preventDefault();
      testHub.style.display = testHub.style.display === 'none' ? '' : 'none';
    }
  });
  let isDraggingHub = false, hubX = 0, hubY = 0;
  hubHeader.onmousedown = (e) => {
    isDraggingHub = true;
    hubX = e.clientX - testHub.offsetLeft;
    hubY = e.clientY - testHub.offsetTop;
  };
  document.addEventListener('mousemove', (e) => {
    if (isDraggingHub) {
      testHub.style.left = (e.clientX - hubX) + 'px';
      testHub.style.top = (e.clientY - hubY) + 'px';
    }
  });
  document.addEventListener('mouseup', () => { isDraggingHub = false; });
})();

// ── Fullscreen Toggle ──
setTimeout(() => {
  const fsBtn = document.getElementById('fullscreen-toggle');
  if (fsBtn) {
    fsBtn.addEventListener('click', async () => {
      try {
        if (!document.fullscreenElement) {
          await document.documentElement.requestFullscreen();
        } else {
          if (document.exitFullscreen) {
            await document.exitFullscreen();
          }
        }
      } catch (err) {
        console.warn('Failed to toggle HTML5 fullscreen:', err);
      }
    });
  }
}, 500);
