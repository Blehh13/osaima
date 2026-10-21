/**
 * apps.js — built-in applications for the OSAIMA shell.
 *
 * Each app is a small definition the window manager can spawn. Apps get a
 * content element to render into plus a context ({ wm, win }). A shared
 * `services` object wires them to the rest of the shell (MCP IPC, the live Lua
 * config, etc.).
 *
 * Apps kept intentionally lightweight and self-contained so the WM has real,
 * varied windows to tile, float, and move across workspaces during a demo.
 */

import { createVoice } from './voice-ui.js';

/**
 * @param {import('./window-manager.js').WindowManager} engine
 * @param {object} services  { invoke, getConfigSource, setConfigSource, reloadConfig }
 */
export function registerApps(engine, services) {
  engine.registerApp(makeTerminal(services));
  engine.registerApp(makeBrowser());
  engine.registerApp(makeMonitor(services));
  engine.registerApp(makeFiles(services));
  engine.registerApp(makeConfigEditor(engine, services));
  engine.registerApp(makeAbout(engine));
  engine.registerApp(makeAssistant(engine, services));
  engine.registerApp(makeTaskManager(services));
  engine.registerApp(makeKnowledge(services));
  engine.registerApp(makeBehavior(engine, services));
  engine.registerApp(makeCalendar(services));
  engine.registerApp(makeNotes(services));
}

// ── Terminal (PTY Emulator) ──────────────────────────────────────────────────────────────
function makeTerminal(services) {
  return {
    id: 'terminal',
    title: 'Terminal',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>',
    width: 600, height: 400,
    async mount(root, ctx) {
      root.classList.add('app-terminal');
      root.style.backgroundColor = '#040914';
      
      const termContainer = document.createElement('div');
      termContainer.style.width = '100%';
      termContainer.style.height = '100%';
      termContainer.style.padding = '8px';
      root.appendChild(termContainer);

      if (!window.Terminal || !window.FitAddon) {
        termContainer.textContent = "Error: xterm.js not loaded.";
        return;
      }

      const term = new window.Terminal({
        cursorBlink: true,
        fontFamily: 'JetBrains Mono, monospace',
        fontSize: 13,
        theme: {
          background: '#040914',
          foreground: '#dcebff'
        }
      });
      const fitAddon = new window.FitAddon.FitAddon();
      term.loadAddon(fitAddon);
      term.open(termContainer);
      
      const winId = String(ctx.win.id || Math.random().toString(36).slice(2));

      // Resolve Tauri v2 APIs
      const T = window.__TAURI__;
      const invoke = (T && (T.core?.invoke || T.invoke || T.tauri?.invoke)) || services?.invoke;
      const listen = T && (T.event?.listen || T.core?.listen);

      if (!invoke) {
        term.write('\x1b[31m✗ System invoke API not found. Terminal is offline.\x1b[0m\r\n');
        return;
      }

      termContainer._xterm = term;
      ctx.win.term = term;

      let ptyActive = false;
      if (listen) {
        try {
          await invoke('spawn_pty', { id: winId });
          ptyActive = true;
          termContainer._insertText = async (data) => {
            try { await invoke('write_pty', { id: winId, data }); } catch (e) {}
          };
          await listen('pty-out-' + winId, (ev) => {
            const data = typeof ev === 'string' ? ev : (ev.payload || ev);
            term.write(data);
          });
          term.onData(async (data) => {
            try { await invoke('write_pty', { id: winId, data }); } catch (e) {}
          });
          term.onResize(async (size) => {
            try { await invoke('resize_pty', { id: winId, rows: size.rows, cols: size.cols }); } catch (e) {}
          });
        } catch (e) {
          ptyActive = false;
        }
      }

      if (!ptyActive) {
        // Fallback: interactive Web Terminal connected to run_command
        // Real user@host, filled in once the host answers; generic until then.
        let promptStr = '\r\n\x1b[1;35muser@osaima\x1b[0m:\x1b[1;34m~\x1b[0m$ ';
        term.write('\x1b[1;36m✦ OSAIMA Shell Terminal (Live Mode)\x1b[0m\r\n');
        term.write('System commands wired to host. Type \x1b[33mhelp\x1b[0m or any Linux command.\r\n');
        try {
          const who = String(await invoke('run_command', { cmd: 'echo "$(whoami)@$(hostname)"' })).trim();
          if (/^[\w.-]+@[\w.-]+$/.test(who)) promptStr = `\r\n\x1b[1;35m${who}\x1b[0m:\x1b[1;34m~\x1b[0m$ `;
        } catch (e) { /* keep the generic prompt */ }
        term.write(promptStr);

        let inputBuffer = '';
        termContainer._insertText = (text) => {
          inputBuffer += text;
          term.write(text);
        };
        term.onData(async (data) => {
          if (data === '\r') {
            term.write('\r\n');
            const cmd = inputBuffer.trim();
            inputBuffer = '';
            if (cmd) {
              if (cmd === 'clear') {
                term.clear();
                term.write(promptStr);
                return;
              }
              try {
                const res = await invoke('run_command', { cmd });
                if (res) {
                  term.write(String(res).replace(/\r?\n/g, '\r\n') + '\r\n');
                }
              } catch (err) {
                term.write('\x1b[31mError: ' + err + '\x1b[0m\r\n');
              }
            }
            term.write(promptStr);
          } else if (data === '\u007F' || data === '\b') {
            if (inputBuffer.length > 0) {
              inputBuffer = inputBuffer.slice(0, -1);
              term.write('\b \b');
            }
          } else if (data >= ' ' || data === '\t') {
            inputBuffer += data;
            term.write(data);
          }
        });
      }

      // Handle resize
      term.onResize(async (size) => {
        try {
          await invoke('resize_pty', { id: winId, rows: size.rows, cols: size.cols });
        } catch (e) {}
      });

      fitAddon.fit();

      const ro = new ResizeObserver(() => {
        fitAddon.fit();
      });
      ro.observe(termContainer);

      setTimeout(() => { fitAddon.fit(); term.focus(); }, 100);
    },
  };
}

// ── Web Browser ───────────────────────────────────────────────────────────────
// A real web browser inside the shell: address bar + iframe view + a start page
// with bookmarks. It's a WebKit webview, so this is genuine web browsing.
function makeBrowser() {
  const HOME = 'about:home';
  const BOOKMARKS = [
    { name: 'Google', url: 'https://www.google.com/search?igu=1' },
    { name: 'Gentoo', url: 'https://www.gentoo.org' },
    { name: 'OSAIMA · GitHub', url: 'https://github.com/akashmanjunath2505/osaima' },
    { name: 'MDN Web Docs', url: 'https://developer.mozilla.org' },
    { name: 'Hacker News', url: 'https://news.ycombinator.com' },
    { name: 'archive.org', url: 'https://archive.org' },
  ];
  return {
    id: 'browser',
    title: 'Browser',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/><path d="M2 12h20"/></svg>',
    width: 760, height: 540,
    mount(root, ctx) {
      root.classList.add('app-browser');
      const bar = el('div', 'br-bar');
      const back = el('button', 'br-nav', '‹');
      const fwd = el('button', 'br-nav', '›');
      const reload = el('button', 'br-nav', '⟳');
      const homeBtn = el('button', 'br-nav', '⌂');
      const url = document.createElement('input');
      url.className = 'br-url';
      url.spellcheck = false;
      url.autocomplete = 'off';
      url.placeholder = 'Search or enter address';
      const go = el('button', 'br-go', 'Go');
      bar.append(back, fwd, reload, homeBtn, url, go);

      const view = el('div', 'br-view');
      const dummy = document.createElement('div');
      dummy.className = 'br-frame';
      dummy.style.width = '100%';
      dummy.style.height = '100%';
      dummy.style.background = 'transparent'; // Must be transparent to see webview if it renders behind
      const startPage = el('div', 'br-home');
      view.append(startPage, dummy);
      root.append(bar, view);

      const { Webview } = window.__TAURI__.webview;
      const { Window } = window.__TAURI__.window;
      const wvLabel = 'browser-' + Date.now();
      let webview = null;
      let syncLoop = null;

      if (ctx && ctx.win) {
        ctx.win._webviewLabel = wvLabel;
      }

      const renderHome = () => {
        dummy.style.display = 'none';
        if (webview) webview.hide().catch(()=>{});
        startPage.style.display = '';
        startPage.innerHTML = `
          <div class="br-logo">✦ OSAIMA Browser</div>
          <input class="br-search" placeholder="Search the web…" spellcheck="false" autocomplete="off" />
          <div class="br-tiles"></div>
          <div class="br-note">Powered by native Tauri Webview. Google Search and modern web apps are fully supported.</div>`;
        const tiles = startPage.querySelector('.br-tiles');
        for (const b of BOOKMARKS) {
          const t = el('button', 'br-tile', b.name);
          t.addEventListener('click', () => navigate(b.url));
          tiles.append(t);
        }
        const search = startPage.querySelector('.br-search');
        search.addEventListener('keydown', (e) => { if (e.key === 'Enter') navigate(e.target.value); });
        setTimeout(() => search.focus(), 30);
      };

      const syncWebview = () => {
        if (!document.body.contains(root)) {
           if (webview) webview.close().catch(()=>{});
           return; // Window closed
        }
        if (!webview || dummy.style.display === 'none') {
           syncLoop = requestAnimationFrame(syncWebview);
           return;
        }
        
        // Hide if OS window is hidden (minimized or different workspace)
        const osWin = root.closest('.wm-window');
        if (osWin && osWin.style.display === 'none') {
           webview.hide().catch(()=>{});
           syncLoop = requestAnimationFrame(syncWebview);
           return;
        }

        const rect = dummy.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
           webview.show().catch(()=>{});
           // For a child Webview, Logical positions work best
           webview.setPosition({ type: 'Logical', x: rect.x, y: rect.y }).catch(()=>{});
           webview.setSize({ type: 'Logical', width: rect.width, height: rect.height }).catch(()=>{});
        }
        
        syncLoop = requestAnimationFrame(syncWebview);
      };

      const normalize = (raw) => {
        const s = (raw || '').trim();
        if (!s) return null;
        if (s === HOME) return HOME;
        if (/^https?:\/\//i.test(s)) return s;
        if (/^[\w-]+(\.[\w-]+)+(\/.*)?$/.test(s)) return 'https://' + s;
        return 'https://www.google.com/search?q=' + encodeURIComponent(s);
      };

      const navigate = (raw) => {
        const target = normalize(raw);
        if (!target) return;
        if (target === HOME) { url.value = ''; renderHome(); return; }
        url.value = target;
        startPage.style.display = 'none';
        dummy.style.display = 'block';
        
        if (!webview) {
           const rect = dummy.getBoundingClientRect();
           const appWin = Window.getCurrent();
           webview = new Webview(appWin, wvLabel, {
             url: target,
             x: rect.x, y: rect.y, width: rect.width, height: rect.height,
             transparent: true
           });
           webview.once('tauri://error', function(e) {
               console.error("WEBVIEW ERROR: ", e);
               dummy.innerHTML = `<div style="color:red; padding:20px;">Webview failed to load: ${JSON.stringify(e)}</div>`;
           });
           if (ctx && ctx.win) ctx.win._webview = webview;
           syncLoop = requestAnimationFrame(syncWebview);
        } else {
           webview.eval(`window.location.href = "${target}";`);
        }
      };

      back.addEventListener('click', () => { if(webview) webview.eval('window.history.back()'); });
      fwd.addEventListener('click', () => { if(webview) webview.eval('window.history.forward()'); });
      reload.addEventListener('click', () => { if(webview) webview.eval('window.location.reload()'); });
      homeBtn.addEventListener('click', () => navigate(HOME));
      go.addEventListener('click', () => navigate(url.value));
      url.addEventListener('keydown', (e) => { if (e.key === 'Enter') navigate(url.value); });

      renderHome();
    },
    unmount(win) {
       if (win._webview) {
          win._webview.close().catch(()=>{});
       }
    }
  };
}

// ── System Monitor ──────────────────────────────────────────────────────────
function makeMonitor(services) {
  return {
    id: 'monitor',
    title: 'System Monitor',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="M18.7 8l-5.1 5.2-2.8-2.7L7 14.3"/></svg>',
    width: 460, height: 320,
    mount(root, ctx) {
      root.classList.add('app-monitor');
      root.innerHTML = `
        <div class="mon-metric">
          <div class="mon-head"><span>CPU</span><span class="mon-cpu-val">—</span></div>
          <div class="mon-track"><div class="mon-fill mon-cpu-fill"></div></div>
        </div>
        <div class="mon-metric">
          <div class="mon-head"><span>MEMORY</span><span class="mon-mem-val">—</span></div>
          <div class="mon-track"><div class="mon-fill mon-mem-fill"></div></div>
        </div>
        <div class="mon-info">
          <div><span>HOST</span><b class="mon-host">—</b></div>
          <div><span>KERNEL</span><b class="mon-kernel">—</b></div>
          <div><span>CORES</span><b class="mon-cores">—</b></div>
          <div><span>AI CORE</span><b class="mon-daemon">—</b></div>
        </div>
        <canvas class="mon-graph" width="420" height="70"></canvas>`;

      const cpuVal = root.querySelector('.mon-cpu-val');
      const cpuFill = root.querySelector('.mon-cpu-fill');
      const memVal = root.querySelector('.mon-mem-val');
      const memFill = root.querySelector('.mon-mem-fill');
      const canvas = root.querySelector('.mon-graph');
      const g = canvas.getContext('2d');
      const samples = new Array(60).fill(0);

      const tick = async () => {
        try {
          const s = await services.invoke('get_system_stats');
          const alive = await services.invoke('ping_daemon');
          const cpu = s.cpu.usage_percent;
          const memPct = (s.memory.used_bytes / s.memory.total_bytes) * 100;
          cpuVal.textContent = cpu.toFixed(1) + '%';
          cpuFill.style.width = cpu + '%';
          memVal.textContent = memPct.toFixed(1) + '%';
          memFill.style.width = memPct + '%';
          root.querySelector('.mon-host').textContent = s.host;
          root.querySelector('.mon-kernel').textContent = s.kernel;
          root.querySelector('.mon-cores').textContent = s.cpu.cores;
          const dEl = root.querySelector('.mon-daemon');
          dEl.textContent = alive ? 'online' : 'offline';
          dEl.className = 'mon-daemon ' + (alive ? 'ok' : 'err');
          samples.push(cpu); samples.shift();
          drawGraph();
        } catch {}
      };
      const drawGraph = () => {
        g.clearRect(0, 0, canvas.width, canvas.height);
        g.beginPath();
        samples.forEach((v, i) => {
          const x = (i / (samples.length - 1)) * canvas.width;
          const y = canvas.height - (v / 100) * canvas.height;
          i === 0 ? g.moveTo(x, y) : g.lineTo(x, y);
        });
        g.strokeStyle = '#5de4f0';
        g.lineWidth = 1.5;
        g.stroke();
        g.lineTo(canvas.width, canvas.height);
        g.lineTo(0, canvas.height);
        g.closePath();
        g.fillStyle = 'rgba(93,228,240,0.08)';
        g.fill();
      };

      tick();
      const timer = setInterval(tick, 1500);
      ctx.win._monitorTimer = timer;
    },
    unmount(win) { if (win._monitorTimer) clearInterval(win._monitorTimer); },
  };
}

