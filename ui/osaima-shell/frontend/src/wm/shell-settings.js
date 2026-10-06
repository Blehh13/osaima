/**
 * shell-settings.js — the shell's tunable values, set from the Lua config:
 *
 *   wm.shell{ stats_poll_ms = 2000, star_count = 150, bookmarks = { ... } }
 *
 * Every value has a default and a validated range. A bad value never breaks the
 * shell: it falls back to the default and is reported in `warnings`.
 */

/** name in Lua -> { key in JS, default, min, max } for whole-number settings */
const NUMBERS = {
  clock_ms: { key: 'clockMs', def: 1000, min: 250, max: 60_000 },
  stats_poll_ms: { key: 'statsPollMs', def: 3000, min: 500, max: 60_000 },
  monitor_poll_ms: { key: 'monitorPollMs', def: 1500, min: 250, max: 60_000 },
  task_poll_ms: { key: 'taskPollMs', def: 2000, min: 500, max: 60_000 },
  proactive_interval_ms: { key: 'proactiveIntervalMs', def: 8000, min: 1000, max: 600_000 },
  proactive_first_ms: { key: 'proactiveFirstMs', def: 4000, min: 0, max: 600_000 },
  proactive_quiet_ms: { key: 'proactiveQuietMs', def: 12_000, min: 1000, max: 3_600_000 },
  cpu_alert_percent: { key: 'cpuAlertPercent', def: 70, min: 1, max: 100 },
  memory_alert_percent: { key: 'memoryAlertPercent', def: 85, min: 1, max: 100 },
  grid_suggest_windows: { key: 'gridSuggestWindows', def: 4, min: 2, max: 50 },
  star_count: { key: 'starCount', def: 320, min: 0, max: 2000 },
  notify_ms: { key: 'notifyMs', def: 3200, min: 500, max: 60_000 },
  notify_action_ms: { key: 'notifyActionMs', def: 7000, min: 500, max: 120_000 },
};

/** name in Lua -> { key in JS, default } for on/off settings */
const BOOLEANS = {
  proactive_enabled: { key: 'proactiveEnabled', def: true },
};

export const DEFAULT_BOOKMARKS = [
  { name: 'Wikipedia', url: 'https://en.wikipedia.org/wiki/Linux' },
  { name: 'Gentoo', url: 'https://www.gentoo.org' },
  { name: 'OSAIMA · GitHub', url: 'https://github.com/Blehh13/osaima' },
  { name: 'MDN Web Docs', url: 'https://developer.mozilla.org' },
  { name: 'Hacker News', url: 'https://news.ycombinator.com' },
  { name: 'archive.org', url: 'https://archive.org' },
];

export const DEFAULTS = Object.freeze({
  ...Object.fromEntries(Object.values(NUMBERS).map((n) => [n.key, n.def])),
  ...Object.fromEntries(Object.values(BOOLEANS).map((b) => [b.key, b.def])),
  searchUrl: 'https://en.wikipedia.org/w/index.php?search=%s',
  bookmarks: DEFAULT_BOOKMARKS,
  dockApps: [
    'assistant', 'browser', 'terminal', 'files', 'monitor',
    'taskmanager', 'knowledge', 'behavior', 'config', 'about',
  ],
  assistantSuggestions: [
    'What is using my memory?',
    'How much disk space do I have?',
    'Open the terminal',
    'Am I connected to the internet?',
    'Tile the windows',
  ],
  launcherSuggestions: [
    'Show system stats',
    'Open terminal',
    'Check AI core status',
    'List running processes',
    'Adjust display brightness',
    'Network diagnostics',
  ],
});

/** Every name accepted by `wm.shell{...}`, so docs and tests can check they stay complete. */
export const SETTING_NAMES = [
  ...Object.keys(NUMBERS),
  ...Object.keys(BOOLEANS),
  'search_url', 'bookmarks', 'dock_apps', 'assistant_suggestions', 'launcher_suggestions',
];

const isHttpUrl = (value) => {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' || url.protocol === 'http:';
  } catch {
    return false;
  }
};

/** A Lua table with no entries arrives as `{}`; treat it as an empty list. */
function asList(value) {
  if (Array.isArray(value)) return value;
  if (value && typeof value === 'object' && Object.keys(value).length === 0) return [];
  return null;
}

/**
 * @param {object|undefined} raw  the table passed to `wm.shell{...}`
 * @returns {{ settings: typeof DEFAULTS, warnings: string[] }}
 */
export function resolveShellSettings(raw) {
  const settings = { ...DEFAULTS };
  const warnings = [];
  if (raw === undefined || raw === null) return { settings, warnings };
  if (typeof raw !== 'object' || Array.isArray(raw)) {
    return { settings, warnings: ['wm.shell expects a table, e.g. wm.shell{ star_count = 100 }'] };
  }
  const known = new Set([
    ...Object.keys(NUMBERS), ...Object.keys(BOOLEANS), 'search_url', 'bookmarks', 'dock_apps',
    'assistant_suggestions', 'launcher_suggestions',
  ]);
  for (const name of Object.keys(raw)) {
    if (!known.has(name)) warnings.push(`unknown setting "${name}" ignored`);
  }

  for (const [name, spec] of Object.entries(NUMBERS)) {
    if (!(name in raw)) continue;
    const value = raw[name];
    if (Number.isInteger(value) && value >= spec.min && value <= spec.max) {
      settings[spec.key] = value;
    } else {
      warnings.push(`${name} must be a whole number from ${spec.min} to ${spec.max}; using ${spec.def}`);
    }
  }

  for (const [name, spec] of Object.entries(BOOLEANS)) {
    if (!(name in raw)) continue;
    if (typeof raw[name] === 'boolean') settings[spec.key] = raw[name];
    else warnings.push(`${name} must be true or false; using ${spec.def}`);
  }

  if ('search_url' in raw) {
    if (typeof raw.search_url === 'string' && raw.search_url.includes('%s') && isHttpUrl(raw.search_url)) {
      settings.searchUrl = raw.search_url;
    } else {
      warnings.push('search_url must be an http(s) address containing %s for the search words');
    }
  }

  if ('bookmarks' in raw) {
    const list = asList(raw.bookmarks);
    if (list === null) {
      warnings.push('bookmarks must be a list of { name = "...", url = "..." }');
    } else {
      settings.bookmarks = list.filter((b, i) => {
        const ok = b && typeof b.name === 'string' && b.name.trim() && isHttpUrl(b.url);
        if (!ok) warnings.push(`bookmark ${i + 1} needs a name and an http(s) url; skipped`);
        return ok;
      }).map((b) => ({ name: b.name.trim(), url: b.url }));
    }
  }

  for (const [name, key, max] of [
    ['dock_apps', 'dockApps', 40],
    ['assistant_suggestions', 'assistantSuggestions', 12],
    ['launcher_suggestions', 'launcherSuggestions', 12],
  ]) {
    if (!(name in raw)) continue;
    const list = asList(raw[name]);
    if (list === null || !list.every((v) => typeof v === 'string' && v.trim() && v.length <= 80)) {
      warnings.push(`${name} must be a list of short, non-empty strings`);
    } else {
      settings[key] = list.slice(0, max).map((v) => v.trim());
      if (list.length > max) warnings.push(`${name} keeps only the first ${max} entries`);
    }
  }
  return { settings, warnings };
}
