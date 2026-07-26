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

/**
 * @param {import('./window-manager.js').WindowManager} engine
 * @param {object} services  { invoke, getConfigSource, setConfigSource, reloadConfig }
 */
export function registerApps(engine, services) {
  engine.registerApp(makeTerminal(services));
  engine.registerApp(makeMonitor(services));
  engine.registerApp(makeFiles());
  engine.registerApp(makeConfigEditor(engine, services));
  engine.registerApp(makeAbout(engine));
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

      const print = (text, cls) => {
        const line = el('div', 'term-line' + (cls ? ' ' + cls : ''));
        line.textContent = text;
        output.appendChild(line);
        output.scrollTop = output.scrollHeight;
      };

      print('Interstellar OS — agentic shell v0.1', 'term-dim');
      print("Type 'help' for commands.", 'term-dim');

      const commands = {
        help: () => print(
          'help  stats  ping  ws  layout  spawn <app>  apps  clear  echo <x>  date  neofetch'),
        clear: () => { output.innerHTML = ''; },
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
        if (fn) await fn(args);
        else print(`command not found: ${cmd}`, 'term-err');
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
      const timer = setInterval(tick, 1500);
      ctx.win._monitorTimer = timer;
    },
    unmount(win) { if (win._monitorTimer) clearInterval(win._monitorTimer); },
  };
}

// ── Files (mock explorer) ────────────────────────────────────────────────────
function makeFiles() {
  const tree = {
    '/': ['bin/', 'etc/', 'home/', 'usr/', 'var/', 'interstellar.conf'],
    '/home/': ['user/'],
    '/home/user/': ['Documents/', 'Projects/', 'wm.lua', 'notes.md', '.bashrc'],
    '/home/user/Projects/': ['osaima-shell/', 'kernel-sched/', 'mcp-daemon/'],
    '/etc/': ['portage/', 'interstellar/', 'fstab', 'hostname'],
  };
  return {
    id: 'files',
    title: 'Files',
    icon: '🗂',
    width: 480, height: 340,
    mount(root) {
      root.classList.add('app-files');
      const path = el('div', 'files-path');
      const list = el('div', 'files-list');
      root.append(path, list);
      let cwd = '/';
      const render = () => {
        path.textContent = cwd;
        list.innerHTML = '';
        if (cwd !== '/') {
          const up = el('div', 'files-item files-dir', '.. ');
          up.addEventListener('click', () => {
            const parts = cwd.replace(/\/$/, '').split('/');
            parts.pop();
            cwd = (parts.join('/') || '') + '/';
            if (cwd === '/') cwd = '/';
            render();
          });
          list.appendChild(up);
        }
        const entries = tree[cwd] || [];
        for (const name of entries) {
          const isDir = name.endsWith('/');
          const item = el('div', 'files-item ' + (isDir ? 'files-dir' : 'files-file'),
            (isDir ? '📁 ' : '📄 ') + name);
          if (isDir) item.addEventListener('click', () => { cwd = cwd + name; render(); });
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
    icon: '⚙',
    width: 600, height: 460,
    floating: true,
    mount(root) {
      root.classList.add('app-config');
      const toolbar = el('div', 'cfg-toolbar');
      const reloadBtn = el('button', 'cfg-btn cfg-reload', '⟳ Reload');
      const status = el('span', 'cfg-status', 'Live Lua configuration — edit and reload');
      toolbar.append(reloadBtn, status);

      const ta = document.createElement('textarea');
      ta.className = 'cfg-editor';
      ta.spellcheck = false;
      ta.value = services.getConfigSource();

      root.append(toolbar, ta);

      reloadBtn.addEventListener('click', async () => {
        const result = await services.reloadConfig(ta.value);
        if (result.ok) {
          status.textContent = '✓ Applied at ' + new Date().toLocaleTimeString();
          status.className = 'cfg-status ok';
        } else {
          status.textContent = '✗ ' + result.error;
          status.className = 'cfg-status err';
        }
      });
      // Ctrl+Enter to reload
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
