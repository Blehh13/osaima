/**
 * widget-manager.js — Desktop Widgets for OSAIMA
 * 
 * Handles spawning and managing mini-widgets (Timers, Reminders, etc.)
 */

export class WidgetManager {
  constructor(services) {
    this.services = services;
    this.layer = document.getElementById('widgets-layer');
    this.widgets = new Map();
    this.seq = 0;
  }

  createWidget(opts) {
    this.seq++;
    const id = 'widget-' + this.seq;
    
    const el = document.createElement('div');
    el.className = 'widget-card expanded';
    el.id = id;

    // Layout position (stacking down the right side, or random)
    // For now, let's just place them slightly offset from top-right.
    const offset = (this.widgets.size % 5) * 20;
    el.style.top = (80 + offset) + 'px';
    el.style.right = (40 + offset) + 'px';

    const header = document.createElement('div');
    header.className = 'widget-header';
    header.innerHTML = `
      <div class="widget-title">
        <span class="widget-icon">${opts.icon || '✦'}</span>
        ${opts.title}
      </div>
      <div class="widget-controls">
        <button class="w-btn w-collapse">−</button>
        <button class="w-btn w-close">×</button>
      </div>
    `;

    const body = document.createElement('div');
    body.className = 'widget-body';
    
    if (opts.render) {
      opts.render(body, id);
    }

    el.appendChild(header);
    el.appendChild(body);
    this.layer.appendChild(el);

    // Make Draggable (simple implementation)
    let isDragging = false, startX, startY, initX, initY;
    header.addEventListener('mousedown', (e) => {
      if (e.target.closest('.widget-controls')) return;
      isDragging = true;
      startX = e.clientX; startY = e.clientY;
      const rect = el.getBoundingClientRect();
      initX = rect.left; initY = rect.top;
      el.style.right = 'auto'; // Switch to left/top positioning
      el.style.left = initX + 'px';
      el.style.top = initY + 'px';
      // Bring to front
      this.layer.appendChild(el);
    });
    window.addEventListener('mousemove', (e) => {
      if (!isDragging) return;
      el.style.left = (initX + (e.clientX - startX)) + 'px';
      el.style.top = (initY + (e.clientY - startY)) + 'px';
    });
    window.addEventListener('mouseup', () => { isDragging = false; });

    // Controls
    header.querySelector('.w-collapse').addEventListener('click', () => {
      el.classList.toggle('expanded');
      el.classList.toggle('collapsed');
    });
    header.querySelector('.w-close').addEventListener('click', () => {
      el.style.transform = 'scale(0.9)';
      el.style.opacity = '0';
      setTimeout(() => {
        el.remove();
        this.widgets.delete(id);
      }, 300);
    });

    this.widgets.set(id, { el, body });
    return id;
  }

  // 1. Timer Widget
  spawnTimer(durationSeconds) {
    let left = durationSeconds;
    let interval = null;
    this.createWidget({
      title: 'Timer',
      icon: '⏱️',
      render: (body, id) => {
        body.innerHTML = `
          <div class="timer-display"></div>
          <div class="timer-controls">
            <button class="t-btn t-pause">Pause</button>
            <button class="t-btn t-stop">Stop</button>
          </div>
        `;
        const disp = body.querySelector('.timer-display');
        const update = () => {
          const m = Math.floor(left / 60).toString().padStart(2, '0');
          const s = (left % 60).toString().padStart(2, '0');
          disp.textContent = `${m}:${s}`;
          if (left <= 0) {
            clearInterval(interval);
            disp.textContent = "00:00";
            disp.style.color = "var(--status-offline)";
            // Play sound or notification
            try { this.services.invoke('trigger_notification', { title: "Timer Finished!", body: "Your timer has ended." }); } catch(e){}
          }
        };
        update();
        interval = setInterval(() => { if (left > 0) { left--; update(); } }, 1000);

        let paused = false;
        body.querySelector('.t-pause').addEventListener('click', (e) => {
          paused = !paused;
          e.target.textContent = paused ? 'Resume' : 'Pause';
          if (paused) clearInterval(interval);
          else interval = setInterval(() => { if (left > 0) { left--; update(); } }, 1000);
        });
        body.querySelector('.t-stop').addEventListener('click', () => {
          clearInterval(interval);
          this.widgets.get(id).el.querySelector('.w-close').click();
        });
      }
    });
  }