// ── Files (real filesystem explorer) ─────────────────────────────────────────
function makeFiles(services) {
  return {
    id: 'files',
    title: 'Files',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>',
    width: 520, height: 380,
    mount(root) {
      root.classList.add('app-files');
      const path = el('div', 'files-path');
      const list = el('div', 'files-list');
      root.append(path, list);
      let cwd = '/';
      // Start in the user's home folder, which is where their files are.
      services.invoke('run_command', { cmd: 'printenv HOME' }).then((h) => {
        const home = String(h || '').trim();
        if (home.startsWith('/') && !home.includes('\n')) { cwd = home; render(); }
      }).catch(() => {});

      const goInto = (name) => { cwd = cwd === '/' ? '/' + name : cwd + '/' + name; render(); };
      const goUp = () => {
        if (cwd === '/') return;
        cwd = cwd.slice(0, cwd.lastIndexOf('/')) || '/';
        render();
      };

      const render = async () => {
        path.textContent = cwd;
        list.innerHTML = '';
        list.appendChild(el('div', 'files-empty', 'reading…'));
        let data;
        try {
          data = await services.invoke('read_dir', { path: cwd });
        } catch (e) {
          list.innerHTML = '';
          list.appendChild(el('div', 'files-empty', 'cannot read ' + cwd + ': ' + e));
          return;
        }
        list.innerHTML = '';
        if (cwd !== '/') {
          const up = el('div', 'files-item files-dir', '📁 ..');
          up.addEventListener('click', goUp);
          list.appendChild(up);
        }
        const entries = data.entries || [];
        for (const ent of entries) {
          const item = el('div', 'files-item ' + (ent.is_dir ? 'files-dir' : 'files-file'),
            (ent.is_dir ? '📁 ' : '📄 ') + ent.name);
          if (ent.is_dir) item.addEventListener('click', () => goInto(ent.name));
          list.appendChild(item);
        }
        if (entries.length === 0) list.appendChild(el('div', 'files-empty', '(empty)'));
      };
      render();
    },
  };
}

// ── Live Lua config editor ───────────────────────────────────────────────────
function makeConfigEditor(engine, services) {
  return {
    id: 'config',
    title: 'wm.lua — Config',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
    width: 600, height: 460,
    floating: true,
    mount(root) {
      root.classList.add('app-config');
      const toolbar = el('div', 'cfg-toolbar');
      const reloadBtn = el('button', 'cfg-btn cfg-reload', '⟳ Reload');
      const saveBtn = el('button', 'cfg-btn cfg-save', '💾 Save');
      const resetBtn = el('button', 'cfg-btn cfg-reset', '↺ Reset');
      const status = el('span', 'cfg-status', 'Live Lua config — Reload previews, Save persists');
      toolbar.append(reloadBtn, saveBtn, resetBtn, status);

      const ta = document.createElement('textarea');
      ta.className = 'cfg-editor';
      ta.spellcheck = false;
      ta.value = services.getConfigSource();

      root.append(toolbar, ta);

      const report = (result, okMsg) => {
        if (result.ok) {
          status.textContent = okMsg + ' at ' + new Date().toLocaleTimeString();
          status.className = 'cfg-status ok';
        } else {
          status.textContent = '✗ ' + result.error;
          status.className = 'cfg-status err';
        }
      };

      reloadBtn.addEventListener('click', async () => report(await services.reloadConfig(ta.value), '✓ Applied'));
      saveBtn.addEventListener('click', async () => report(await services.saveConfig(ta.value), '✓ Saved'));
      resetBtn.addEventListener('click', async () => {
        const result = await services.resetConfig();
        ta.value = services.getConfigSource();
        report(result, '↺ Restored default');
      });
      // Ctrl+Enter to reload (preview)
      ta.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); reloadBtn.click(); }
      });
    },
  };
}

// ── About / keybinding cheatsheet ────────────────────────────────────────────
function makeAbout(engine) {
  return {
    id: 'about',
    title: 'About OSAIMA',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
    width: 440, height: 380,
    floating: true,
    mount(root) {
      root.classList.add('app-about');
      const binds = [...engine.bindings.entries()]
        .map(([k, v]) => `<div class="ab-bind"><kbd>${escapeHtml(k)}</kbd><span>${escapeHtml(actionLabel(v))}</span></div>`)
        .join('');
      root.innerHTML = `
        <div class="ab-header">
          <div class="ab-logo">✦</div>
          <div>
            <div class="ab-title">OSAIMA</div>
            <div class="ab-sub">Lua-driven tiling window manager</div>
          </div>
        </div>
        <p class="ab-desc">A dynamic, agentic desktop shell. The window manager's
        layouts, gaps, workspaces and keybindings are all defined in a live Lua
        configuration file.</p>
        <div class="ab-section">KEYBINDINGS</div>
        <div class="ab-binds">${binds || '<em>none configured</em>'}</div>`;
    },
  };
}

// ── Assistant (the shell) ────────────────────────────────────────────────────
// The home screen: the user says what they want, sees what will happen and how
// risky it is, approves it here (never in a browser pop-up), and sees the result.
// A side rail shows the computer's state, learned suggestions and the audit log.
const SVG_SEND = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M13 6l6 6-6 6"/></svg>';

const SVG_MIC = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0"/><path d="M12 18v3"/></svg>';
const SVG_STOP = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>';
const SVG_SPEAKER = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5 6 9H3v6h3l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/></svg>';

// Examples of what the system is for, worded the way a person would say them.
const STARTERS = [
  'Find PDFs bigger than 10 MB in my home folder',
  'What is using the most memory right now?',
  'Write an email to my guide asking for a one-week extension',
  'How much space is left on my disk?',
  'Turn the volume down a bit',
  'How do I see which program is using port 8080?',
];

// What each risk tier means for the person, not for the code.
const TIER_COPY = {
  read_only: { label: 'Only looks', detail: 'Reads information. Nothing on your computer changes.' },
  reversible: { label: 'Makes changes', detail: 'Changes files or settings. You can undo it.' },
  irreversible: { label: "Can't be undone", detail: 'Deletes or overwrites something permanently.' },
};

