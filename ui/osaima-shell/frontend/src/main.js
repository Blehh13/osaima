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
import { tauriTransport } from './wm/agent-client.js';
import { demoTransport } from './wm/agent-demo.js';
import { askAssistant } from './wm/assistant.js';

// ── Backend IPC ───────────────────────────────────────────────────────────
// Inside Tauri (`withGlobalTauri`), calls go to the Rust host and the AI Core.
// In a plain browser the shell runs in clearly-labelled demo mode on mock data.
const tauriInvoke = window.__TAURI__?.core?.invoke ?? createDemoBackend();
if (!window.__TAURI__) document.body.classList.add('demo-mode');

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

  // Short commands ("open terminal") are shortcuts; anything longer is a request
  // for the assistant, which can look into the system and act on it.
  const lower = query.toLowerCase();
  if (query.trim().split(/\s+/).length > 3) return sendToAssistant(query, contentEl);

  contentEl.textContent = '⠋ Processing…';
  await new Promise(r => setTimeout(r, 200)); // brief pause so the change is visible

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
      : '✗ AI Core is offline. Start it with: osaima-mcp-daemon &';
  } else if (lower.includes('assistant') || lower.includes('agent') || lower.includes('ai ')) {
    spawnFromLauncher('assistant', contentEl, 'the AI Assistant');
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
  } else if (lower.includes('about') || lower.includes('keybind') || lower.includes('help')) {
    spawnFromLauncher('about', contentEl, 'About / keybindings');
  } else {
    sendToAssistant(query, contentEl);
  }
}

// Hand a request to the AI Assistant window and get the launcher out of the way.
function sendToAssistant(query, contentEl) {
  if (!WM) {
    contentEl.textContent = 'The desktop is still starting…';
    return;
  }
  contentEl.textContent = '→ Asking the assistant…';
  askAssistant(WM, query);
  setTimeout(closeLauncher, 200);
}

function runSuggestion(btn) {
  launcherInput.value = btn.textContent;
  handleQuery(btn.textContent);
}
document.getElementById('launcher-backdrop').addEventListener('click', closeLauncher);
document.getElementById('launcher-close').addEventListener('click', closeLauncher);
for (const chip of document.querySelectorAll('.suggestion-chip')) {
  chip.addEventListener('click', () => runSuggestion(chip));
}

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

    // Mirror live status into the Control Center flyout.
    const ccAiSub = document.getElementById('cc-ai-sub');
    const ccAiTile = document.getElementById('cc-ai');
    if (ccAiSub) ccAiSub.textContent = alive ? 'Online' : 'Offline';
    if (ccAiTile) ccAiTile.classList.toggle('active', !!alive);
  } catch (err) {
    console.warn('Stats poll failed:', err);
  }
}

// Poll immediately, then every 3 seconds
pollStats();
setInterval(pollStats, 3000);

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

  // Airplane / Wi-Fi tiles — cosmetic (VM uses wired NAT).
  const airplane = document.getElementById('cc-airplane');
  const wifi = document.getElementById('cc-wifi');
  airplane.addEventListener('click', () => {
    const on = airplane.classList.toggle('active');
    airplane.querySelector('.cc-tile-sub').textContent = on ? 'On' : 'Off';
    wifi.classList.toggle('active', !on);
    document.getElementById('cc-net-sub').textContent = on ? 'Off' : 'Connected';
    document.getElementById('ctl-net').textContent = on ? '✈' : '🌐';
  });

  // Clock in the footer.
  const cc2 = document.getElementById('cc-clock2');
  const tickCC = () => { cc2.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }); };
  tickCC(); setInterval(tickCC, 1000);
}

// ── Utility ───────────────────────────────────────────────────────────────
/** Mock backend for browser previews. Nothing here touches a real system. */
function createDemoBackend() {
  const DEMO_PROCS = ['osaima-shell', 'mcp-daemon', 'firefox', 'foot', 'pipewire', 'sway', 'ollama', 'python3'];
  return async (cmd, args = {}) => {
    switch (cmd) {
      case 'get_system_stats': {
        const total = 16 * 1024 ** 3;
        return {
          os: 'Interstellar OS (demo)', os_version: '0.2', kernel: '6.x-demo', host: 'demo',
          uptime_secs: Math.floor(performance.now() / 1000), load_average: [0.4, 0.3, 0.2],
          cpu: { usage_percent: 8 + Math.random() * 20, cores: 8, brand: 'Demo CPU' },
          memory: { total_bytes: total, used_bytes: total * (0.3 + Math.random() * 0.05),
            available_bytes: total * 0.65, swap_total_bytes: 0, swap_used_bytes: 0 },
        };
      }
      case 'ping_daemon': return false;
      case 'list_processes':
        return { processes: DEMO_PROCS.map((name, i) => ({
          pid: 1000 + i * 37, name, cpu_percent: Math.random() * 30 / (i + 1),
          memory_bytes: (400 - i * 40) * 1024 ** 2, uid: 1000, status: 'Sleep',
        })) };
      case 'home_dir': return '/home/demo';
      case 'read_dir':
        return { path: args.path || '/home/demo', entries: [
          { name: 'Documents', is_dir: true }, { name: 'Downloads', is_dir: true }, { name: 'notes.txt', is_dir: false },
        ] };
      case 'run_command':
        return { output: 'Demo mode: commands only run inside Interstellar OS.\n', exit_code: 1, timed_out: false, truncated: false };
      default:
        throw new Error(`'${cmd}' is unavailable in demo mode`);
    }
  };
}

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
  { id: 'browser', icon: '🌐', label: 'Browser' },
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
    agentTransport: window.__TAURI__ ? tauriTransport(window.__TAURI__) : demoTransport(),
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