  clearTimers() {
    this.widgets.forEach((w, id) => {
      if (w.el.querySelector('.w-title').textContent.includes('TIMER')) {
        w.el.querySelector('.w-close').click();
      }
    });
  }

  // 2. Reminder Widget
  spawnReminder(text) {
    this.createWidget({
      title: 'Reminder',
      icon: '📌',
      render: (body) => {
        body.innerHTML = `
          <div class="reminder-text">${text}</div>
          <button class="reminder-done">Mark as Done</button>
        `;
        body.querySelector('.reminder-done').addEventListener('click', (e) => {
          e.target.closest('.widget-card').querySelector('.w-close').click();
        });
      }
    });
  }

  // 3. Calendar Widget
  spawnCalendar() {
    this.createWidget({
      title: 'Calendar',
      icon: '📅',
      render: (body) => {
        const d = new Date();
        body.innerHTML = `
          <div class="calendar-header">${d.toLocaleString('default', { month: 'long' })} ${d.getFullYear()}</div>
          <div class="calendar-grid">
            <span>S</span><span>M</span><span>T</span><span>W</span><span>T</span><span>F</span><span>S</span>
            <!-- Simple mock grid -->
            <span></span><span></span><span>1</span><span>2</span><span>3</span><span>4</span><span>5</span>
            <span>6</span><span>7</span><span>8</span><span>9</span><span>10</span><span>11</span><span>12</span>
            <span>13</span><span>14</span><span class="cal-today">15</span><span>16</span><span>17</span><span>18</span><span>19</span>
            <span>20</span><span>21</span><span>22</span><span>23</span><span>24</span><span>25</span><span>26</span>
            <span>27</span><span>28</span><span>29</span><span>30</span><span>31</span><span></span><span></span>
          </div>
        `;
      }
    });
  }

  // 4. Calculator Widget
  spawnCalculator(initialResult = "") {
    this.createWidget({
      title: 'Calculator',
      icon: '🧮',
      render: (body) => {
        body.innerHTML = `
          <div class="calc-display">${initialResult || "0"}</div>
          <div class="calc-hint">Evaluated by AI Core.</div>
        `;
      }
    });
  }

  // 5. Weather Widget
  spawnWeather() {
    this.createWidget({
      title: 'Weather',
      icon: '⛅',
      render: (body) => {
        body.innerHTML = `
          <div class="weather-temp">72°F</div>
          <div class="weather-cond">Partly Cloudy</div>
          <div class="weather-loc">San Francisco, CA</div>
        `;
      }
    });
  }

  // 6. System Vitals Widget
  spawnVitals() {
    const id = this.createWidget({
      title: 'System Vitals',
      icon: '📈',
      render: (body) => {
        body.innerHTML = `
          <div class="vitals-row"><span>CPU</span> <div class="vital-bar"><div class="vital-fill" id="vit-cpu"></div></div></div>
          <div class="vitals-row"><span>MEM</span> <div class="vital-bar"><div class="vital-fill" id="vit-mem"></div></div></div>
        `;
      }
    });
    
    // Auto-update vitals
    const update = async () => {
      const w = this.widgets.get(id);
      if (!w) return; // widget closed
      try {
        const s = await this.services.invoke('get_system_stats');
        const cpuFill = w.body.querySelector('#vit-cpu');
        const memFill = w.body.querySelector('#vit-mem');
        if (cpuFill) cpuFill.style.width = Math.min(100, s.cpu.usage_percent) + '%';
        if (memFill) memFill.style.width = Math.min(100, (s.memory.used_bytes / s.memory.total_bytes) * 100) + '%';
      } catch(e) {}
      setTimeout(update, 2000);
    };
    update();
  }

  // 7. Scratchpad Widget
  spawnScratchpad() {
    this.createWidget({
      title: 'Scratchpad',
      icon: '📝',
      render: (body) => {
        body.innerHTML = `<textarea class="scratch-area" placeholder="Jot down a quick note..."></textarea>`;
      }
    });
  }