function makeAssistant(engine, services) {
  return {
    id: 'assistant',
    title: 'Assistant',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>',
    width: 980, height: 660,
    mount(root, ctx) {
      root.classList.add('app-assistant');
      const main = el('section', 'as-main');
      const log = el('div', 'as-log');
      log.setAttribute('aria-live', 'polite');

      const composer = document.createElement('form');
      composer.className = 'as-composer';
      const input = document.createElement('input');
      input.className = 'as-input';
      input.placeholder = 'Tell your computer what you need…';
      input.spellcheck = false;
      input.setAttribute('aria-label', 'Request');
      const send = el('button', 'as-send');
      send.type = 'submit';
      send.innerHTML = SVG_SEND;
      send.setAttribute('aria-label', 'Send');
      const speaker = el('button', 'as-speak');
      speaker.type = 'button';
      speaker.innerHTML = SVG_SPEAKER;
      const mic = el('button', 'as-mic');
      mic.type = 'button';
      mic.innerHTML = SVG_MIC;
      composer.append(input, speaker, mic, send);
      const note = el('p', 'as-note', 'Nothing on this computer changes until you approve it.');

      // ── Voice: push to talk, and (optionally) replies read aloud ──
      const noteDefault = note.textContent;
      let noteTimer = null;
      const showNote = (message) => {
        note.textContent = message;
        clearTimeout(noteTimer);
        noteTimer = setTimeout(() => { note.textContent = noteDefault; }, 6000);
      };
      const renderVoice = () => {
        const s = voice.view;
        const listening = s.state === 'listening';
        mic.disabled = !s.canListen || s.state === 'transcribing';
        mic.dataset.state = s.state;
        mic.innerHTML = listening ? SVG_STOP : SVG_MIC;
        mic.setAttribute('aria-label', listening ? 'Stop and send' : 'Talk to your computer');
        mic.title = !s.canListen ? (s.why || 'Voice is not available') : listening ? 'Stop and send' : 'Talk to your computer';
        speaker.disabled = !s.canSpeak;
        speaker.setAttribute('aria-pressed', String(s.speakReplies));
        speaker.setAttribute('aria-label', 'Read replies aloud');
        speaker.title = !s.canSpeak ? (s.why || 'Voice is not available') : s.speakReplies ? 'Replies are read aloud (click to turn off)' : 'Read replies aloud';
      };
      const voice = createVoice({
        invoke: services.invoke,
        onText: (text) => run(text),
        onNotice: showNote,
        onChange: renderVoice,
        alive: () => root.isConnected,
      });
      mic.addEventListener('click', () => voice.toggle());
      speaker.addEventListener('click', () => voice.setSpeakReplies(!voice.view.speakReplies));
      renderVoice();
      voice.refresh();
      // The voice service may start after the window opens; look again until it is there.
      const voiceTimer = setInterval(() => {
        if (!root.isConnected) { clearInterval(voiceTimer); voice.dispose(); return; }
        if (!voice.view.canListen && voice.view.state === 'idle') voice.refresh();
      }, 15000);
      window.addEventListener('beforeunload', () => voice.dispose());
      // Files handed over from the file manager ("Ask OSAIMA"); they go with
      // each request until the user removes them.
      let selected = [];
      const attachRow = el('div', 'as-attached');
      attachRow.hidden = true;
      const renderAttached = () => {
        attachRow.hidden = !selected.length;
        if (!selected.length) { attachRow.replaceChildren(); return; }
        const names = selected.map((p) => p.split('/').pop());
        const label = el('span', 'as-attached-text', `About ${names.length > 2 ? `${names[0]} and ${names.length - 1} more` : names.join(' and ')}`);
        label.title = selected.join('\n');
        const clear = el('button', 'as-attached-clear', '×');
        clear.setAttribute('aria-label', 'Stop using these files');
        clear.addEventListener('click', () => { selected = []; renderAttached(); input.focus(); });
        attachRow.replaceChildren(label, clear);
      };
      main.append(log, attachRow, composer, note);

      const rail = buildRail();
      root.append(main, rail.el);

      // ── Empty state: greeting and starters ──
      const intro = el('div', 'as-intro');
      const hour = new Date().getHours();
      const part = hour < 12 ? 'morning' : hour < 17 ? 'afternoon' : 'evening';
      const hello = el('h1', 'as-hello', `Good ${part}.`);
      intro.append(
        hello,
        el('p', 'as-lede', "Tell me what you need in your own words. I'll show you exactly what I'm going to do before anything changes."),
        el('h2', 'as-sub', 'Try one of these'),
      );
      const starters = el('ul', 'as-starters');
      for (const s of STARTERS) {
        const li = document.createElement('li');
        const b = el('button', 'as-starter', s);
        b.type = 'button';
        b.addEventListener('click', () => run(s));
        li.append(b);
        starters.append(li);
      }
      intro.append(starters);
      log.append(intro);
      services.invoke('run_command', { cmd: 'whoami' }).then((u) => {
        const name = String(u || '').trim();
        if (/^[a-z_][a-z0-9_-]{0,31}$/i.test(name) && name !== 'root') hello.textContent = `Good ${part}, ${name}.`;
      }).catch(() => {});

      // ── Conversation ──
      let turn = null;   // the agent turn being built; cards attach to it live

      const scroll = () => { log.scrollTop = log.scrollHeight; };
      function addUser(text) {
        intro.remove();
        const row = el('div', 'as-turn as-user');
        row.append(el('p', 'as-said', text));
        log.append(row);
        scroll();
      }
      function startTurn() {
        const row = el('div', 'as-turn as-agent');
        const text = el('p', 'as-text as-working', 'Working on it…');
        const cards = el('div', 'as-cards');
        const meta = el('p', 'as-meta');
        row.append(text, cards, meta);
        log.append(row);
        scroll();
        return { row, text, cards, meta };
      }
      function attach(card) {
        (turn ? turn.cards : log).append(card);
        scroll();
      }

      // Approval asked inline. Resolves to { approved, typed }.
      function ask({ what, typed = false, phrase = 'confirm', untrusted = false }) {
        return new Promise((resolve) => {
          const card = el('div', 'as-card as-approve');
          card.dataset.tier = typed ? 'irreversible' : 'reversible';
          card.append(el('h3', 'as-card-title', typed ? "This can't be undone" : 'Needs your OK'));
          card.append(el('p', 'as-card-what', what.charAt(0).toUpperCase() + what.slice(1) + '.'));
          if (untrusted) card.append(el('p', 'as-card-warn', 'This was suggested by outside content (a page, file or message), not only by you.'));
          let field = null;
          if (typed) {
            field = document.createElement('input');
            field.className = 'as-typed';
            field.placeholder = `Type "${phrase}" to allow`;
            field.spellcheck = false;
            card.append(field);
          }
          const actions = el('div', 'as-card-actions');
          const allow = el('button', 'as-btn as-btn-go', 'Allow');
          const cancel = el('button', 'as-btn', 'Cancel');
          allow.disabled = typed;
          if (field) field.addEventListener('input', () => { allow.disabled = field.value.trim().toLowerCase() !== phrase; });
          const done = (approved) => {
            allow.disabled = cancel.disabled = true;
            if (field) field.disabled = true;
            card.classList.add(approved ? 'is-approved' : 'is-cancelled');
            actions.replaceChildren(el('span', 'as-card-state', approved ? 'Allowed' : 'Cancelled'));
            resolve({ approved, typed: field ? field.value.trim() : null });
          };
          allow.addEventListener('click', () => done(true));
          cancel.addEventListener('click', () => done(false));
          actions.append(allow, cancel);
          card.append(actions);
          if (turn) turn.text.textContent = 'Waiting for your OK.';
          attach(card);
          (field || allow).focus();
        });
      }

      // A proposed shell command: what it does, how risky it is, Run / Cancel,
      // then its output. The daemon decides the tier and re-checks it on run.
      function commandCard(plan, command) {
        const card = el('div', 'as-card as-command');
        card.append(el('h3', 'as-card-title', plan || 'Run a command'));
        const line = el('pre', 'as-cmdline');
        line.textContent = command;
        const risk = el('p', 'as-risk', 'Checking what this does…');
        const field = document.createElement('input');
        field.className = 'as-typed';
        field.spellcheck = false;
        field.hidden = true;
        const out = el('pre', 'as-output');
        out.hidden = true;
        const actions = el('div', 'as-card-actions');
        const runBtn = el('button', 'as-btn as-btn-go', 'Run');
        const cancel = el('button', 'as-btn', 'Cancel');
        runBtn.disabled = true;
        actions.append(runBtn, cancel);
        card.append(line, risk, field, out, actions);

        let tier = null;
        let phrase = 'confirm';
        services.invoke('assess_command', { command }).then((a) => {
          if (!a || !TIER_COPY[a.tier]) throw new Error('no assessment');
          tier = a.tier;
          phrase = a.confirm_phrase || phrase;
          card.dataset.tier = tier;
          const copy = TIER_COPY[tier];
          risk.replaceChildren(el('strong', 'as-risk-label', copy.label), document.createTextNode(' ' + copy.detail));
          if (a.reasons && a.reasons.length) risk.append(el('span', 'as-risk-why', ' ' + a.reasons.join('; ').replace(/`/g, '')));
          if (tier === 'irreversible') {
            field.hidden = false;
            field.placeholder = `Type "${phrase}" to allow`;
            field.addEventListener('input', () => { runBtn.disabled = field.value.trim().toLowerCase() !== phrase; });
          } else {
            runBtn.disabled = false;
          }
        }).catch(() => {
          // Fail closed: without the daemon there is no sandbox and no audit log.
          card.dataset.tier = 'irreversible';
          risk.textContent = "The AI core isn't running, so this can't be run safely right now.";
        });

        runBtn.addEventListener('click', async () => {
          runBtn.disabled = cancel.disabled = field.disabled = true;
          runBtn.textContent = 'Running…';
          out.hidden = false;
          out.textContent = 'Running in a sandbox…';
          try {
            const res = await services.invoke('execute_ai_command', {
              command, approved: true, typed: tier === 'irreversible' ? field.value : null,
            });
            if (!res) throw new Error('No answer from the AI core');
            const text = (res.output || '').replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '').trim();
            out.textContent = text || '(no output)';
            const ok = (res.exit_code ?? 1) === 0;
            actions.replaceChildren(el('span', 'as-card-state' + (ok ? '' : ' is-bad'),
              `${ok ? 'Done' : 'Finished with errors'} · ran in a sandbox · recorded in Activity`));
          } catch (e) {
            out.textContent = String(e && e.message ? e.message : e);
            actions.replaceChildren(el('span', 'as-card-state is-bad', "Didn't run"));
          }
          rail.refreshActivity();
          scroll();
        });
        cancel.addEventListener('click', () => {
          card.classList.add('is-cancelled');
          actions.replaceChildren(el('span', 'as-card-state', 'Cancelled. Nothing ran.'));
        });
        return card;
      }

      // Generated text (email, answer, how-to) with Copy / Save as note. Built
      // with textContent only: it comes from the model and must never be HTML.
      function contentCard({ title, text }) {
        const card = el('div', 'as-card as-content');
        card.append(el('h3', 'as-card-title', title));
        const body = el('div', 'as-content-body');
        renderLightMarkdown(body, text);
        const actions = el('div', 'as-card-actions');
        const copy = el('button', 'as-btn', 'Copy');
        copy.addEventListener('click', async () => {
          try { await navigator.clipboard.writeText(text); copy.textContent = 'Copied'; }
          catch { copy.textContent = "Couldn't copy"; }
        });
        const save = el('button', 'as-btn', 'Save as note');
        save.addEventListener('click', () => {
          const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
          notes.unshift({ id: Date.now().toString(36), title, content: text,
                          createdAt: Date.now(), updatedAt: Date.now(), pinned: false });
          localStorage.setItem('osaima-notes', JSON.stringify(notes));
          save.textContent = 'Saved to Notes'; save.disabled = true;
        });
        actions.append(copy, save);
        card.append(body, actions);
        return card;
      }

      // Shared with dispatchIntent: only while this window is on screen.
      services.ask = (o) => (root.isConnected ? ask(o) : null);
      services.showCommand = (plan, command) => (root.isConnected ? (attach(commandCard(plan, command)), true) : false);

      // File search/list results as rows you can act on, and an Undo button
      // after any file change that can be reversed.
      const HOME_RE = /^\/home\/[^/]+|^\/root/;
      const tilde = (p) => String(p || '').replace(HOME_RE, '~');
      const size = (b) => b >= 1073741824 ? (b / 1073741824).toFixed(1) + ' GB' : b >= 1048576 ? (b / 1048576).toFixed(1) + ' MB'
        : b >= 1024 ? Math.round(b / 1024) + ' KB' : b + ' B';
      const age = (secs) => {
        if (!secs) return '';
        const d = (Date.now() / 1000 - secs) / 86400;
        return d < 1 ? 'today' : d < 2 ? 'yesterday' : d < 30 ? `${Math.floor(d)} days ago` : new Date(secs * 1000).toLocaleDateString();
      };
      function fileAction(op, path, button) {
        button.disabled = true;
        services.invoke('os_action', { action: 'files', op, path, approved: false })
          .then(() => { button.textContent = op === 'open' ? 'Opened' : 'Shown'; })
          .catch((e) => { button.textContent = "Couldn't"; button.title = String(e.message || e); })
          .finally(() => setTimeout(() => { button.disabled = false; button.textContent = op === 'open' ? 'Open' : 'Show in Files'; }, 2000));
      }
      function filesCard(result) {
        const files = result.files || [];
        const card = el('div', 'as-card as-files');
        card.dataset.tier = 'read_only';
        card.append(el('h3', 'as-card-title', result.folder ? `In ${tilde(result.folder)}` : 'Files'));
        const list = el('ul', 'as-file-list');
        const row = (f) => {
          const li = el('li', 'as-file');
          const info = el('div', 'as-file-info');
          info.append(el('span', 'as-file-name', (f.is_dir ? '📁 ' : '') + f.name));
          info.append(el('span', 'as-file-meta', [tilde(f.path.slice(0, f.path.length - f.name.length - 1)), f.is_dir ? '' : size(f.size), age(f.modified)].filter(Boolean).join(' · ')));
          const open = el('button', 'as-btn as-btn-small', 'Open');
          open.addEventListener('click', () => fileAction('open', f.path, open));
          const show = el('button', 'as-btn as-btn-small', 'Show in Files');
          show.addEventListener('click', () => fileAction('reveal', f.path, show));
          li.append(info, open, show);
          return li;
        };
        files.slice(0, 8).forEach((f) => list.append(row(f)));
        card.append(list);
        if (files.length > 8) {
          const more = el('button', 'as-btn as-btn-small', `Show ${files.length - 8} more`);
          more.addEventListener('click', () => { files.slice(8).forEach((f) => list.append(row(f))); more.remove(); });
          card.append(more);
        }
        return card;
      }
      // count > 1: a plan's changes are undone together, newest first
      function undoCard(text, count = 1) {
        const card = el('div', 'as-card as-undo');
        card.dataset.tier = 'reversible';
        const actions = el('div', 'as-card-actions');
        const undo = el('button', 'as-btn', count > 1 ? 'Undo the whole plan' : 'Undo');
        undo.addEventListener('click', async () => {
          undo.disabled = true;
          try {
            for (let k = 0; k < count; k++) {
              await services.invoke('os_action', { action: 'files', op: 'undo', approved: false });
            }
            actions.replaceChildren(el('span', 'as-card-state', 'Undone. Everything is back where it was.'));
          } catch (e) {
            actions.replaceChildren(el('span', 'as-card-state is-bad', "Couldn't undo: " + (e.message || e)));
          }
          rail.refreshActivity();
        });
        actions.append(el('span', 'as-card-state', text), undo);
        card.append(actions);
        return card;
      }
      // One card for a whole multi-step plan: every step and its risk, one
      // approval (typed "confirm" if any step can't be undone), then each step
      // runs in order with its result; the first failure stops the rest.
      const TIER_RANK = { read_only: 0, reversible: 1, irreversible: 2 };
      async function runPlan(rawSteps, reply, query, w) {
        const steps = rawSteps.map((s) => planStep(s, services));
        const card = el('div', 'as-card as-plan');
        card.append(el('h3', 'as-card-title', `Plan: ${steps.length} step${steps.length === 1 ? '' : 's'}`));
        const list = el('ol', 'as-plan-steps');
        const rows = steps.map((st) => {
          const li = el('li', 'as-plan-step');
          li.dataset.state = 'pending';
          const text = el('span', 'as-plan-text', describeStep(st));
          const note = el('span', 'as-plan-note', 'checking…');
          li.append(text, note);
          list.append(li);
          return { li, note };
        });
        card.append(list);
        attach(card);

        // risk of each step, from the daemon where it decides
        let worst = 0;
        let needsOk = false;
        await Promise.all(steps.map(async (st, i) => {
          let tier = 'read_only', ok = true, warn = null;
          const ref = st.kind === 'os' && fromStep(st.req);
          try {
            if (ref) {
              // its files are only known once step N has run; the daemon checks then
              const d = await services.invoke('os_action', { ...st.req, paths: undefined, path: undefined, dry_run: true }).catch(() => ({}));
              const pol = { search: 'read_only', list: 'read_only', open: 'read_only', reveal: 'read_only', copy: 'reversible' };
              tier = d.tier || pol[st.req.op] || 'reversible';
              ok = tier === 'read_only';
              warn = null;
            } else if (st.kind === 'os') {
              const d = await services.invoke('os_action', { ...st.req, dry_run: true });
              tier = d.tier || 'read_only';
              ok = d.would_run;
            } else if (st.kind === 'cmd') {
              const a = await services.invoke('assess_command', { command: st.command });
              tier = a.tier;
              ok = tier === 'read_only';
            } else {
              const rule = BUILTIN_POLICY[st.intent];
              tier = rule ? (rule.typed ? 'irreversible' : 'reversible') : 'read_only';
              ok = !rule;
            }
          } catch (e) {
            // e.g. a folder an earlier step will create: checked again when it runs
            warn = String(e.message || e);
          }
          st.tier = tier;
          worst = Math.max(worst, TIER_RANK[tier] ?? 1);
          needsOk = needsOk || !ok;
          rows[i].li.dataset.tier = tier;
          rows[i].note.textContent = (warn ? `check: ${warn}` : TIER_COPY[tier] ? TIER_COPY[tier].label : tier)
            + (ref ? ` · uses the files from step ${ref}` : '');
        }));
        const tierName = Object.keys(TIER_RANK).find((k) => TIER_RANK[k] === worst);
        card.dataset.tier = tierName;
        const typedNeeded = worst === 2;

        const approved = await new Promise((resolve) => {
          if (!needsOk && worst === 0) { resolve({ ok: true, typed: null }); return; }
          const field = typedNeeded ? Object.assign(document.createElement('input'), {
            className: 'as-typed', placeholder: 'Type "confirm" to allow every step', spellcheck: false }) : null;
          const actions = el('div', 'as-card-actions');
          const go = el('button', 'as-btn as-btn-go', 'Run plan');
          const cancel = el('button', 'as-btn', 'Cancel');
          go.disabled = typedNeeded;
          if (field) field.addEventListener('input', () => { go.disabled = field.value.trim().toLowerCase() !== 'confirm'; });
          go.addEventListener('click', () => { actions.remove(); if (field) field.disabled = true; resolve({ ok: true, typed: field ? field.value.trim() : null }); });
          cancel.addEventListener('click', () => { actions.replaceChildren(el('span', 'as-card-state', 'Cancelled. Nothing ran.')); card.classList.add('is-cancelled'); resolve({ ok: false }); });
          actions.append(go, cancel);
          if (field) card.append(field);
          card.append(actions);
          if (turn) turn.text.textContent = 'Waiting for your OK on the plan.';
          (field || go).focus();
          scroll();
        });
        if (!approved.ok) return 'Cancelled. None of the steps ran.';

        // the user approved this exact list, so each step carries that approval;
        // the daemon still re-checks every one of them
        const planServices = { ...services, runPlan: null,
          ask: async () => ({ approved: true, typed: approved.typed || null }),
          showCommand: null };
        let done = 0;
        let undoable = 0;
        const results = [];
        const finishUndo = () => { if (undoable) attach(undoCard(`${undoable} change${undoable === 1 ? '' : 's'} to your files.`, undoable)); };
        for (let i = 0; i < steps.length; i++) {
          const st = steps[i];
          const row = rows[i];
          row.li.dataset.state = 'running';
          row.note.textContent = 'running…';
          if (turn) turn.text.textContent = `Step ${i + 1} of ${steps.length}…`;
          try {
            let note;
            if (st.kind === 'os') {
              const ref = fromStep(st.req);
              if (ref) {
                const found = ((results[ref - 1] || {}).files || []).filter((f) => !f.is_dir).map((f) => f.path);
                if (!found.length) throw new Error(`step ${ref} found no files to use`);
                st.req = { ...st.req, paths: found, path: undefined };
                row.note.textContent = `running on ${found.length} file${found.length === 1 ? '' : 's'}…`;
              }
              const res = await services.invoke('os_action', { ...st.req, approved: true, typed: approved.typed || '' });
              results[i] = res && res.result;
              if (st.req.action === 'files') services.showFileResult(st.req, res && res.result, { inPlan: true });
              if (res && res.result && res.result.undoable) undoable++;
              note = describeOsResult(st.req, res) || 'done';
            } else if (st.kind === 'cmd') {
              const res = await services.invoke('execute_ai_command', { command: st.command, approved: true, typed: approved.typed });
              const out = (res.output || '').replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '').trim();
              if ((res.exit_code ?? 1) !== 0) throw new Error(out.split('\n').pop() || `exit ${res.exit_code}`);
              note = out ? out.split('\n').slice(0, 3).join(' · ').slice(0, 160) : 'done';
            } else {
              note = (await dispatchIntent(st.intent, st.params, '', query, w, planServices)) || 'done';
            }
            row.li.dataset.state = 'done';
            row.note.textContent = note;
            done++;
          } catch (e) {
            row.li.dataset.state = 'failed';
            row.note.textContent = String(e.message || e);
            rows.slice(i + 1).forEach((r) => { r.li.dataset.state = 'skipped'; r.note.textContent = 'skipped'; });
            finishUndo();
            rail.refreshActivity();
            return `Stopped at step ${i + 1}: ${String(e.message || e)}. ${done ? `The ${done} step${done === 1 ? '' : 's'} before it finished.` : 'Nothing was changed.'}`;
          }
        }
        finishUndo();
        rail.refreshActivity();
        offerRoutine(rawSteps, query);
        return reply || `Done: all ${steps.length} steps finished.`;
      }
      // After a finished plan: offer to keep it as a routine. Never for plans shaped by
      // outside content (tainted), and not for a routine that is already saved.
      async function offerRoutine(rawSteps, query) {
        if (!meta || meta.tainted || meta.source === 'routine') return;
        let seen = { count: 1, saved: false };
        try { seen = (await services.invoke('routine_note', { steps: rawSteps })) || seen; } catch { return; }
        if (seen.saved || !root.isConnected) return;
        const card = el('div', 'as-card as-routine');
        card.append(el('h3', 'as-card-title', 'Save as a routine'));
        card.append(el('p', 'as-routine-note', seen.count >= 2
          ? `You've done this ${seen.count} times. Save it as a routine?`
          : 'Run these steps again later by name.'));
        // exactly what would be saved; file names came from a folder listing, so say so
        const what = el('ol', 'as-task-outline');
        rawSteps.forEach((st) => what.append(el('li', null, describeStep(planStep(st, services)))));
        card.append(what);
        if (meta.usedFolderNames) card.append(el('p', 'as-card-warn', 'These steps use file names read from your folders. Check them before saving; each run will still ask you first.'));
        const field = Object.assign(document.createElement('input'), { className: 'as-typed', spellcheck: false, maxLength: 60 });
        field.value = String(query || '').trim().replace(/^(please|can you|could you)\s+/i, '').replace(/[.!?\s]+$/, '').slice(0, 40).trim();
        const actions = el('div', 'as-card-actions');
        const save = el('button', 'as-btn as-btn-go', 'Save');
        const state = el('span', 'as-card-state', '');
        save.addEventListener('click', async () => {
          const name = field.value.trim();
          if (!name) { state.textContent = 'Give it a name first.'; return; }
          save.disabled = true;
          try {
            await services.invoke('routine_save', { name, request: String(query || ''), steps: rawSteps });
            field.disabled = true;
            actions.replaceChildren(el('span', 'as-card-state', `Saved. Say "run my ${name} routine" any time.`));
            rail.refreshRoutines();
          } catch (e) {
            save.disabled = false;
            state.textContent = String(e.message || e);
          }
        });
        field.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); save.click(); } });
        actions.append(save, state);
        card.append(field, actions);
        attach(card);
        scroll();
      }
      services.runPlan = (steps, reply, query, w) => (root.isConnected ? runPlan(steps, reply, query, w) : null);

      // ── Agent tasks ──
      // A job the daemon's agent works through (task.rs): first its scope to
      // approve (one folder, internet or not, never root), then live steps with
      // their output, approvals in place, and a summary. It runs in the
      // background, so the conversation can go on meanwhile.
      const TASK_STATUS = {
        planning: 'Planning…', awaiting_scope: 'Needs your OK to start', running: 'Working', awaiting_approval: 'Needs your OK',
        done: 'Done', failed: 'Stopped', cancelled: 'Cancelled',
      };
      const STEP_ICON = { running: '…', done: '✓', failed: '✗', refused: '⊘', declined: '⊘', needs_approval: '?' };
      function taskCard(id, goal) {
        const card = el('div', 'as-card as-task');
        card.dataset.tier = 'read_only';
        const head = el('div', 'as-task-head');
        const title = el('h3', 'as-card-title', goal.length > 90 ? goal.slice(0, 90) + '…' : goal);
        const chip = el('span', 'as-task-status', 'Planning…');
        head.append(title, chip);
        const scope = el('div', 'as-task-scope');
        const list = el('ol', 'as-task-steps');
        const pending = el('div', 'as-task-pending');
        const summary = el('div', 'as-task-summary');
        const foot = el('div', 'as-card-actions');
        const stop = el('button', 'as-btn', 'Stop');
        const meta = el('span', 'as-card-state', '');
        foot.append(stop, meta);
        stop.hidden = true;   // until the task is running; the scope has its own Cancel
        card.append(head, scope, list, pending, summary, foot);
        stop.addEventListener('click', () => services.invoke('task_cancel', { id }).catch(() => {}));

        const rows = new Map();
        let scopeShown = false, pendingFor = null, last = '';
        const decide = (approve, typed) => services.invoke('task_decide', { id, approve, typed: typed || '' }).catch(() => {});

        function render(st) {
          chip.textContent = TASK_STATUS[st.status] || st.status;
          stop.hidden = !['running', 'awaiting_approval'].includes(st.status);
          card.dataset.status = st.status;
          if (st.title) title.textContent = st.title;
          // scope: shown once, answered once
          if (st.status === 'awaiting_scope' && !scopeShown) {
            scopeShown = true;
            card.dataset.tier = 'reversible';
            const facts = el('ul', 'as-task-facts');
            facts.append(el('li', null, st.folder ? `Changes files only in ${tilde(st.folder)}` : 'Only looks; changes no files'));
            facts.append(el('li', null, st.network ? 'Downloads from the internet' : 'No internet access'));
            facts.append(el('li', null, 'Never runs as administrator (root)'));
            const outline = el('ol', 'as-task-outline');
            (st.outline || []).forEach((o) => outline.append(el('li', null, o)));
            const go = el('button', 'as-btn as-btn-go', 'Start task');
            const no = el('button', 'as-btn', 'Cancel');
            const acts = el('div', 'as-card-actions');
            go.addEventListener('click', () => { acts.replaceChildren(el('span', 'as-card-state', 'Started')); decide(true); });
            no.addEventListener('click', () => { acts.replaceChildren(el('span', 'as-card-state', 'Cancelled')); decide(false); });
            acts.append(go, no);
            scope.append(el('p', 'as-task-lede', 'Here is the plan. The computer itself (Linux kernel) holds the task to these limits:'), facts, outline, acts);
            go.focus();
          }
          if (st.status !== 'awaiting_scope' && scopeShown) scope.querySelector('.as-card-actions')?.remove();
          for (const s of st.steps || []) {
            let r = rows.get(s.id);
            if (!r) {
              const li = el('li', 'as-task-step');
              const icon = el('span', 'as-task-icon');
              const body = el('div', 'as-task-body');
              const detail = el(s.tool === 'run_command' ? 'code' : 'span', 'as-task-detail', s.detail);
              body.append(detail);
              if (s.why) body.append(el('span', 'as-task-why', s.why));
              const out = document.createElement('details');
              out.className = 'as-task-out';
              out.append(el('summary', null, 'Output'), el('pre', null, ''));
              out.hidden = true;
              body.append(out);
              li.append(icon, body);
              list.append(li);
              r = { li, icon, out };
              rows.set(s.id, r);
            }
            r.li.dataset.state = s.state;
            r.icon.textContent = STEP_ICON[s.state] || '•';
            if (s.output) {
              r.out.hidden = false;
              r.out.querySelector('pre').textContent = s.output;
              if (s.state === 'failed' || s.state === 'refused') r.out.open = r.out.open || s.state === 'refused';
            }
          }
          // a risky step waiting for the typed confirm
          if (st.status === 'awaiting_approval' && st.pending && pendingFor !== st.pending.step) {
            pendingFor = st.pending.step;
            card.dataset.tier = 'irreversible';
            const field = Object.assign(document.createElement('input'), { className: 'as-typed', placeholder: 'Type "confirm" to allow this step', spellcheck: false });
            const allow = el('button', 'as-btn as-btn-go', 'Allow');
            const skip = el('button', 'as-btn', 'Skip this step');
            allow.disabled = true;
            field.addEventListener('input', () => { allow.disabled = field.value.trim().toLowerCase() !== 'confirm'; });
            allow.addEventListener('click', () => { pending.replaceChildren(); decide(true, field.value); });
            skip.addEventListener('click', () => { pending.replaceChildren(); decide(false); });
            const acts = el('div', 'as-card-actions');
            acts.append(allow, skip);
            const what = el('pre', 'as-cmdline');
            what.textContent = st.pending.detail;
            pending.replaceChildren(el('p', 'as-card-warn', "This step can't be undone, so it waits for you." + (st.pending.why ? ` (${st.pending.why})` : '')), what, field, acts);
            field.focus();
          }
          if (st.status !== 'awaiting_approval' && pending.childElementCount) { pending.replaceChildren(); pendingFor = null; card.dataset.tier = 'reversible'; }
          const ended = ['done', 'failed', 'cancelled'].includes(st.status);
          meta.textContent = `${(st.steps || []).length} step${(st.steps || []).length === 1 ? '' : 's'} · ${st.seconds}s`;
          if (ended) {
            stop.remove();
            card.dataset.tier = st.status === 'done' ? 'read_only' : 'irreversible';
            if (st.summary && st.summary !== last) { last = st.summary; summary.replaceChildren(); renderLightMarkdown(summary, st.summary); }
          }
          scroll();
          return ended;
        }

        const poll = setInterval(async () => {
          if (!root.isConnected) { clearInterval(poll); return; }
          try {
            const st = await services.invoke('task_status', { id });
            if (render(st)) { clearInterval(poll); rail.refreshActivity(); }
          } catch (e) { meta.textContent = "Can't reach the AI core: " + (e.message || e); }
        }, 1000);
        return card;
      }
      services.startTask = async (goal) => {
        if (!root.isConnected) return null;
        const r = await services.invoke('task_start', { goal });
        attach(taskCard(r.id, goal));
        return "I'll plan this first and show you what it needs. You can keep talking to me while it runs.";
      };

      // an automation the model proposed: shown in plain words, created only on Allow
      services.addAutomation = async (rule) => {
        if (!root.isConnected) return null;
        const d = await services.invoke('automation_add', { rule, dry_run: true });
        const ok = await ask({ what: 'set up this automation: ' + d.summary, typed: !!d.typed });
        if (!ok.approved) return 'Cancelled — no automation was set up.';
        const r = await services.invoke('automation_add', { rule, approved: true, typed: ok.typed || '' });
        rail.refreshActivity();
        return `Done. ${r.summary}. You'll get a notification each time it runs, and you can see or remove it in Settings → Automations.`;
      };

      services.showFileResult = (req, result, opts = {}) => {
        if (!root.isConnected || !result) return;
        if (result.files && (req.op === 'search' || req.op === 'list') && result.files.length) attach(filesCard(result));
        else if (result.undoable && !opts.inPlan) attach(undoCard('Changed your files.'));
        rail.refreshActivity();
      };

      // How the reply was produced, in plain words.
      function describeSource(m) {
        const files = [...new Set((m.sources || []).map((s) => s.split('/').pop().replace(/\.md$/, '')))];
        const cite = files.length ? ` · using ${files.join(', ')}` : '';
        if (m.source === 'fast') return `Understood on this computer in ${Math.round(m.ms || 0)} ms`;
        if (m.source === 'llm') {
          const who = /^gemini/.test(m.backend || '') ? 'Gemini' : 'the on-device model';
          return `Answered by ${who} in ${((m.ms || 0) / 1000).toFixed(1)} s${cite}`;
        }
        return "The AI core didn't answer, so this is a basic offline reply";
      }

      // The conversation so far, sent with each request so follow-ups make sense.
      const history = [];
      const remember = (role, text) => {
        if (!text) return;
        history.push({ role, text: String(text).slice(0, 1500) });
        if (history.length > 6) history.splice(0, history.length - 6);
      };

      let meta = null;
      async function respond(query) {
        const q = query.toLowerCase().trim();
        const w = ctx.wm;
        meta = null;
        if (services.behavior) services.behavior.recordAssistantQuery(query);

        try {
          const r = await services.invoke('classify_intent', { query, history: history.slice(), selected });
          if (r && r.intent) {
            const params = r.params || {};
            // only text from outside the computer (pages, mail, handed-in documents) blocks
            // saving a routine; the user's own folder listings and the knowledge base don't
            const outside = (r.context_from || ['untrusted']).some((k) => k !== 'folder' && k !== 'knowledge');
            meta = { source: r.source || 'llm', ms: r.ms, sources: r.sources || [], backend: r.backend, tainted: !!r.tainted && outside,
                     usedFolderNames: !!r.tainted && (r.context_from || []).includes('folder') };
            if (r.intent === 'generate_text' && params.content) {
              meta.content = { title: params.title || 'Answer', text: params.content };
              return r.reply && r.reply !== 'Here you go:' ? r.reply : 'Here it is.';
            }
            // on a real desktop the daemon gates window actions itself
            const confirmation = services.desktopMode && DESKTOP_INTENTS.has(r.intent) ? null : builtinConfirmation(r.intent, params, r);
            if (confirmation && !(await confirmBuiltin(confirmation, services))) return 'Cancelled. Nothing was changed.';
            // the AI answered; a failure from here on is the action's, so say so
            // rather than falling back to the offline notes
            let out;
            try {
              out = await dispatchIntent(r.intent, params, r.reply || '', query, w, services, ctx.win?.id);
            } catch (e) {
              console.warn('action failed:', e);
              return `Something went wrong while doing that: ${e && e.message ? e.message : e}`;
            }
            if (out) return out;
            // No action for this intent, but the AI did answer: show that, never the offline text.
            if (r.reply) return r.reply;
          }
        } catch (err) {
          console.warn('classify_intent failed:', err);
        }

        // Offline fallback: only when the AI core gave nothing.
        meta = { source: 'offline' };
        if (/^(hi|hello|hey)\b/.test(q)) return 'Hello. What would you like to do?';
        if (/^(help|\?)$|what can you do/.test(q)) return 'Try one of the examples, or describe a task: finding files, checking the system, writing a message, changing volume or Wi-Fi.';
        if (/(how|status|health|cpu|memory|ram).*(system|doing)|^(stats?|status)$/.test(q)) {
          try {
            const s = await services.invoke('get_system_stats');
            return `CPU ${s.cpu.usage_percent.toFixed(0)}%, memory ${((s.memory.used_bytes / s.memory.total_bytes) * 100).toFixed(0)}% used.`;
          } catch { return "I can't reach the system right now."; }
        }
        // the built-in notes can answer a question offline, never stand in for an action
        if (services.rag && /^(what|how|why|who|when|where|which|explain|tell me about)\b/.test(q)) {
          const res = services.rag.answer(query);
          if (res.sources.length > 0) return res.answer;
        }
        return "I can't reach the AI core right now. Check that it's running (Settings shows its status), then try again.";
      }

      let busy = false;
      const queued = [];   // sent while an answer was still coming: run in order
      let lastSent = { text: '', at: 0 };
      async function run(text) {
        const v = (text ?? input.value).trim();
        if (!v) return;
        if (text === undefined) input.value = '';
        // a double click (or a double Enter) sends the same request once
        if (v === lastSent.text && Date.now() - lastSent.at < 1500) return;
        lastSent = { text: v, at: Date.now() };
        voice.silence();
        if (busy) { queued.push(v); return; }
        busy = true;
        addUser(v);
        turn = startTurn();
        const t = turn;
        let reply;
        try { reply = await respond(v); }
        catch (e) { reply = 'Something went wrong: ' + (e && e.message ? e.message : e); }
        t.text.classList.remove('as-working');
        // A command card already says what it does; don't repeat it above.
        const title = t.cards.querySelector('.as-command .as-card-title');
        if (title && title.textContent.trim().replace(/\.$/, '') === String(reply).trim().replace(/\.$/, '')) {
          reply = "Here's what I'd run. Check it, then choose Run.";
        }
        t.text.textContent = reply;
        voice.speak(reply);
        if (meta && meta.content) t.cards.prepend(contentCard(meta.content));
        if (meta) t.meta.textContent = describeSource(meta);
        remember('user', v);
        remember('assistant', meta && meta.content ? `${reply}\n${meta.content.text}` : reply);
        turn = null;
        busy = false;
        scroll();
        if (queued.length) { run(queued.shift()); return; }
        rail.refreshSuggestions();
        rail.refreshRoutines();
        input.focus();
      }

      composer.addEventListener('submit', (e) => { e.preventDefault(); run(); });
      window.addEventListener('osaima:ask', (e) => { if (root.isConnected) run(e.detail); });
      window.addEventListener('osaima:focus', () => { if (root.isConnected) input.focus(); });
      // pick up files sent from the file manager
      const shareTimer = setInterval(async () => {
        if (!root.isConnected) { clearInterval(shareTimer); return; }
        try {
          const r = await services.invoke('shared_take');
          // a request from the Ask bar: run it as if typed here
          if (r && r.query) run(r.query);
          if (r && r.paths && r.paths.length) {
            selected = r.paths;
            renderAttached();
            input.placeholder = 'Ask about these files…';
            input.focus();
          }
        } catch { /* daemon busy or down: try again */ }
      }, 1500);
      setTimeout(() => input.focus(), 30);

      // ── Side rail ──
      function buildRail() {
        const railEl = el('aside', 'as-rail');

        const sys = el('section', 'as-panel');
        sys.append(el('h2', 'as-panel-title', 'This computer'));
        const meters = {};
        for (const [key, label] of [['cpu', 'Processor'], ['mem', 'Memory'], ['disk', 'Disk']]) {
          const row = el('div', 'as-meter');
          const head = el('div', 'as-meter-head');
          const val = el('span', 'as-meter-val', '—');
          head.append(el('span', 'as-meter-label', label), val);
          const track = el('div', 'as-meter-track');
          const fill = el('div', 'as-meter-fill');
          track.append(fill);
          row.append(head, track);
          sys.append(row);
          meters[key] = { val, fill };
        }
        // health watch warnings: overheating, throttling, low disk, failed services
        const alertList = el('ul', 'as-alerts');
        const host = el('p', 'as-panel-foot', '');
        sys.append(alertList, host);

        const sug = el('section', 'as-panel');
        sug.append(el('h2', 'as-panel-title', 'You often ask'));
        const sugList = el('ul', 'as-sug');
        sug.append(sugList);

        const rou = el('section', 'as-panel');
        rou.append(el('h2', 'as-panel-title', 'Your routines'));
        const rouList = el('ul', 'as-sug');
        rou.append(rouList);

        const act = el('section', 'as-panel as-panel-grow');
        act.append(el('h2', 'as-panel-title', 'Activity'));
        const actList = el('ol', 'as-act');
        const actFoot = el('p', 'as-panel-foot', '');
        act.append(actList, actFoot);

        railEl.append(sys, rou, sug, act);

        const gb = (b) => (b / 1073741824).toFixed(1) + ' GB';
        const setMeter = (m, pct, text) => {
          m.val.textContent = text;
          m.fill.style.width = Math.max(0, Math.min(100, pct)).toFixed(1) + '%';
          m.fill.dataset.level = pct >= 90 ? 'high' : pct >= 75 ? 'mid' : 'ok';
        };
        async function refreshStats() {
          try {
            const s = await services.invoke('get_system_stats');
            setMeter(meters.cpu, s.cpu.usage_percent, `${s.cpu.usage_percent.toFixed(0)}%`);
            const mp = (s.memory.used_bytes / s.memory.total_bytes) * 100;
            setMeter(meters.mem, mp, `${gb(s.memory.used_bytes)} of ${gb(s.memory.total_bytes)}`);
            if (s.disk) {
              const dp = (s.disk.used_bytes / s.disk.total_bytes) * 100;
              setMeter(meters.disk, dp, `${gb(s.disk.total_bytes - s.disk.used_bytes)} free`);
            }
            host.textContent = `${s.host} · ${s.os || 'Linux'}`;
          } catch { host.textContent = "Can't read system stats."; }
        }

        async function refreshRoutines() {
          let mine = [];
          try { mine = ((await services.invoke('routines', {})) || {}).routines || []; } catch {}
          if (!mine.length) {
            rouList.replaceChildren(el('li', 'as-empty', 'Finished plans you save appear here.'));
            return;
          }
          rouList.replaceChildren(...mine.map((r) => {
            const li = el('li', 'as-routine-item');
            const b = el('button', 'as-sug-item', r.name);
            b.type = 'button';
            b.title = `${r.steps} step${r.steps === 1 ? '' : 's'}`;
            b.addEventListener('click', () => run(`run my ${r.name} routine`));
            const x = el('button', 'as-routine-del', '\u00d7');
            x.type = 'button';
            x.title = 'Delete this routine';
            x.setAttribute('aria-label', `Delete ${r.name}`);
            x.addEventListener('click', async () => {
              try { await services.invoke('routine_delete', { id: r.id }); } catch {}
              refreshRoutines();
            });
            li.append(b, x);
            return li;
          }));
        }

        function showAlerts(alerts) {
          alertList.replaceChildren(...(alerts || []).slice(0, 3).map((a) => {
            const li = el('li', 'as-alert');
            li.dataset.level = a.level || 'warn';
            li.append(el('strong', null, a.title), el('span', null, a.advice || a.detail || ''));
            const why = el('button', 'as-sug-item', 'Why is this happening?');
            why.type = 'button';
            why.addEventListener('click', () => run(`why ${a.title.toLowerCase()}?`));
            li.append(why);
            return li;
          }));
        }
        async function refreshSuggestions() {
          let mine = [], usual = [];
          try {
            const r = (await services.invoke('suggestions', { n: 4 })) || {};
            mine = r.suggestions || [];
            usual = r.predictions || [];
            showAlerts(r.alerts);
          } catch {}
          // "you usually do this around now": do it, or make it an automation (asks first)
          const predicted = usual.map((p) => {
            const li = el('li', 'as-usual');
            li.append(el('span', 'as-usual-why', `Usually around ${p.usual_time}`));
            const go = el('button', 'as-sug-item', p.text);
            go.type = 'button';
            go.addEventListener('click', () => run(p.text));
            const auto = el('button', 'as-usual-auto', `Do this every day at ${p.usual_time}`);
            auto.type = 'button';
            auto.addEventListener('click', () => run(`every day at ${p.usual_time} ${p.text}`));
            li.append(go, auto);
            return li;
          });
          mine = mine.filter((m) => !usual.some((p) => p.text === m.toLowerCase()));
          if (!mine.length && !predicted.length) {
            sugList.replaceChildren(el('li', 'as-empty', "Nothing yet. Requests you make often will show up here."));
            return;
          }
          sugList.replaceChildren(...predicted, ...mine.map((s) => {
            const li = document.createElement('li');
            const b = el('button', 'as-sug-item', s);
            b.type = 'button';
            b.addEventListener('click', () => run(s));
            li.append(b);
            return li;
          }));
        }

        const sentence = (e) => {
          const d = e.data || {};
          const what = describeFileRequest(d) || OS_ACTION_WORDS[`${d.action} ${d.op || ''}`.trim()]
            || (d.action === 'files' && d.op === 'search' ? 'searched your files' : null)
            || (d.action === 'files' && d.op === 'reveal' ? `showed ${baseName(d.path || (d.paths || [])[0] || 'a file')} in Files` : null)
            || (d.action === 'files' && d.op === 'undo' ? 'undid the last file change' : null)
            || (d.action === 'app' ? `open ${d.app || 'an app'}` : null)
            || (d.action === 'window' && d.app ? `${d.op} ${d.app}` : null)
            || `${String(d.action || '').replace(/_/g, ' ')} ${String(d.op || '').replace(/_/g, ' ')}`.trim();
          switch (e.event) {
            case 'execute': return ['ok', `Ran ${d.command}`];
            case 'refused': return ['held', `Held back ${d.command} until you approve it`];
            case 'os_action': return ['ok', what.charAt(0).toUpperCase() + what.slice(1)];
            case 'os_refused': return ['held', `Held back ${what} until you approve it`];
            case 'os_failed': return ['bad', `Couldn't ${what}: ${d.error || 'failed'}`];
            case 'task_start': return ['ok', `Started task: ${d.title || d.goal}`];
            case 'task_done': return ['ok', `Finished task: ${d.title} (${d.steps} steps)`];
            case 'task_refused': return ['held', `Held back in a task: ${d.detail} (${d.why})`];
            case 'automation_add': return ['ok', `Set up automation: ${d.summary}`];
            case 'automation_run': return [d.ok ? 'ok' : 'bad', `Automation ran: ${d.automation}${d.ok ? '' : ` (failed: ${d.error})`}`];
            case 'automation_cancelled': return ['held', `Cancelled automation: ${d.automation || 'pending run'}`];
            default: return null;
          }
        };
        async function refreshActivity() {
          let res = null;
          try { res = await services.invoke('audit_tail', { n: 40 }); } catch {}
          if (!res) { actFoot.textContent = "Can't read the activity log."; return; }
          const rows = (res.entries || [])
            .filter((e) => !(e.event === 'os_action' && e.data && e.data.op === 'status'))
            .map((e) => [e, sentence(e)]).filter(([, s]) => s).slice(-6).reverse();
          if (!rows.length) actList.replaceChildren(el('li', 'as-empty', 'Commands I run for you will be listed here.'));
          else actList.replaceChildren(...rows.map(([e, [kind, text]]) => {
            const li = el('li', 'as-act-item');
            li.dataset.kind = kind;
            const t = new Date(e.ts);
            li.append(el('time', 'as-act-time', t.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })),
                      el('span', 'as-act-text', text));
            return li;
          }));
          const v = res.verified || {};
          actFoot.textContent = v.ok ? `Log intact · ${v.records} entries checked` : `Log was altered at entry ${v.line}`;
          actFoot.classList.toggle('is-bad', !v.ok);
        }

        refreshStats(); refreshRoutines(); refreshSuggestions(); refreshActivity();
        const timer = setInterval(() => {
          if (!root.isConnected) { clearInterval(timer); return; }
          refreshStats();
        }, 3000);
        const slow = setInterval(() => {
          if (!root.isConnected) { clearInterval(slow); return; }
          refreshActivity(); refreshSuggestions();
        }, 15000);
        return { el: railEl, refreshActivity, refreshSuggestions, refreshRoutines };
      }
    },
  };
}

