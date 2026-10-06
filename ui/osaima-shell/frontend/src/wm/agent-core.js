/**
 * agent-core.js — the proactive "AI Core" of Interstellar OS.
 *
 * Where the AI Assistant app is reactive (you ask, it acts), this is the
 * *proactive* side of the agentic OS: a background loop that periodically looks
 * at system stats (via the MCP daemon) and the current window-manager state,
 * reasons over a small ruleset, and — when it has something genuinely useful to
 * say — surfaces a single actionable suggestion. One click carries out the
 * action (switch layout, open Task Manager, tidy windows, …).
 *
 * Deliberately conservative: one suggestion at a time, per-rule cooldowns, and a
 * global quiet period so it never nags. This is the hook where a real LLM would
 * later replace the ruleset.
 */

import { DEFAULTS } from './shell-settings.js';

export class AgentCore {
  /**
   * @param {object} opts
   * @param {import('./window-manager.js').WindowManager} opts.engine
   * @param {Function} opts.invoke   MCP invoke(cmd)
   * @param {Function} opts.suggest  (message, { actionLabel, onAction }) => void
   */
  constructor({ engine, invoke, suggest, behavior = null }) {
    this.engine = engine;
    this.invoke = invoke;
    this.suggest = suggest;
    this.behavior = behavior;

    this.lastFiredAt = new Map();   // rule id -> timestamp
    this.lastAnyAt = 0;             // last time ANY suggestion fired
    this.settings = DEFAULTS;       // thresholds and pacing; see shell-settings.js
    this.timer = null;

    this.rules = this.buildRules();
  }

  /** Begin watching. A short first delay lets the desktop settle before the agent speaks up. */
  start(settings = DEFAULTS) {
    this.configure(settings);
    setTimeout(() => this.tick(), settings.proactiveFirstMs);
  }

  /** Apply new settings (also used when the Lua config is reloaded). */
  configure(settings) {
    this.settings = settings;
    this.stop();
    if (settings.proactiveEnabled) {
      this.timer = setInterval(() => this.tick(), settings.proactiveIntervalMs);
    }
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  async tick() {
    const now = Date.now();
    if (!this.settings.proactiveEnabled) return;
    if (now - this.lastAnyAt < this.settings.proactiveQuietMs) return;

    // Gather the context the rules reason over.
    let stats = null;
    try { stats = await this.invoke('get_system_stats'); } catch {}
    const w = this.engine;
    const ctx = {
      stats,
      cpu: stats ? stats.cpu.usage_percent : 0,
      memPct: stats ? (stats.memory.used_bytes / stats.memory.total_bytes) * 100 : 0,
      layout: w.config.layout,
      tiledCount: w.windows.filter((x) => x.workspace === w.currentWs && !x.minimized && !x.floating).length,
      visibleCount: w.visibleWindows().length,
      now,
    };

    // Evaluate rules in priority order; fire the first eligible one.
    for (const rule of this.rules) {
      const last = this.lastFiredAt.get(rule.id) || 0;
      if (now - last < rule.cooldown) continue;
      const suggestion = rule.evaluate(ctx);
      if (suggestion) {
        this.lastFiredAt.set(rule.id, now);
        this.lastAnyAt = now;
        this.suggest(suggestion.message, {
          actionLabel: suggestion.actionLabel,
          onAction: suggestion.onAction,
        });
        return; // one at a time
      }
    }
  }

  buildRules() {
    const w = this.engine;
    const b = this.behavior;
    return [
      {
        // Behavior-learned: offer the app you most often open first this session.
        id: 'learned-next-app',
        cooldown: 70000,
        evaluate: (c) => {
          if (!b || c.visibleCount > 0) return null;
          const next = b.predictNextApp();
          if (!next || w.windows.some((x) => x.appId === next)) return null;
          const name = (w.appRegistry.get(next) || {}).title || next;
          return {
            message: `AI Core: you usually start with ${name}. Open it?`,
            actionLabel: 'Open ' + name,
            onAction: () => w.spawn(next),
          };
        },
      },
      {
        id: 'high-cpu',
        cooldown: 45000,
        evaluate: (c) => c.cpu > this.settings.cpuAlertPercent ? {
          message: `AI Core: CPU is high (${c.cpu.toFixed(0)}%). Want to see what's running?`,
          actionLabel: 'Open Task Manager',
          onAction: () => w.spawn('taskmanager'),
        } : null,
      },
      {
        id: 'high-mem',
        cooldown: 60000,
        evaluate: (c) => c.memPct > this.settings.memoryAlertPercent ? {
          message: `AI Core: memory usage is at ${c.memPct.toFixed(0)}%. I can open the monitor.`,
          actionLabel: 'Open Monitor',
          onAction: () => w.spawn('monitor'),
        } : null,
      },
      {
        id: 'suggest-grid',
        cooldown: 40000,
        evaluate: (c) => (c.tiledCount >= this.settings.gridSuggestWindows && c.layout === 'tile') ? {
          message: `AI Core: you have ${c.tiledCount} windows open — a grid layout may fit better.`,
          actionLabel: 'Use grid',
          onAction: () => w.setLayout('grid'),
        } : null,
      },
      {
        id: 'empty-desktop',
        cooldown: 50000,
        evaluate: (c) => c.visibleCount === 0 ? {
          message: 'AI Core: your desktop is empty. Shall I open a terminal to get started?',
          actionLabel: 'Open Terminal',
          onAction: () => w.spawn('terminal'),
        } : null,
      },
      {
        id: 'tips',
        cooldown: 90000,
        evaluate: () => {
          const tip = TIPS[Math.floor(Math.random() * TIPS.length)];
          return {
            message: 'AI Core tip: ' + tip.text,
            actionLabel: tip.actionLabel,
            onAction: tip.onAction ? () => tip.onAction(w) : (() => {}),
          };
        },
      },
    ];
  }
}

const TIPS = [
  { text: 'You can talk to me in plain English — try the AI Assistant.', actionLabel: 'Open Assistant', onAction: (w) => w.spawn('assistant') },
  { text: 'Press Alt+Tab to cycle window layouts.', actionLabel: 'Cycle now', onAction: (w) => w.cycleLayout() },
  { text: 'The whole window manager is configured in live Lua.', actionLabel: 'Edit config', onAction: (w) => w.spawn('config') },
  { text: 'Alt+1..5 jumps between workspaces.', actionLabel: 'Got it' },
];
