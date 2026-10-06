/**
 * assistant-rules.js — the original keyword-based assistant.
 *
 * It is the fallback when the agent service (and so any language model) isn't
 * running: simple window-manager commands and a system summary still work.
 */

const APP_ALIASES = {
  browser: ['browser', 'web', 'internet', 'chrome', 'website'],
  terminal: ['terminal', 'shell', 'console', 'command'],
  files: ['files', 'file', 'explorer', 'folder'],
  monitor: ['monitor', 'system monitor', 'usage', 'graph'],
  taskmanager: ['task', 'process', 'task manager', 'processes'],
  config: ['config', 'settings', 'wm.lua', 'lua'],
  about: ['about', 'keybind', 'shortcut', 'help window'],
};

/**
 * @param {string} query
 * @param {{ wm: object, services: object }} context
 * @returns {Promise<string>}
 */
export async function ruleBasedReply(query, { wm, services }) {
  const q = query.toLowerCase().trim();

  // System status
  if (/(how|status|health|stat|cpu|memory|ram).*(system|doing|is it|are you)|^(stats?|status)$|how (is|are)/.test(q)
      || q.includes('how is the system') || q === 'stats') {
    try {
      const s = await services.invoke('get_system_stats');
      const memPct = ((s.memory.used_bytes / s.memory.total_bytes) * 100).toFixed(0);
      return `System looks healthy. CPU is at ${s.cpu.usage_percent.toFixed(0)}% across `
        + `${s.cpu.cores} cores, memory ${memPct}% used, running ${s.kernel}.`;
    } catch {
      return 'I could not reach the AI Core for stats right now.';
    }
  }

  // Layout
  for (const layout of ['tile', 'monocle', 'grid', 'spiral', 'float']) {
    if (q.includes(layout)) {
      wm.setLayout(layout);
      wm.notify('Layout → ' + layout, 'ok');
      return `Done. Switched to the ${layout} layout.`;
    }
  }
  if (q.includes('arrange') || q.includes('organi') || q.includes('clean up')) {
    wm.setLayout('tile');
    return 'Arranged everything into a tidy tiling layout.';
  }

  // Workspace
  const ws = q.match(/workspace\s*(\d+)|desktop\s*(\d+)|go to\s*(\d+)/);
  if (ws) {
    const n = parseInt(ws[1] || ws[2] || ws[3], 10);
    if (n >= 1 && n <= wm.workspaces.length) {
      wm.switchWorkspace(n - 1);
      return `Switched to workspace ${n} (${wm.workspaces[n - 1].name}).`;
    }
    return `There are ${wm.workspaces.length} workspaces; pick one from 1 to ${wm.workspaces.length}.`;
  }

  // Launch an app
  if (q.includes('open') || q.includes('launch') || q.includes('start') || q.includes('run')) {
    for (const [id, words] of Object.entries(APP_ALIASES)) {
      if (words.some((word) => q.includes(word))) {
        wm.spawn(id);
        wm.notify('Launched ' + id, 'ok');
        return `Opened ${id} for you.`;
      }
    }
    return 'I can open: terminal, files, monitor, task manager, config, or about. Which one?';
  }

  // Close
  if (q.includes('close') || q.includes('quit')) {
    if (wm.focusedId) {
      wm.closeFocused();
      return 'Closed the focused window.';
    }
    return 'There is no focused window to close.';
  }

  // Gaps
  const gaps = q.match(/gap[s]?\s*(?:to|of)?\s*(\d+)/);
  if (gaps) {
    wm.config.gaps = parseInt(gaps[1], 10);
    wm.layout();
    return `Set window gaps to ${gaps[1]}px.`;
  }

  if (q.includes('help') || q.includes('what can you') || q === '?') {
    return 'Without a language model I understand simple commands like: "tile the windows", '
      + '"use spiral layout", "open the terminal", "go to workspace 3", "how is the system?", '
      + '"set gaps to 20", "close this".';
  }
  if (q.includes('thank')) return 'Anytime. ✦';
  if (/^(hi|hello|hey|yo)\b/.test(q)) return 'Hello! What would you like me to do?';

  // Knowledge questions from the built-in corpus
  if (services.rag) {
    const res = services.rag.answer(query);
    if (res.sources.length > 0) {
      const cite = res.sources.map((s) => s.title).filter((v, i, a) => a.indexOf(v) === i);
      return `${res.answer}\n\n— retrieved from: ${cite.join(', ')}`;
    }
  }

  return "I'm not sure how to do that without a language model. Try 'help', or start the "
    + 'assistant service (Settings → Assistant) for open-ended requests.';
}