// ── Task Manager (ML-prioritized processes) ──────────────────────────────────
// Nods to the kernel's ML-based scheduling vision: each process shows an
// ML-assigned priority the "scheduler" adjusts over time.
function makeTaskManager(services) {
  return {
    id: 'taskmanager',
    title: 'Task Manager',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>',
    width: 580, height: 420,
    mount(root, ctx) {
      root.classList.add('app-tasks');
      root.innerHTML = `
        <div class="tk-toolbar">
          <span class="tk-title">Processes — live from /proc</span>
          <span class="tk-hint">click a row · Renice / Kill</span>
        </div>
        <div class="tk-head">
          <span class="tk-c-pid">PID</span>
          <span class="tk-c-name">NAME</span>
          <span class="tk-c-cpu">CPU%</span>
          <span class="tk-c-mem">MEM</span>
          <span class="tk-c-pri">LOAD</span>
        </div>
        <div class="tk-rows"></div>
        <div class="tk-actions">
          <button class="tk-btn tk-boost" disabled>⬆ Renice (boost)</button>
          <button class="tk-btn tk-kill" disabled>✕ Kill</button>
        </div>`;

      const rowsEl = root.querySelector('.tk-rows');
      const boostBtn = root.querySelector('.tk-boost');
      const killBtn = root.querySelector('.tk-kill');

      let procs = [];
      let selectedPid = null;

      const render = () => {
        rowsEl.innerHTML = '';
        for (const p of procs) {
          const load = Math.max(0.02, Math.min(1, p.cpu / 40 + p.mem / 4000));
          const pct = Math.round(load * 100);
          const cls = load > 0.66 ? 'hi' : load > 0.33 ? 'mid' : 'lo';
          const row = el('div', 'tk-row' + (p.pid === selectedPid ? ' selected' : ''));
          row.innerHTML = `
            <span class="tk-c-pid">${p.pid}</span>
            <span class="tk-c-name">${escapeHtml(p.name)}</span>
            <span class="tk-c-cpu">${p.cpu.toFixed(1)}</span>
            <span class="tk-c-mem">${p.mem.toFixed(0)}M</span>
            <span class="tk-c-pri"><span class="tk-prbar ${cls}" style="width:${pct}%"></span><b>${pct}</b></span>`;
          row.addEventListener('click', () => {
            selectedPid = p.pid;
            boostBtn.disabled = killBtn.disabled = false;
            render();
          });
          rowsEl.append(row);
        }
      };

      const load = async () => {
        try {
          const data = await services.invoke('list_processes');
          procs = data.processes || [];
        } catch { return; }
        render();
      };

      boostBtn.addEventListener('click', async () => {
        if (!selectedPid) return;
        try { await services.invoke('run_command', { cmd: 'renice -n -5 -p ' + selectedPid }); } catch {}
        load();
      });
      killBtn.addEventListener('click', async () => {
        if (!selectedPid) return;
        try { await services.invoke('kill_process', { pid: selectedPid }); } catch {}
        selectedPid = null;
        boostBtn.disabled = killBtn.disabled = true;
        load();
      });

      load();
      const timer = setInterval(load, 2000);
      ctx.win._tkTimer = timer;
    },
    unmount(win) { if (win._tkTimer) clearInterval(win._tkTimer); },
  };
}

