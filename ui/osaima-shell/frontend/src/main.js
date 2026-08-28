/**
 * osaima-shell — main.js
 * Interstellar OS Agentic UI Shell
 *
 * Responsibilities:
 *  1. Animated starfield canvas
 *  2. Live clock
 *  3. AI Command Hub (launcher) open/close + keyboard nav
 *  4. MCP Daemon polling via Tauri IPC (with graceful fallback)
 *  5. System stats display
 */

import { initWindowManager } from './wm/boot.js';
import { AgentCore } from './wm/agent-core.js';
import { RagEngine, SEED_KNOWLEDGE } from './wm/rag.js';
import { BehaviorStore } from './wm/behavior.js';

// ── Tauri invoke (safe import for non-Tauri dev environments) ──────────────
let tauriInvoke = null;
try {
  const tauri = await import('@tauri-apps/api/core');
  tauriInvoke = tauri.invoke;
} catch {
  // Running in browser dev mode — use mock data
  tauriInvoke = async (cmd) => {
    if (cmd === 'get_system_stats') {
      return {
        os: 'Interstellar OS',
        kernel: '6.x.x-interstellar',
        host: 'interstellar-dev',
        memory: { total_bytes: 17179869184, used_bytes: 5368709120 },
        cpu: { usage_percent: 12.5, cores: 8 },
      };
    }
    if (cmd === 'ping_daemon') return false;
  };
}

// ── 1. Starfield Canvas ───────────────────────────────────────────────────
const canvas = document.getElementById('starfield');
const ctx = canvas.getContext('2d');
let stars = [];

function resizeCanvas() {
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;
}

function initStars(count = 320) {
  stars = Array.from({ length: count }, () => ({
    x: Math.random() * canvas.width,
    y: Math.random() * canvas.height,
    r: Math.random() * 1.2 + 0.2,
    alpha: Math.random() * 0.6 + 0.1,
    speed: Math.random() * 0.015 + 0.003,
    drift: (Math.random() - 0.5) * 0.06,
  }));
}

function drawStars() {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  for (const s of stars) {
    ctx.beginPath();
    ctx.arc(s.x, s.y, s.r, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(200, 220, 255, ${s.alpha})`;
    ctx.fill();
    // Twinkle
    s.alpha += Math.sin(Date.now() * s.speed) * 0.002;
    s.alpha = Math.max(0.05, Math.min(0.85, s.alpha));
    s.x += s.drift;
    if (s.x > canvas.width + 2)  s.x = -2;
    if (s.x < -2) s.x = canvas.width + 2;
  }
  requestAnimationFrame(drawStars);
}

window.addEventListener('resize', () => { resizeCanvas(); initStars(); });
resizeCanvas();
initStars();
drawStars();

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

function openLauncher() {
  launcher.classList.remove('hidden');
  requestAnimationFrame(() => launcherInput.focus());
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
  } else if (lower.includes('assistant') || lower.includes('agent') || lower.includes('ai ')) {
    spawnFromLauncher('assistant', contentEl, 'the AI Assistant');
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
  } else if (lower.includes('about') || lower.includes('keybind') || lower.includes('help')) {
    spawnFromLauncher('about', contentEl, 'About / keybindings');
  } else if (lower.includes('brightness')) {
    contentEl.textContent = '→ Display brightness control:\n(Direct hardware control via udev — planned for Phase 4)';
  } else if (lower.includes('network') || lower.includes('diagnostic')) {
    contentEl.textContent = '→ Network diagnostics:\n(NetworkManager IPC integration — planned for Phase 4)';
  } else {
    contentEl.textContent = `→ "${query}"\n\nAI routing is active. Full LLM integration via\nagentic-services and MCP planned in Phase 4.`;
  }
}

window.runSuggestion = function(btn) {
  launcherInput.value = btn.textContent;
  handleQuery(btn.textContent);
};

// ── 5. MCP System Stats Polling ───────────────────────────────────────────
async function pollStats() {
  try {
    const stats = await tauriInvoke('get_system_stats');
    const alive  = await tauriInvoke('ping_daemon');

    // Status bar pills
    document.getElementById('cpu-val').textContent =
      stats.cpu.usage_percent.toFixed(0) + '%';
    document.getElementById('mem-val').textContent =
      formatBytes(stats.memory.used_bytes);

    // Context panel
    document.getElementById('ctx-host').textContent   = stats.host;
    document.getElementById('ctx-kernel').textContent = stats.kernel;
    document.getElementById('ctx-cores').textContent  = stats.cpu.cores;

    const memPct = (stats.memory.used_bytes / stats.memory.total_bytes) * 100;
    document.getElementById('ctx-mem-bar').textContent =
      formatBytes(stats.memory.used_bytes) + ' / ' + formatBytes(stats.memory.total_bytes);
    document.getElementById('mem-bar-fill').style.width = memPct.toFixed(1) + '%';

    // Daemon indicator
    const indicator = document.getElementById('daemon-indicator');
    const statusText = document.getElementById('daemon-status-text');
    if (alive) {
      indicator.className = 'indicator online';
      statusText.textContent = 'AI Core online';
      statusText.className = 'online';
    } else {
      indicator.className = 'indicator offline';
      statusText.textContent = 'AI Core offline';
      statusText.className = '';
    }
  } catch (err) {
    console.warn('Stats poll failed:', err);
  }
}

// Poll immediately, then every 3 seconds
pollStats();
setInterval(pollStats, 3000);

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
const ambientCenter = document.getElementById('ambient-center');

const APP_LAUNCHERS = [
  { id: 'assistant', icon: '✦', label: 'AI Assistant' },
  { id: 'terminal', icon: '❯', label: 'Terminal' },
  { id: 'files', icon: '🗂', label: 'Files' },
  { id: 'monitor', icon: '📊', label: 'Monitor' },
  { id: 'taskmanager', icon: '⚡', label: 'Task Manager' },
  { id: 'knowledge', icon: '📚', label: 'Knowledge (RAG)' },
  { id: 'behavior', icon: '🧠', label: 'Behavior Profile' },
  { id: 'config', icon: '⚙', label: 'wm.lua' },
  { id: 'about', icon: '✧', label: 'About' },
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
  // Hide the ambient hint once there's something on screen.
  const anyVisible = wins.some((w) => !w.minimized);
  ambientCenter.style.opacity = anyVisible ? '0' : '';
}

function renderDockWorkspaces(current, workspaces) {
  dockWorkspaces.innerHTML = '';
  workspaces.forEach((ws, i) => {
    const pill = document.createElement('button');
    const occupied = WM && WM.windowsOn(i).length > 0;
    pill.className = 'dock-ws' +
      (i === current ? ' active' : '') +
      (occupied ? ' occupied' : '');
    pill.textContent = ws.name;
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
  const { engine, reload } = await initWindowManager({
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
  window.reloadWM = reload; // reload the Lua config programmatically
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
})();
