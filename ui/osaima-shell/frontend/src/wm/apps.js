/**
 * apps.js — built-in applications for the Interstellar OS shell.
 *
 * Each app is a small definition the window manager can spawn. Apps get a
 * content element to render into plus a context ({ wm, win }). A shared
 * `services` object wires them to the rest of the shell (MCP IPC, the live Lua
 * config, etc.).
 *
 * Apps kept intentionally lightweight and self-contained so the WM has real,
 * varied windows to tile, float, and move across workspaces during a demo.
 */

import { makeAssistant } from './assistant.js';

/**
 * @param {import('./window-manager.js').WindowManager} engine
 * @param {object} services  { invoke, getConfigSource, setConfigSource, reloadConfig }
 */
export function registerApps(engine, services) {
  engine.registerApp(makeTerminal(services));
  engine.registerApp(makeBrowser(services));
  engine.registerApp(makeMonitor(services));
  engine.registerApp(makeFiles(services));
  engine.registerApp(makeConfigEditor(engine, services));
  engine.registerApp(makeAbout(engine));
  engine.registerApp(makeAssistant(engine, services));
  engine.registerApp(makeTaskManager(services));
  engine.registerApp(makeKnowledge(services));
  engine.registerApp(makeBehavior(engine, services));
}

// ── Terminal ──────────────────────────────────────────────────────────────
function makeTerminal(services) {
  return {
    id: 'terminal',
    title: 'Terminal',
    icon: '❯',
    width: 560, height: 380,
    mount(root, ctx) {
      root.classList.add('app-terminal');
      const output = el('div', 'term-output');
      const inputLine = el('div', 'term-inputline');
      const prompt = el('span', 'term-prompt', 'interstellar ❯ ');
      const input = document.createElement('input');
      input.className = 'term-input';
      input.spellcheck = false;
      input.autocomplete = 'off';
      inputLine.append(prompt, input);
      root.append(output, inputLine);

      const history = [];
      let hIdx = 0;
      let cwd = null; // null = the user's home directory
      const setCwd = (dir) => {
        cwd = dir;
        const base = dir === '/' ? '/' : dir.split('/').pop();
        prompt.textContent = base + ' ❯ ';
      };
      services.invoke('home_dir').then(setCwd).catch(() => {});

      const print = (text, cls) => {
        const line = el('div', 'term-line' + (cls ? ' ' + cls : ''));
        line.textContent = text;
        output.appendChild(line);
        output.scrollTop = output.scrollHeight;
      };

      print('Interstellar OS — agentic shell v0.1', 'term-dim');
      print("Real shell: try ls, ps aux, uname -a, cat /etc/os-release.  'help' for built-ins.", 'term-dim');

      const commands = {
        help: () => {
          print('built-ins: help  cd <dir>  stats  ping  ws  layout  spawn <app>  apps  clear  echo <x>  date  neofetch');
          print('anything else runs as a REAL command on Interstellar OS (ls, ps, uname, emerge, …)', 'term-dim');
        },
        clear: () => { output.innerHTML = ''; },
        cd: async (args) => {
          // Single-quote the path for /bin/sh; a leading ~ expands to $HOME.
          const target = args.length ? args.join(' ') : '~';
          const home = target === '~' || target.startsWith('~/');
          const rest = home ? target.slice(1) : target;
          const quoted = (home ? '"$HOME"' : '') + "'" + rest.replace(/'/g, "'\\''") + "'";
          try {
            const res = await services.invoke('run_command', { cmd: 'cd ' + quoted + ' && pwd', cwd });
            if (res.exit_code === 0) setCwd(res.output.trim());
            else print(res.output.trim() || 'cd: no such directory', 'term-err');
          } catch (e) { print('cd: ' + e, 'term-err'); }
        },
        echo: (args) => print(args.join(' ')),
        date: () => print(new Date().toString()),
        apps: () => print([...ctx.wm.appRegistry.keys()].join('  ')),
        ws: () => {
          const w = ctx.wm;
          print('workspace ' + (w.currentWs + 1) + '/' + w.workspaces.length +
            '  (' + w.workspaces.map((x) => x.name).join(' ') + ')');
        },
        layout: () => print('layout: ' + ctx.wm.config.layout +
          '  gaps=' + ctx.wm.config.gaps + '  master=' + ctx.wm.config.masterRatio.toFixed(2)),
        spawn: (args) => {
          if (!args[0]) return print('usage: spawn <app>', 'term-err');
          const w = ctx.wm.spawn(args[0]);
          print(w ? 'spawned ' + args[0] : 'no such app: ' + args[0], w ? '' : 'term-err');
        },
        stats: async () => {
          try {
            const s = await services.invoke('get_system_stats');
            const memPct = ((s.memory.used_bytes / s.memory.total_bytes) * 100).toFixed(1);
            print(`host   ${s.host}`);
            print(`os     ${s.os}`);
            print(`kernel ${s.kernel}`);
            print(`cpu    ${s.cpu.usage_percent.toFixed(1)}% / ${s.cpu.cores} cores`);
            print(`mem    ${memPct}% (${fmtBytes(s.memory.used_bytes)} / ${fmtBytes(s.memory.total_bytes)})`);
          } catch { print('error: MCP daemon unreachable', 'term-err'); }
        },
        ping: async () => {
          const alive = await services.invoke('ping_daemon');
          print(alive ? '✓ AI Core online' : '✗ AI Core offline', alive ? 'term-ok' : 'term-err');
        },
        neofetch: async () => {
          let s;
          try { s = await services.invoke('get_system_stats'); } catch { s = null; }
          print('      .   *        Interstellar OS', 'term-ok');
          print('   *  ⨀  .        ----------------', 'term-ok');
          print('  .        *      kernel  ' + (s ? s.kernel : '6.x-interstellar'), 'term-ok');
          print('     *  .   *     wm      lua-tiling', 'term-ok');
          print('  .    *     .    shell   osaima-shell', 'term-ok');
        },
      };

      const run = async (raw) => {
        const line = raw.trim();
        print(prompt.textContent + line, 'term-echo');
        if (!line) return;
        history.push(line); hIdx = history.length;
        const [cmd, ...args] = line.split(/\s+/);
        const fn = commands[cmd];
        if (fn) { await fn(args); return; }
        // Not a built-in — run it as a REAL command on the OS.
        try {
          const res = await services.invoke('run_command', { cmd: line, cwd });
          if (res.output) res.output.replace(/\n$/, '').split('\n').forEach((l) => print(l));
          if (res.truncated) print('[output truncated]', 'term-dim');
          if (res.timed_out) print('[stopped after 30 s: interactive programs are not supported yet]', 'term-err');
          else if (res.exit_code) print('[exit ' + res.exit_code + ']', 'term-dim');
        } catch (e) {
          print('error: ' + e, 'term-err');
        }
      };

      input.addEventListener('keydown', async (e) => {
        if (e.key === 'Enter') { const v = input.value; input.value = ''; await run(v); }
        else if (e.key === 'ArrowUp') { if (hIdx > 0) input.value = history[--hIdx]; e.preventDefault(); }
        else if (e.key === 'ArrowDown') { if (hIdx < history.length - 1) input.value = history[++hIdx]; else { hIdx = history.length; input.value = ''; } e.preventDefault(); }
      });
      // Focus the input when the terminal window is clicked.
      root.addEventListener('mousedown', () => setTimeout(() => input.focus(), 0));
      setTimeout(() => input.focus(), 30);
    },
  };
}

// ── Web Browser ───────────────────────────────────────────────────────────────
// A real web browser inside the shell: address bar + iframe view + a start page
// with bookmarks. It's a WebKit webview, so this is genuine web browsing.
function makeBrowser(services) {
  const HOME = 'about:home';
  // Bookmarks and the search engine come from the Lua config (wm.shell{...}).
  return {
    id: 'browser',
    title: 'Browser',
    icon: '🌐',
    width: 760, height: 540,
    mount(root) {
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
      const frame = document.createElement('iframe');
      frame.className = 'br-frame';
      frame.setAttribute('sandbox', 'allow-scripts allow-forms allow-same-origin allow-popups');
      const startPage = el('div', 'br-home');
      view.append(startPage, frame);
      root.append(bar, view);

      const renderHome = () => {
        frame.style.display = 'none';
        frame.removeAttribute('src');
        startPage.style.display = '';
        startPage.innerHTML = `
          <div class="br-logo">✦ Interstellar Browser</div>
          <input class="br-search" placeholder="Search the web…" spellcheck="false" autocomplete="off" />
          <div class="br-tiles"></div>
          <div class="br-note">Tip: some large sites (Google, YouTube) refuse to be embedded — that's their own security policy, not a shell bug. The bookmarks above load fine, and any address you type works.</div>`;
        const tiles = startPage.querySelector('.br-tiles');
        for (const b of services.settings().bookmarks) {
          const t = el('button', 'br-tile', b.name);
          t.addEventListener('click', () => navigate(b.url));
          tiles.append(t);
        }
        const search = startPage.querySelector('.br-search');
        search.addEventListener('keydown', (e) => { if (e.key === 'Enter') navigate(e.target.value); });
        setTimeout(() => search.focus(), 30);
      };

      const normalize = (raw) => {
        const s = (raw || '').trim();
        if (!s) return null;
        if (s === HOME) return HOME;
        if (/^https?:\/\//i.test(s)) return s;
        if (/^[\w-]+(\.[\w-]+)+(\/.*)?$/.test(s)) return 'https://' + s;
        return services.settings().searchUrl.replace('%s', encodeURIComponent(s));
      };

      const navigate = (raw) => {
        const target = normalize(raw);
        if (!target) return;
        if (target === HOME) { url.value = ''; renderHome(); return; }
        url.value = target;
        startPage.style.display = 'none';
        frame.style.display = '';
        frame.src = target;
      };

      back.addEventListener('click', () => { try { frame.contentWindow.history.back(); } catch {} });
      fwd.addEventListener('click', () => { try { frame.contentWindow.history.forward(); } catch {} });
      reload.addEventListener('click', () => { if (frame.getAttribute('src')) frame.src = frame.src; });
      homeBtn.addEventListener('click', () => navigate(HOME));
      go.addEventListener('click', () => navigate(url.value));
      url.addEventListener('keydown', (e) => { if (e.key === 'Enter') navigate(url.value); });

      renderHome();
    },
  };
}

// ── System Monitor ──────────────────────────────────────────────────────────
function makeMonitor(services) {
  return {
    id: 'monitor',
    title: 'System Monitor',
    icon: '📊',
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
      const timer = setInterval(tick, services.settings().monitorPollMs);
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
    icon: '🗂',
    width: 520, height: 380,
    mount(root) {
      root.classList.add('app-files');
      const path = el('div', 'files-path');
      const list = el('div', 'files-list');
      root.append(path, list);
      let cwd = '/';
      const start = services.invoke('home_dir').then((h) => { cwd = h; }).catch(() => {});

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
      start.then(render);
    },
  };
}

// ── Live Lua config editor ───────────────────────────────────────────────────
function makeConfigEditor(engine, services) {
  return {
    id: 'config',
    title: 'wm.lua — Config',
    icon: '⚙',
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
    title: 'About Interstellar OS',
    icon: '✦',
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
            <div class="ab-title">Interstellar OS</div>
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

// ── Task Manager (ML-prioritized processes) ──────────────────────────────────
// Nods to the kernel's ML-based scheduling vision: each process shows an
// ML-assigned priority the "scheduler" adjusts over time.
function makeTaskManager(services) {
  return {
    id: 'taskmanager',
    title: 'Task Manager',
    icon: '⚡',
    width: 580, height: 420,
    mount(root, ctx) {
      root.classList.add('app-tasks');
      root.innerHTML = `
        <div class="tk-toolbar">
          <span class="tk-title">Processes — live from /proc</span>
          <span class="tk-hint">click a row · Lower priority / End</span>
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
          <button class="tk-btn tk-boost" disabled>⬇ Lower priority</button>
          <button class="tk-btn tk-kill" disabled>✕ End process</button>
          <span class="tk-status"></span>
        </div>`;

      const rowsEl = root.querySelector('.tk-rows');
      const boostBtn = root.querySelector('.tk-boost');
      const killBtn = root.querySelector('.tk-kill');
      const statusEl = root.querySelector('.tk-status');
      const status = (msg) => { statusEl.textContent = msg; };

      let procs = [];
      let selectedPid = null;

      const render = () => {
        rowsEl.innerHTML = '';
        for (const p of procs) {
          const memMiB = p.memory_bytes / 1048576;
          const load = Math.max(0.02, Math.min(1, p.cpu_percent / 40 + memMiB / 4000));
          const pct = Math.round(load * 100);
          const cls = load > 0.66 ? 'hi' : load > 0.33 ? 'mid' : 'lo';
          const row = el('div', 'tk-row' + (p.pid === selectedPid ? ' selected' : ''));
          row.innerHTML = `
            <span class="tk-c-pid">${p.pid}</span>
            <span class="tk-c-name">${escapeHtml(p.name)}</span>
            <span class="tk-c-cpu">${p.cpu_percent.toFixed(1)}</span>
            <span class="tk-c-mem">${memMiB.toFixed(0)}M</span>
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
        } catch (e) { status('AI Core unavailable: ' + e); return; }
        render();
      };

      boostBtn.addEventListener('click', async () => {
        if (!selectedPid) return;
        // Raising priority needs root; lowering your own process's priority does not.
        try {
          const res = await services.invoke('run_command', { cmd: 'renice -n 10 -p ' + Number(selectedPid) });
          status(res.exit_code === 0 ? 'Lowered priority of ' + selectedPid : res.output.trim());
        } catch (e) { status(String(e)); }
        load();
      });
      killBtn.addEventListener('click', async () => {
        if (!selectedPid) return;
        try {
          await services.invoke('kill_process', { pid: selectedPid });
          status('Ended process ' + selectedPid);
        } catch (e) { status(String(e)); }
        selectedPid = null;
        boostBtn.disabled = killBtn.disabled = true;
        load();
      });

      load();
      const timer = setInterval(load, services.settings().taskPollMs);
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
    icon: '📚',
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
    icon: '🧠',
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