// ── Knowledge Base (RAG front-end) ───────────────────────────────────────────
// Ask a question; the RAG engine retrieves the most relevant passages and shows
// the extractive answer plus its cited sources with similarity scores.
function makeKnowledge(services) {
  return {
    id: 'knowledge',
    title: 'Knowledge Base (RAG)',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>',
    width: 520, height: 460,
    floating: true,
    mount(root) {
      root.classList.add('app-rag');
      const stats = services.rag ? services.rag.stats() : { docs: 0, chunks: 0 };
      root.innerHTML = `
        <div class="rag-bar">
          <input class="rag-input" placeholder="Ask about OSAIMA, the WM, Linux…" spellcheck="false" />
          <button class="rag-ask">Search</button>
        </div>
        <div class="rag-meta">Indexed: ${stats.docs} docs · ${stats.chunks} chunks (TF-IDF retrieval)</div>
        <div class="rag-results"></div>`;

      const input = root.querySelector('.rag-input');
      const askBtn = root.querySelector('.rag-ask');
      const results = root.querySelector('.rag-results');

      const run = () => {
        const q = input.value.trim();
        if (!q || !services.rag) return;
        const res = services.rag.answer(q);
        results.innerHTML = '';
        const ans = el('div', 'rag-answer');
        ans.textContent = res.answer;
        results.append(el('div', 'rag-label', 'ANSWER'), ans);
        if (res.sources.length) {
          results.append(el('div', 'rag-label', 'RETRIEVED SOURCES'));
          for (const s of res.sources) {
            const card = el('div', 'rag-source');
            card.innerHTML = `<div class="rag-src-head"><b>${escapeHtml(s.title)}</b>`
              + `<span class="rag-score">${(s.score * 100).toFixed(0)}% · ${escapeHtml(s.source)}</span></div>`;
            const t = el('div', 'rag-src-text'); t.textContent = s.text;
            card.append(t);
            results.append(card);
          }
        }
      };
      askBtn.addEventListener('click', run);
      input.addEventListener('keydown', (e) => { if (e.key === 'Enter') run(); });
      root.addEventListener('mousedown', () => setTimeout(() => input.focus(), 0));
      setTimeout(() => input.focus(), 30);
    },
  };
}