  // 8. Agentic Orchestrator (Developer Tool)
  spawnOrchestrator(planDescription, commandToRun) {
    this.createWidget({
      title: 'Agentic Execution',
      icon: '⚡',
      render: (body, id) => {
        // Plan and command come from the LLM: only ever insert them as text,
        // never as HTML, or a prompt injection could script the shell.
        body.innerHTML = `
          <div class="orch-plan"><b>Plan:</b> <span class="orch-plan-text"></span></div>
          <div class="orch-cmd"><code></code></div>
          <div class="orch-risk">Checking risk…</div>
          <input class="orch-typed" type="text" autocomplete="off" spellcheck="false" hidden>
          <div class="orch-term" id="orch-term-${id}"></div>
          <div class="orch-controls">
            <button class="orch-btn orch-run" disabled>Execute</button>
            <button class="orch-btn orch-cancel">Cancel</button>
          </div>
        `;
        body.querySelector('.orch-plan-text').textContent = planDescription;
        body.querySelector('.orch-cmd code').textContent = commandToRun;
        const termEl = body.querySelector('.orch-term');
        const runBtn = body.querySelector('.orch-run');
        const riskEl = body.querySelector('.orch-risk');
        const typedEl = body.querySelector('.orch-typed');

        // The daemon decides the tier (and re-checks it on execute); the UI only
        // reflects it: read-only runs on click, reversible needs this click,
        // irreversible also needs the confirm phrase typed.
        const TIERS = {
          read_only: ['Read-only', '#5cc28f'],
          reversible: ['Reversible: changes your files', '#e8b04a'],
          irreversible: ['Irreversible', '#ef6b6b'],
        };
        let tier = null;
        let phrase = 'confirm';
        this.services.invoke('assess_command', { command: commandToRun }).then((a) => {
          if (!a || !TIERS[a.tier]) throw new Error('no assessment');
          tier = a.tier;
          phrase = a.confirm_phrase || phrase;
          const [label, color] = TIERS[tier];
          riskEl.textContent = label + (a.reasons && a.reasons.length ? ' · ' + a.reasons.join('; ') : '');
          riskEl.style.color = color;
          if (tier === 'irreversible') {
            typedEl.hidden = false;
            typedEl.placeholder = `Type "${phrase}" to allow this`;
            typedEl.addEventListener('input', () => {
              runBtn.disabled = typedEl.value.trim().toLowerCase() !== phrase;
            });
          } else {
            runBtn.disabled = false;
          }
        }).catch(() => {
          // Fail closed: without the daemon there is no sandbox or audit log.
          riskEl.textContent = 'AI core is offline, so this command cannot be run safely.';
          riskEl.style.color = '#ef6b6b';
        });

        let hasRun = false;
        runBtn.addEventListener('click', async () => {
          if (hasRun) {
            this.widgets.get(id).el.querySelector('.w-close').click();
            return;
          }
          runBtn.disabled = true;
          typedEl.disabled = true;
          runBtn.textContent = 'Running...';
          termEl.textContent = 'Executing in sandbox...\n';

          try {
            const res = await this.services.invoke('execute_ai_command', {
              command: commandToRun,
              approved: true,
              typed: tier === 'irreversible' ? typedEl.value : null,
            });
            if (!res) throw new Error('No response from AI core');
            // Strip ANSI escape codes
            const output = (res.output || '').replace(/\x1b\[[0-9;]*m/g, '').replace(/\x1b\[\?25[hl]/g, '');
            termEl.textContent += output + `\n\n[Exit ${res.exit_code ?? '?'} · sandbox: ${res.sandbox} · logged]`;
            runBtn.textContent = 'Done';
            runBtn.disabled = false;
            hasRun = true;
          } catch (e) {
            termEl.textContent += '\n\n[Error] ' + (e && e.message ? e.message : e);
            runBtn.textContent = 'Failed';
          }
        });
        
        body.querySelector('.orch-cancel').addEventListener('click', () => {
          this.widgets.get(id).el.querySelector('.w-close').click();
        });
      }
    });
  }
}