// ── Behavior Profile (behavior-learning front-end) ───────────────────────────
// Shows what OSAIMA has learned about the user: top apps, preferred layout,
// predicted next app, suggestion acceptance, peak activity hour.
function makeBehavior(engine, services) {
  return {
    id: 'behavior',
    title: 'Behavior Profile',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24 2.5 2.5 0 0 1 1.98-3A2.5 2.5 0 0 1 9.5 2Z"/><path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24 2.5 2.5 0 0 0-1.98-3A2.5 2.5 0 0 0 14.5 2Z"/></svg>',
    width: 440, height: 440,
    floating: true,
    mount(root) {
      root.classList.add('app-behavior');
      const render = () => {
        const b = services.behavior;
        if (!b) { root.innerHTML = '<div class="bh-empty">Behavior store unavailable.</div>'; return; }
        const s = b.summary();
        const appName = (id) => (engine.appRegistry.get(id) || {}).title || id;
        const topApps = s.topApps.length
          ? s.topApps.map((a) => `<div class="bh-row"><span>${escapeHtml(appName(a.id))}</span><b>${a.count}×</b></div>`).join('')
          : '<div class="bh-dim">nothing yet — use the desktop and come back</div>';
        const accept = s.acceptanceRate === null ? '—' : (s.acceptanceRate * 100).toFixed(0) + '%';
        const queries = s.recentQueries.length
          ? s.recentQueries.map((q) => `<div class="bh-q">“${escapeHtml(q.q)}”</div>`).join('')
          : '<div class="bh-dim">no questions asked yet</div>';
        root.innerHTML = `
          <div class="bh-head">
            <div class="bh-logo">🧠</div>
            <div><div class="bh-title">What OSAIMA has learned</div>
            <div class="bh-sub">${s.totalEvents} events observed · persists on-device</div></div>
          </div>
          <div class="bh-grid">
            <div class="bh-card"><span class="bh-k">Preferred layout</span><span class="bh-v">${escapeHtml(s.preferredLayout || '—')}</span></div>
            <div class="bh-card"><span class="bh-k">Likely next app</span><span class="bh-v">${escapeHtml(appName(s.predictedNext) || '—')}</span></div>
            <div class="bh-card"><span class="bh-k">Suggestion accept</span><span class="bh-v">${accept}</span></div>
            <div class="bh-card"><span class="bh-k">Peak hour</span><span class="bh-v">${s.peakHour === null ? '—' : s.peakHour + ':00'}</span></div>
          </div>
          <div class="bh-section">MOST-USED APPS</div>
          <div class="bh-list">${topApps}</div>
          <div class="bh-section">RECENT QUESTIONS</div>
          <div class="bh-list">${queries}</div>
          <div class="bh-actions"><button class="bh-btn bh-refresh">↻ Refresh</button><button class="bh-btn bh-reset">Reset learning</button></div>`;
        root.querySelector('.bh-refresh').addEventListener('click', render);
        root.querySelector('.bh-reset').addEventListener('click', () => { b.reset(); render(); });
      };
      render();
    },
  };
}

// ── Notes App ─────────────────────────────────────────────────────────────────
function makeNotes(services) {
  const STORAGE_KEY = 'osaima-notes';
  const SELECT_KEY = 'osaima-notes-select';

  const loadNotes = () => JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]');
  const saveNotes = (notes) => localStorage.setItem(STORAGE_KEY, JSON.stringify(notes));
  const fmtDate = (ts) => {
    const d = new Date(ts);
    const now = new Date();
    if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
  };

  return {
    id: 'notes',
    title: 'Notes',
    icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/></svg>',
    width: 680, height: 480,
    mount(root, ctx) {
      root.classList.add('app-notes');

      let notes = loadNotes();
      let selectedId = null;
      let searchQuery = '';
      let saveTimer = null;

      // ── build DOM ──
      const sidebar = el('div', 'notes-sidebar');
      const sidebarHeader = el('div', 'notes-sidebar-header');
      const titleRow = el('div', 'notes-sidebar-title');
      titleRow.innerHTML = '<span>Notes</span>';
      const newBtn = el('button', 'notes-new-btn', '+ New');
      titleRow.appendChild(newBtn);
      const searchInput = document.createElement('input');
      searchInput.className = 'notes-search';
      searchInput.placeholder = 'Search notes…';
      searchInput.spellcheck = false;
      sidebarHeader.append(titleRow, searchInput);
      const listEl = el('div', 'notes-list');
      const countEl = el('div', 'notes-count');
      sidebar.append(sidebarHeader, listEl, countEl);

      const editorPane = el('div', 'notes-editor');
      root.append(sidebar, editorPane);

      // ── helpers ──
      const getFiltered = () => {
        let filtered = notes;
        if (searchQuery) {
          const q = searchQuery.toLowerCase();
          filtered = notes.filter(n => n.title.toLowerCase().includes(q) || n.content.toLowerCase().includes(q));
        }
        // pinned first, then by updatedAt
        return filtered.sort((a, b) => (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) || b.updatedAt - a.updatedAt);
      };

      const autoSave = () => {
        clearTimeout(saveTimer);
        saveTimer = setTimeout(() => saveNotes(notes), 300);
      };

      const selectNote = (id) => {
        selectedId = id;
        renderList();
        renderEditor();
      };

      const createNote = (title, content) => {
        const note = {
          id: Date.now().toString(36) + Math.random().toString(36).slice(2, 5),
          title: title || 'Untitled Note',
          content: content || '',
          createdAt: Date.now(),
          updatedAt: Date.now(),
          pinned: false
        };
        notes.unshift(note);
        saveNotes(notes);
        selectNote(note.id);
        return note;
      };

      // ── render sidebar list ──
      const renderList = () => {
        const filtered = getFiltered();
        listEl.innerHTML = '';
        if (filtered.length === 0) {
          listEl.innerHTML = '<div class="notes-empty-list">' + (searchQuery ? 'No matching notes.' : 'No notes yet.<br>Click + New to create one.') + '</div>';
        } else {
          for (const n of filtered) {
            const item = el('div', 'notes-item' + (n.id === selectedId ? ' selected' : ''));
            const titleDiv = el('div', 'notes-item-title');
            if (n.pinned) { const pin = el('span', 'notes-item-pin', '📌'); titleDiv.appendChild(pin); }
            const titleText = document.createTextNode(n.title || 'Untitled');
            titleDiv.appendChild(titleText);
            const preview = el('div', 'notes-item-preview', (n.content || '').slice(0, 60) || 'Empty note');
            const date = el('div', 'notes-item-date', fmtDate(n.updatedAt));
            item.append(titleDiv, preview, date);
            item.addEventListener('click', () => selectNote(n.id));
            listEl.appendChild(item);
          }
        }
        countEl.textContent = notes.length + ' note' + (notes.length !== 1 ? 's' : '');
      };

      // ── render editor ──
      const renderEditor = () => {
        editorPane.innerHTML = '';
        const note = notes.find(n => n.id === selectedId);
        if (!note) {
          editorPane.innerHTML = '<div class="notes-empty"><div class="notes-empty-icon">📝</div><div class="notes-empty-text">Select a note or create a new one</div><div class="notes-empty-hint">Tip: Ask the AI to "write a note about…"</div></div>';
          return;
        }

        // Toolbar
        const toolbar = el('div', 'notes-toolbar');
        const pinBtn = el('button', 'notes-tool-btn' + (note.pinned ? ' active' : ''), note.pinned ? '📌 Pinned' : '📌 Pin');
        const boldBtn = el('button', 'notes-tool-btn', '𝐁');
        const italicBtn = el('button', 'notes-tool-btn', '𝑰');
        const listBtn = el('button', 'notes-tool-btn', '• List');
        const spacer = el('div', 'notes-tool-spacer');
        const saveHint = el('span', 'notes-save-hint', 'Auto-saved');
        const delBtn = el('button', 'notes-tool-btn danger', '🗑 Delete');
        toolbar.append(pinBtn, boldBtn, italicBtn, listBtn, spacer, saveHint, delBtn);

        // Title input
        const titleInput = document.createElement('input');
        titleInput.className = 'notes-title-input';
        titleInput.value = note.title;
        titleInput.placeholder = 'Note title…';
        titleInput.spellcheck = false;

        // Content textarea
        const contentArea = document.createElement('textarea');
        contentArea.className = 'notes-content-area';
        contentArea.value = note.content;
        contentArea.placeholder = 'Start writing…';

        editorPane.append(toolbar, titleInput, contentArea);

        // ── events ──
        titleInput.addEventListener('input', () => {
          note.title = titleInput.value;
          note.updatedAt = Date.now();
          saveHint.textContent = 'Saving…';
          autoSave();
          setTimeout(() => { saveHint.textContent = 'Auto-saved'; renderList(); }, 400);
        });

        contentArea.addEventListener('input', () => {
          note.content = contentArea.value;
          note.updatedAt = Date.now();
          saveHint.textContent = 'Saving…';
          autoSave();
          setTimeout(() => { saveHint.textContent = 'Auto-saved'; }, 400);
        });

        pinBtn.addEventListener('click', () => {
          note.pinned = !note.pinned;
          note.updatedAt = Date.now();
          saveNotes(notes);
          renderList();
          renderEditor();
        });

        const insertAround = (before, after) => {
          const start = contentArea.selectionStart;
          const end = contentArea.selectionEnd;
          const sel = contentArea.value.substring(start, end);
          contentArea.setRangeText(before + sel + after, start, end, 'select');
          note.content = contentArea.value;
          note.updatedAt = Date.now();
          autoSave();
          contentArea.focus();
        };

        boldBtn.addEventListener('click', () => insertAround('**', '**'));
        italicBtn.addEventListener('click', () => insertAround('_', '_'));
        listBtn.addEventListener('click', () => {
          const start = contentArea.selectionStart;
          contentArea.setRangeText('\n- ', start, start, 'end');
          note.content = contentArea.value;
          note.updatedAt = Date.now();
          autoSave();
          contentArea.focus();
        });

        delBtn.addEventListener('click', () => {
          const idx = notes.findIndex(n => n.id === selectedId);
          if (idx !== -1) {
            notes.splice(idx, 1);
            saveNotes(notes);
            selectedId = notes.length > 0 ? notes[0].id : null;
            renderList();
            renderEditor();
          }
        });

        setTimeout(() => contentArea.focus(), 30);
      };

      // ── wire sidebar events ──
      newBtn.addEventListener('click', () => createNote());
      searchInput.addEventListener('input', () => {
        searchQuery = searchInput.value.trim();
        renderList();
      });

      // Check for AI-driven selection
      const checkAISelect = () => {
        const selId = localStorage.getItem(SELECT_KEY);
        if (selId) {
          localStorage.removeItem(SELECT_KEY);
          notes = loadNotes(); // reload in case AI just created one
          if (notes.find(n => n.id === selId)) {
            selectNote(selId);
          }
        }
      };

      // Initial render
      if (notes.length > 0) selectedId = notes[0].id;
      checkAISelect();
      renderList();
      renderEditor();

      // Poll for AI-driven changes
      const poll = setInterval(() => {
        const selId = localStorage.getItem(SELECT_KEY);
        if (selId) {
          localStorage.removeItem(SELECT_KEY);
          notes = loadNotes();
          if (notes.find(n => n.id === selId)) selectedId = selId;
          renderList();
          renderEditor();
        }
      }, 500);
      ctx.win._notesTimer = poll;
    },
    unmount(win) { if (win._notesTimer) clearInterval(win._notesTimer); },
  };
}

// ── helpers ──────────────────────────────────────────────────────────────────
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
function fmtBytes(b) {
  if (b >= 1073741824) return (b / 1073741824).toFixed(1) + 'GB';
  if (b >= 1048576) return (b / 1048576).toFixed(0) + 'MB';
  return (b / 1024).toFixed(0) + 'KB';
}
function actionLabel(v) {
  if (typeof v === 'function') return '(lua function)';
  return String(v).replace(/_/g, ' ').replace(':', ' → ');
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
// Policy for built-in intents the shell executes directly. Mirrors the daemon's
// tiers: irreversible ones need the typed phrase, the rest a confirm dialog.
// (redteam_eval.py scores against the same table.)
const BUILTIN_POLICY = {
  clean_storage: { always: true, typed: true, what: 'permanently empty the Trash and clear cached thumbnails' },
  delete_note: { always: true, what: 'delete a note' },
  close_all_windows: { what: 'close every open window' },
  optimize_cpu: { what: 'pause (freeze) background apps using CPU' },
  fix_network: { what: 'restart networking' },
};

function builtinConfirmation(intent, params, result) {
  const rule = BUILTIN_POLICY[intent];
  if (!rule) return null;
  const unsure = result.source !== 'fast' || result.tainted;
  if (!rule.always && !unsure) return null;
  const what = intent === 'delete_note' && params.title ? `delete the note "${params.title}"` : rule.what;
  return { what, typed: !!rule.typed, untrusted: !!result.tainted };
}

// Asks inline in the assistant when it is on screen, else with a browser dialog.
async function confirmBuiltin({ what, typed, untrusted }, services) {
  if (services && services.ask) {
    const answer = await services.ask({ what, typed, untrusted });
    if (answer) return answer.approved && (!typed || (answer.typed || '').toLowerCase() === 'confirm');
  }
  const why = untrusted ? '\n(This was suggested by outside content, not just by you.)' : '';
  if (typed) {
    const answer = window.prompt(`This can't be undone: ${what}.${why}\nType "confirm" to continue.`);
    return (answer || '').trim().toLowerCase() === 'confirm';
  }
  return window.confirm(`Allow: ${what}?${why}`);
}

// Model text with **bold**, `code` and "- " bullets, built from text nodes
// only: nothing the model writes is ever parsed as HTML.
function renderLightMarkdown(target, text) {
  const inline = (parent, line) => {
    for (const part of line.split(/(\*\*[^*]+\*\*|`[^`]+`)/g)) {
      if (!part) continue;
      if (part.startsWith('**') && part.endsWith('**') && part.length > 4) parent.append(el('strong', null, part.slice(2, -2)));
      else if (part.startsWith('`') && part.endsWith('`') && part.length > 2) parent.append(el('code', 'as-code', part.slice(1, -1)));
      else parent.append(document.createTextNode(part));
    }
  };
  let list = null;
  for (const raw of String(text).split('\n')) {
    const bullet = raw.match(/^\s*(?:[-*•]|\d+[.)])\s+(.*)$/);
    if (bullet) {
      if (!list) { list = el('ul', 'as-md-list'); target.append(list); }
      const li = document.createElement('li');
      inline(li, bullet[1]);
      list.append(li);
      continue;
    }
    list = null;
    const p = el('p', 'as-md-p');
    if (!raw.trim()) p.classList.add('as-md-gap');
    inline(p, raw.replace(/^#+\s*/, ''));
    target.append(p);
  }
}

// ── Multi-step plans ─────────────────────────────────────────────────────────
// Each step of an orchestrate plan becomes one of: an OS/desktop/file request
// for the daemon's os_action gate, a shell command for the sandbox, or a
// built-in the shell runs itself.
function planStep(step, services) {
  const intent = step.intent;
  const p = step.params || {};
  if (intent === 'files') return { kind: 'os', req: { ...p, action: 'files', op: p.op || 'search' } };
  if (intent === 'os_action') return { kind: 'os', req: { ...p } };
  if (intent === 'shutdown') return { kind: 'os', req: { action: 'power', op: 'shutdown' } };
  if (services.desktopMode && DESKTOP_INTENTS.has(intent)) {
    const req = desktopRequest(intent, p);
    if (req) return { kind: 'os', req };
  }
  if (intent === 'execute_command' && p.command) return { kind: 'cmd', command: p.command, plan: p.plan };
  return { kind: 'builtin', intent, params: p };
}

// A plan step that acts on the files an earlier step found: {"from_step": N}.
function fromStep(req) {
  const ref = req && (req.paths && req.paths.from_step || req.path && req.path.from_step);
  return Number.isInteger(ref) && ref > 0 ? ref : null;
}

function describeStep(st) {
  const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);
  if (st.kind === 'os') {
    const r = st.req;
    const text = describeFileRequest(r) || OS_ACTION_WORDS[`${r.action} ${r.op}`.trim()]
      || (r.action === 'app' ? `open ${r.app}` : r.action === 'browser' ? `search the web for "${r.query}"`
      : r.action === 'files' && r.op === 'search' ? `find ${r.kind || ''} files${r.query ? ` named "${r.query}"` : ''}`.replace('  ', ' ')
      : r.action === 'files' && r.op === 'reveal' ? `show ${baseName(r.path)} in Files`
      : r.action === 'files' && r.op === 'open' ? `open ${baseName(r.path || (r.paths || [])[0])}`
      : `${r.action.replace(/_/g, ' ')} ${String(r.op).replace(/_/g, ' ')}`);
    return cap(text.replace(' (you can undo this)', ''));
  }
  if (st.kind === 'cmd') return cap(st.plan || 'Run a command');
  const t = st.params.title ? ` "${st.params.title}"` : st.params.app ? ` ${st.params.app}` : '';
  return cap(st.intent.replace(/_/g, ' ') + t);
}

// "move invoice.pdf and 2 more to Documents": what a file change will do, in words.
const baseName = (p) => String(p || '').replace(/\/+$/, '').split('/').pop() || p;
function describeFileRequest(req) {
  if (req.action !== 'files') return null;
  const ref = fromStep(req);
  const list = ref ? [] : (Array.isArray(req.paths) && req.paths.length ? req.paths : [req.path]).filter((p) => typeof p === 'string').map(baseName);
  const names = ref ? `the files from step ${ref}` : list.length > 2 ? `${list[0]} and ${list.length - 1} more` : list.join(' and ');
  switch (req.op) {
    case 'move': return `move ${names} to ${baseName(req.to)}`;
    case 'copy': return `copy ${names} to ${baseName(req.to)}`;
    case 'rename': return `rename ${names} to ${req.name}`;
    case 'trash': return `move ${names} to the Trash (you can undo this)`;
    case 'mkdir': return `create the folder ${baseName(req.path)}`;
    default: return null;
  }
}

// How OS actions that need approval are described to the user.
const OS_ACTION_WORDS = {
  'power reboot': 'restart the computer',
  'power shutdown': 'shut down the computer',
  'power suspend': 'put the computer to sleep',
  'power logout': 'log you out',
  'wifi off': 'turn Wi-Fi off',
  'bluetooth off': 'turn Bluetooth off',
  'lock_screen': 'lock the screen',
  'window close_all': 'close every open window',
};

// OS controls go through the daemon (system.os_action). When its policy needs
// the user's approval, ask here and retry; the daemon re-checks either way.
async function runOsAction(services, req) {
  const call = (extra) => services.invoke('os_action', { ...req, approved: false, typed: null, ...extra });
  try {
    return await call({});
  } catch (e) {
    const msg = String(e && e.message ? e.message : e);
    if (!msg.includes('needs approval')) throw e;
    const what = describeFileRequest(req) || OS_ACTION_WORDS[`${req.action} ${req.op}`.trim()]
      || `${req.action.replace('_', ' ')} ${req.op}`.trim();
    const phrase = (msg.match(/typed phrase '([^']+)'/) || [])[1];
    const answer = services.ask ? await services.ask({ what, typed: !!phrase, phrase: phrase || 'confirm' }) : null;
    if (answer) {
      if (!answer.approved) throw new Error('cancelled');
      return call({ approved: true, typed: answer.typed });
    }
    if (phrase) {
      const typed = window.prompt(`This can't be undone: ${what}.\nType "${phrase}" to continue.`);
      if ((typed || '').trim().toLowerCase() !== phrase) throw new Error('cancelled');
      return call({ approved: true, typed });
    }
    if (!window.confirm(`Allow: ${what}?`)) throw new Error('cancelled');
    return call({ approved: true });
  }
}

function describeOsResult(req, res) {
  const r = (res && res.result) || {};
  switch (req.action) {
    case 'volume': return r.percent == null ? null : `Volume ${r.percent}%${r.muted ? ' (muted)' : ''}.`;
    case 'brightness': return r.percent == null ? null : `Brightness ${r.percent}%.`;
    case 'wifi': return `Wi-Fi is ${r.wifi_enabled ? 'on' : 'off'}${r.connected ? ' and connected' : ''}.`;
    case 'bluetooth': return `Bluetooth is ${r.bluetooth_on ? 'on' : 'off'}.`;
    case 'media': return `Media: ${req.op.replace('_', '/')}.`;
    case 'lock_screen': return 'Screen locked.';
    case 'power': return `Power: ${req.op}.`;
    case 'app': return r.launched ? `Opened ${req.app}.` : r.focused ? `${req.app} was already open, so I brought it forward.` : null;
    case 'window':
      if (req.op === 'focus') return `Switched to ${req.app}.`;
      if (req.op === 'close_all') return r.closed ? `Closed ${r.closed} window${r.closed === 1 ? '' : 's'}. I stayed open so we can keep talking.` : 'There was nothing to close.';
      if (req.op === 'close') return r.closed ? `Closed ${req.app || 'the window'}.` : `No ${req.app || 'window'} was open.`;
      return null;
    case 'layout': return `Switched the layout to ${req.op}.`;
    case 'workspace': return `Now on workspace ${req.value}.`;
    case 'browser': return `Searching the web for "${req.query}" in Firefox.`;
    case 'files':
      switch (req.op) {
        case 'search': return r.total ? `Found ${r.total} file${r.total === 1 ? '' : 's'}${r.total > (r.files || []).length ? `; here are the ${(r.files || []).length} newest` : ''}.` : 'I found no files matching that.';
        case 'list': return `${(r.files || []).length} items in ${baseName(r.folder)}.`;
        case 'open': return 'Opened it.';
        case 'reveal': return 'Showing it in Files.';
        case 'undo': return `Undone: ${r.undone === 'trash' ? 'restored from the Trash' : r.undone === 'move' ? 'moved back' : 'removed what I created'}.`;
        default: return `Done: ${describeFileRequest(req) || req.op}.`;
      }
    default: return null;
  }
}

// On a real desktop (sway), window intents act on real windows through the
// daemon's os_action gate instead of on panels in this page.
const DESKTOP_INTENTS = new Set(['open_app', 'close_window', 'close_all_windows', 'focus_app', 'layout',
                                 'workspace', 'search_web', 'minimize_all']);
function desktopRequest(intent, params) {
  switch (intent) {
    case 'open_app': return params.app ? { action: 'app', op: 'launch', app: params.app } : null;
    case 'focus_app': return params.app ? { action: 'window', op: 'focus', app: params.app } : null;
    case 'close_window': return { action: 'window', op: 'close', app: params.app || params.target || '' };
    case 'close_all_windows': return { action: 'window', op: 'close_all' };
    case 'layout': return params.layout ? { action: 'layout', op: params.layout } : null;
    case 'workspace': return params.number ? { action: 'workspace', op: 'switch', value: params.number } : null;
    case 'search_web': return params.query ? { action: 'browser', op: 'search', query: params.query } : null;
    default: return null;
  }
}

export async function dispatchIntent(intent, params, reply, query, w, services, skipWinId = null) {
  if (services.desktopMode && DESKTOP_INTENTS.has(intent)) {
    if (intent === 'minimize_all') return 'Windows tile instead of minimising here. Ask me to switch to an empty workspace, or close what you are not using.';
    const req = desktopRequest(intent, params);
    if (req) return dispatchIntent('os_action', req, reply, query, w, services, skipWinId);
  }
  const invokeFn = (cmd, args) => (window.__TAURI__?.core?.invoke || window.__TAURI__?.invoke || services?.invoke || (async () => null))(cmd, args);
  switch (intent) {
    case 'optimize_cpu':
      try {
        const res = await invokeFn('freeze_heavy_apps');
        return res || "Optimization complete.";
      } catch (e) { return "Failed to optimize CPU: " + e; }

    case 'wake_apps':
      try {
        const res = await invokeFn('unfreeze_apps');
        return res || "Woke up apps.";
      } catch (e) { return "Failed to wake apps: " + e; }

    case 'clean_storage':
      try {
        const res = await invokeFn('clean_storage');
        return res || "Storage cleaned.";
      } catch (e) { return "Failed to clean storage: " + e; }

    case 'fix_network':
      try {
        const res = await invokeFn('fix_network');
        return res || "Network reset.";
      } catch (e) { return "Failed to fix network: " + e; }

    case 'timer':
      if (services.widgets) {
        services.widgets.spawnTimer(params.seconds || 300);
      }
      return reply || "Timer started!";

    case 'stop_timer':
      if (services.widgets) {
        services.widgets.clearTimers();
      }
      return reply || "Timer stopped.";

    case 'reminder':
      if (services.widgets) {
        const text = params.text || params.reminder || query;
        services.widgets.spawnReminder(text);
      }
      return reply || "Reminder created!";

    case 'calendar':
      if (services.widgets) services.widgets.spawnCalendar();
      return reply || "Here is the calendar.";

    case 'calculator':
      if (services.widgets) {
        services.widgets.spawnCalculator(params.result || params.expression || '');
      }
      return reply || `The result is ${params.result}.`;

    case 'weather':
      if (services.widgets) services.widgets.spawnWeather();
      return reply || "Here is the weather.";

    case 'vitals':
      if (services.widgets) services.widgets.spawnVitals();
      return reply || "Showing system vitals.";

    case 'scratchpad':
      if (services.widgets) services.widgets.spawnScratchpad();
      return reply || "Opened a scratchpad.";

    case 'open_app': {
      let app = params.app || '';
      if (app === 'taskmanager') app = 'monitor';
      if (w.appRegistry.has(app)) {
        w.spawn(app);
        w.notify('Launched ' + app, 'ok');
      }
      return reply || `Opened ${app}.`;
    }

    case 'layout':
      if (params.layout) {
        w.setLayout(params.layout);
        w.notify('Layout → ' + params.layout, 'ok');
      }
      return reply || `Switched to ${params.layout} layout.`;

    case 'workspace': {
      const n = params.number || 1;
      w.switchWorkspace(n - 1);
      return reply || `Switched to workspace ${n}.`;
    }

    case 'system_status':
      try {
        const s = await services.invoke('get_system_stats');
        const memPct = ((s.memory.used_bytes / s.memory.total_bytes) * 100).toFixed(0);
        let out = `The processor is at ${s.cpu.usage_percent.toFixed(0)}% and memory is ${memPct}% used.`;
        // Name the programs behind the numbers: that is usually the real question.
        try {
          const r = await services.invoke('list_processes');
          // drop the ps that produced this list: it always looks busy
          const procs = (Array.isArray(r) ? r : (r && r.processes) || []).filter((p) => p.name !== 'ps');
          const pct = (p) => p.mem_percent ?? 0;
          const byMem = [...procs].sort((a, b) => pct(b) - pct(a)).slice(0, 3).filter((p) => pct(p) > 0);
          if (byMem.length) out += ' Using the most memory: ' + byMem.map((p) => `${p.name} (${pct(p).toFixed(1)}%)`).join(', ') + '.';
          const busy = [...procs].sort((a, b) => (b.cpu_percent ?? 0) - (a.cpu_percent ?? 0))[0];
          if (busy && (busy.cpu_percent ?? 0) >= 5) out += ` Busiest right now: ${busy.name} (${busy.cpu_percent.toFixed(0)}% CPU).`;
        } catch { /* stats alone still answer */ }
        return out;
      } catch { return "I can't read the system's stats right now."; }

    case 'close_window': {
      const target = params.app || params.target || '';
      if (target) {
        const targetWin = w.windows?.find(win =>
          win.appId === target ||
          win.appId?.toLowerCase().includes(target.toLowerCase()) ||
          win.title?.toLowerCase().includes(target.toLowerCase())
        );
        if (targetWin) {
          w.close(targetWin.id);
          return reply || `Closed ${target}.`;
        }
        return reply || `Could not find a window called "${target}".`;
      }
      if (w.focusedId) {
        const focWin = w.getWindow(w.focusedId);
        if (focWin && focWin.appId === 'assistant') {
          return "I can't close myself while you're talking to me! Tell me which app to close.";
        }
        w.closeFocused();
        return reply || 'Closed the focused window.';
      }
      return 'There is no focused window to close.';
    }

    case 'close_all_windows': {
      const allWins = w.windows ? [...w.windows] : [];
      let count = 0;
      for (const win of allWins) {
        if (win.appId === 'assistant' || win.id === skipWinId) continue;
        try { w.close(win.id); count++; } catch {}
      }
      w.notify('Closed ' + count + ' windows', 'ok');
      return reply || `Closed ${count} windows. (I kept myself open so you can keep talking to me.)`;
    }

    case 'minimize_all': {
      const allW = w.windows ? [...w.windows] : [];
      for (const win of allW) {
        try { w.minimize(win.id); } catch {}
      }
      return reply || 'Minimized all windows — desktop cleared.';
    }

    case 'focus_app': {
      const target = params.app || '';
      const found = w.windows?.find(win => win.appId === target || win.title?.toLowerCase().includes(target));
      if (found) {
        w.focus(found.id);
        return reply || `Brought ${target} to focus.`;
      }
      return reply || `Could not find a window for "${target}".`;
    }

    case 'task':
      // a job the agent works through step by step, checking results
      if (services.startTask) return services.startTask(params.goal || query);
      return reply || "Tasks need the assistant window open.";

    case 'automation':
      // something to do later or on a schedule; the daemon runs it and notifies
      if (services.addAutomation && params.rule) {
        try { const out = await services.addAutomation(params.rule); if (out) return out; }
        catch (e) { return "I couldn't set that up: " + (e.message || e); }
      }
      return reply || 'Automations need the assistant window open.';

    case 'orchestrate': {
      const steps = params.steps || [];
      // The assistant shows the whole plan, takes one approval and runs it step by step.
      if (services.runPlan && steps.length) {
        const out = await services.runPlan(steps, reply, query, w);
        if (out) return out;
      }
      // A plan comes from the LLM, so every state-changing built-in step needs the
      // user's approval up front: one dialog listing the whole plan.
      const needed = steps.map((st) => (services.desktopMode && DESKTOP_INTENTS.has(st.intent)) ? null
        : builtinConfirmation(st.intent, st.params || {}, { source: 'llm' })).filter(Boolean);
      if (needed.length) {
        const ok = await confirmBuiltin({ what: 'run this plan: ' + needed.map((n) => n.what).join('; '),
                                          typed: needed.some((n) => n.typed), untrusted: false }, services);
        if (!ok) return 'Cancelled — none of the steps were run.';
      }
      let results = [];
      for (const step of steps) {
        try {
          const si = step.intent;
          const sp = step.params || {};
          const r = await dispatchIntent(si, sp, '', query, w, services, skipWinId);
          results.push(si);
          await new Promise(r => setTimeout(r, 400));
        } catch (e) {
          results.push('Failed: ' + step.intent);
        }
      }
      return reply || `Done! Executed ${results.length} steps.`;
    }

    case 'search_web': {
      if (w.appRegistry.has('browser')) {
        let bWin = w.windows.find(x => x.appId === 'browser');
        if (!bWin) {
          bWin = w.spawn('browser');
        } else {
          w.focus(bWin.id);
        }
        setTimeout(() => {
          if (bWin && params.query) {
             const input = bWin.contentEl.querySelector('.br-url');
             const goBtn = bWin.contentEl.querySelector('.br-go');
             if (input && goBtn) {
                input.value = params.query;
                goBtn.click();
             }
          }
        }, 500);
      }
      return reply || `Searching the web for "${params.query}".`;
    }

    case 'shutdown':
      // Routed through the daemon so it is confirmed and audited like any OS action.
      return dispatchIntent('os_action', { action: 'power', op: 'shutdown' }, reply, query, w, services, skipWinId);

    case 'files':
      return dispatchIntent('os_action', { ...params, action: 'files', op: params.op || 'search' }, reply, query, w, services, skipWinId);

    case 'os_action': {
      const req = { ...params, action: params.action, op: params.op || '', value: params.value ?? null };
      try {
        const res = await runOsAction(services, req);
        // file results and undoable changes get a card with buttons
        if (req.action === 'files' && services.showFileResult) services.showFileResult(req, res && res.result);
        return describeOsResult(req, res) || reply;
      } catch (e) {
        const msg = e && e.message ? e.message : String(e);
        return msg === 'cancelled' ? 'Cancelled. Nothing was changed.' : `Couldn't do that: ${msg}`;
      }
    }

    case 'schedule_event': {
       const title = params.title || 'Event';
       const dateStr = (params.date || 'Today').trim();
       const timeStr = params.time || 'All Day';
       
       let targetDate = new Date();
       const dsLow = dateStr.toLowerCase();
       if (dsLow.includes('tomorrow')) {
           targetDate.setDate(targetDate.getDate() + 1);
       } else if (!dsLow.includes('today')) {
           // Try parsing the string directly (e.g. "May 25")
           let parsed = new Date(dateStr + " " + targetDate.getFullYear());
           if (!isNaN(parsed.getTime())) {
               targetDate = parsed;
           }
       }
       
       const yyyyMmDd = `${targetDate.getFullYear()}-${String(targetDate.getMonth()+1).padStart(2,'0')}-${String(targetDate.getDate()).padStart(2,'0')}`;

       const events = JSON.parse(localStorage.getItem('osaima-calendar-events') || '{}');
       if (!events[yyyyMmDd]) events[yyyyMmDd] = [];
       events[yyyyMmDd].push({ title, time: timeStr });
       localStorage.setItem('osaima-calendar-events', JSON.stringify(events));

       let cWin = w.windows.find(x => x.appId === 'calendar');
       localStorage.setItem('osaima-calendar-goto', yyyyMmDd);
       
       if (!cWin) {
          w.spawn('calendar');
       } else {
          w.focus(cWin.id);
       }

       return reply || `Scheduled "${title}" on ${yyyyMmDd} at ${timeStr}.`;
    }

    case 'set_gaps':
      if (params.pixels) {
        w.config.gaps = params.pixels;
        w.layout();
      }
      return reply || `Set gaps to ${params.pixels}px.`;

    case 'execute_command':
      if (services.showCommand && services.showCommand(params.plan, params.command || query)) {
        return reply || "Here's the command I'd run. Check it, then choose Run.";
      }
      if (services.widgets) {
        services.widgets.spawnOrchestrator(
          params.plan || 'Execute user command',
          params.command || query
        );
      }
      return reply || "I've prepared the command. Review and execute in the widget.";

    case 'greeting':
    case 'chat':
      return reply || 'Hello! What would you like me to do?';

    case 'create_note': {
      const title = params.title || 'Untitled';
      const content = params.content || '';
      const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      const note = { id: Date.now().toString(36), title, content, createdAt: Date.now(), updatedAt: Date.now(), pinned: false };
      notes.unshift(note);
      localStorage.setItem('osaima-notes', JSON.stringify(notes));
      localStorage.setItem('osaima-notes-select', note.id);
      let nWin = w.windows.find(x => x.appId === 'notes');
      if (!nWin) { w.spawn('notes'); } else { w.focus(nWin.id); }
      return reply || `Created note "${title}".`;
    }

    case 'edit_note': {
      const target = (params.title || '').toLowerCase();
      const newContent = params.content || '';
      const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      const match = notes.find(n => n.title.toLowerCase().includes(target));
      if (!match) return reply || `Could not find a note matching "${params.title}".`;
      match.content = newContent;
      match.updatedAt = Date.now();
      localStorage.setItem('osaima-notes', JSON.stringify(notes));
      localStorage.setItem('osaima-notes-select', match.id);
      let nWin = w.windows.find(x => x.appId === 'notes');
      if (!nWin) { w.spawn('notes'); } else { w.focus(nWin.id); }
      return reply || `Updated note "${match.title}".`;
    }

    case 'delete_note': {
      const target = (params.title || '').toLowerCase();
      let notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      const idx = notes.findIndex(n => n.title.toLowerCase().includes(target));
      if (idx === -1) return reply || `Could not find a note matching "${params.title}".`;
      const removed = notes.splice(idx, 1)[0];
      localStorage.setItem('osaima-notes', JSON.stringify(notes));
      return reply || `Deleted note "${removed.title}".`;
    }

    case 'list_notes': {
      const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      if (notes.length === 0) return reply || 'You have no notes yet. Ask me to create one!';
      const list = notes.slice(0, 10).map((n, i) => `${i + 1}. ${n.title}`).join('\n');
      let nWin = w.windows.find(x => x.appId === 'notes');
      if (!nWin) { w.spawn('notes'); } else { w.focus(nWin.id); }
      return reply || `You have ${notes.length} note(s):\n${list}`;
    }

    case 'view_note': {
      const target = (params.title || '').toLowerCase();
      const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      const match = notes.find(n => n.title.toLowerCase().includes(target));
      if (!match) return reply || `Could not find a note matching "${params.title}".`;
      localStorage.setItem('osaima-notes-select', match.id);
      let nWin = w.windows.find(x => x.appId === 'notes');
      if (!nWin) { w.spawn('notes'); } else { w.focus(nWin.id); }
      const preview = match.content.length > 200 ? match.content.slice(0, 200) + '…' : match.content;
      return reply || `"${match.title}":\n${preview}`;
    }

    case 'search_notes': {
      const q = (params.query || '').toLowerCase();
      const notes = JSON.parse(localStorage.getItem('osaima-notes') || '[]');
      const matches = notes.filter(n => n.title.toLowerCase().includes(q) || n.content.toLowerCase().includes(q));
      if (matches.length === 0) return reply || `No notes matched "${params.query}".`;
      const list = matches.slice(0, 8).map((n, i) => `${i + 1}. ${n.title}`).join('\n');
      return reply || `Found ${matches.length} note(s) matching "${params.query}":\n${list}`;
    }

    case 'unknown':
    default:
      break;
  }
  return null;
}

// ── Calendar App ──────────────────────────────────────────────────────────────
function makeCalendar(services) {
  const MONTHS = ["January","February","March","April","May","June","July","August","September","October","November","December"];
  return {
    id: 'calendar',
    title: 'Calendar',
    icon: '📅',
    width: 700, height: 480,
    mount(root) {
      root.classList.add('app-calendar');
      root.style.cssText = 'background:#0a0d14; color:#fff; padding:24px; display:flex; flex-direction:column; overflow:hidden; font-family:inherit;';

      let viewYear, viewMonth, selYear, selMonth, selDay;
      const now = new Date();
      viewYear = now.getFullYear(); viewMonth = now.getMonth();
      selYear = now.getFullYear(); selMonth = now.getMonth(); selDay = now.getDate();

      const getEvents = () => JSON.parse(localStorage.getItem('osaima-calendar-events') || '{}');
      const saveEvents = (ev) => localStorage.setItem('osaima-calendar-events', JSON.stringify(ev));
      const pad = n => String(n).padStart(2,'0');
      const dateKey = (y,m,d) => `${y}-${pad(m+1)}-${pad(d)}`;

      const render = () => {
        // Check if AI scheduled something
        const goto = localStorage.getItem('osaima-calendar-goto');
        if (goto) {
          const p = goto.split('-');
          if (p.length === 3) {
            viewYear = parseInt(p[0]); viewMonth = parseInt(p[1])-1; selDay = parseInt(p[2]);
            selYear = viewYear; selMonth = viewMonth;
          }
          localStorage.removeItem('osaima-calendar-goto');
        }

        const events = getEvents();
        const today = new Date();
        const selKey = dateKey(selYear, selMonth, selDay);
        const selEvents = events[selKey] || [];
        const firstDow = new Date(viewYear, viewMonth, 1).getDay();
        const dim = new Date(viewYear, viewMonth+1, 0).getDate();

        // Build day grid
        let grid = '';
        for (let i = 0; i < firstDow; i++) grid += '<div></div>';
        for (let d = 1; d <= dim; d++) {
          const dk = dateKey(viewYear, viewMonth, d);
          const isSel = (d === selDay && viewMonth === selMonth && viewYear === selYear);
          const isToday = (d === today.getDate() && viewMonth === today.getMonth() && viewYear === today.getFullYear());
          const hasEv = events[dk] && events[dk].length > 0;
          let bg = '#1a202c';
          if (isSel) bg = '#3b82f6';
          else if (isToday) bg = '#2d3748';
          const bord = isToday && !isSel ? 'border:1px solid #3b82f6;' : '';
          const dot = hasEv ? '<div style="width:5px;height:5px;background:#f43f5e;border-radius:50%;margin:3px auto 0;"></div>' : '';
          grid += `<div class="cd" data-d="${d}" style="background:${bg};${bord}padding:6px 0;border-radius:6px;cursor:pointer;text-align:center;min-height:36px;transition:background .15s;">${d}${dot}</div>`;
        }

        // Build event list with delete buttons
        let evList = '';
        if (selEvents.length) {
          selEvents.forEach((ev, idx) => {
            evList += `<div style="display:flex;align-items:center;gap:8px;background:#1a202c;padding:10px 12px;margin-bottom:6px;border-radius:6px;border-left:3px solid #3b82f6;">
              <div style="flex:1;font-size:13px;"><b style="color:#60a5fa;margin-right:6px;">${ev.time}</b>${escapeHtml(ev.title)}</div>
              <button class="ev-del" data-key="${selKey}" data-idx="${idx}" style="background:none;border:none;color:#ef4444;cursor:pointer;font-size:16px;padding:2px 6px;border-radius:4px;transition:background .15s;" title="Delete event">✕</button>
            </div>`;
          });
        } else {
          evList = '<div style="opacity:0.4;padding:8px;font-size:13px;">No events for this day.</div>';
        }

        const selLabel = new Date(selYear, selMonth, selDay).toLocaleDateString(undefined, {weekday:'short', month:'short', day:'numeric', year:'numeric'});

        root.innerHTML = `
          <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:20px;">
            <div style="display:flex;align-items:center;gap:6px;">
              <button class="mp" style="background:#1e293b;border:none;color:#fff;width:28px;height:28px;border-radius:4px;cursor:pointer;font-size:14px;">◀</button>
              <span style="min-width:160px;text-align:center;font-size:16px;font-weight:500;">${MONTHS[viewMonth]} ${viewYear}</span>
              <button class="mn" style="background:#1e293b;border:none;color:#fff;width:28px;height:28px;border-radius:4px;cursor:pointer;font-size:14px;">▶</button>
            </div>
            <button class="ct" style="background:#1e293b;border:none;color:#60a5fa;padding:5px 14px;border-radius:4px;cursor:pointer;font-size:13px;">Today</button>
          </div>
          <div style="display:flex;gap:20px;flex:1;min-height:0;">
            <div style="flex:1.2;">
              <div style="display:grid;grid-template-columns:repeat(7,1fr);gap:4px;text-align:center;">
                ${['S','M','T','W','T','F','S'].map(h=>`<div style="opacity:0.4;font-size:11px;padding-bottom:6px;">${h}</div>`).join('')}
                ${grid}
              </div>
            </div>
            <div style="flex:1;border-left:1px solid rgba(255,255,255,0.08);padding-left:20px;display:flex;flex-direction:column;min-height:0;">
              <div style="font-size:14px;font-weight:500;margin-bottom:12px;color:#94a3b8;">${selLabel}</div>
              <div style="flex:1;overflow-y:auto;">${evList}</div>
              <div style="font-size:11px;color:#4b5563;padding-top:8px;">✨ Say "schedule meeting on May 25 at 3pm"</div>
            </div>
          </div>`;

        // Wire up nav
        root.querySelector('.mp').onclick = () => { viewMonth--; if(viewMonth<0){viewMonth=11;viewYear--;} render(); };
        root.querySelector('.mn').onclick = () => { viewMonth++; if(viewMonth>11){viewMonth=0;viewYear++;} render(); };
        root.querySelector('.ct').onclick = () => { const n=new Date(); viewYear=n.getFullYear(); viewMonth=n.getMonth(); selYear=viewYear; selMonth=viewMonth; selDay=n.getDate(); render(); };

        // Wire up day clicks
        root.querySelectorAll('.cd').forEach(el => {
          el.onmouseenter = () => { if(el.style.background!=='rgb(59, 130, 246)') el.style.background='#2d3748'; };
          el.onmouseleave = () => { const d=parseInt(el.dataset.d); const isSel=(d===selDay&&viewMonth===selMonth&&viewYear===selYear); if(!isSel) el.style.background='#1a202c'; };
          el.onclick = () => { selDay=parseInt(el.dataset.d); selMonth=viewMonth; selYear=viewYear; render(); };
        });

        // Wire up delete buttons
        root.querySelectorAll('.ev-del').forEach(btn => {
          btn.onmouseenter = () => btn.style.background = 'rgba(239,68,68,0.15)';
          btn.onmouseleave = () => btn.style.background = 'none';
          btn.onclick = () => {
            const key = btn.dataset.key;
            const idx = parseInt(btn.dataset.idx);
            const ev = getEvents();
            if (ev[key]) {
              ev[key].splice(idx, 1);
              if (ev[key].length === 0) delete ev[key];
              saveEvents(ev);
              render();
            }
          };
        });
      };

      render();
      // Poll for AI-scheduled events (via localStorage goto flag)
      const poll = setInterval(() => {
        if (localStorage.getItem('osaima-calendar-goto')) render();
      }, 500);
      root._calInterval = poll;
    },
    unmount(win) {
       const cal = win.el.querySelector('.app-calendar');
       if (cal && cal._calInterval) clearInterval(cal._calInterval);
    }
  };
}

