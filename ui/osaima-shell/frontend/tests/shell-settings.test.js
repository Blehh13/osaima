import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  DEFAULTS,
  SETTING_NAMES,
  resolveShellSettings,
} from '../src/wm/shell-settings.js';
import { LuaVM, luaToJS } from '../src/wm/lua-vm.js';
import { createWmApi } from '../src/wm/api.js';
import { DEFAULT_WM_LUA } from '../src/wm/default-config.js';
import { renderDefaultConfig } from '../scripts/sync-default-config.mjs';
import { AgentCore } from '../src/wm/agent-core.js';

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const WM_LUA = readFileSync(here('../src/config/wm.lua'), 'utf8').replace(/\r\n/g, '\n');

/** The parts of the window manager the Lua API touches. */
function fakeEngine() {
  return {
    config: {},
    bindings: [],
    rules: [],
    autostart: [],
    notifications: [],
    addBinding(binding, action) { this.bindings.push([binding, action]); },
    notify(message, kind) { this.notifications.push([message, kind]); },
  };
}

function runLua(source) {
  const engine = fakeEngine();
  const vm = new LuaVM();
  vm.setGlobal('wm', createWmApi(engine));
  vm.run(source, 'test.lua');
  return engine;
}

test('with nothing configured the defaults apply', () => {
  const { settings, warnings } = resolveShellSettings(undefined);
  assert.deepEqual(settings, DEFAULTS);
  assert.deepEqual(warnings, []);
});

test('every kind of setting can be overridden', () => {
  const { settings, warnings } = resolveShellSettings({
    stats_poll_ms: 2000,
    star_count: 0,
    proactive_enabled: false,
    cpu_alert_percent: 90,
    search_url: 'https://duckduckgo.com/?q=%s',
    bookmarks: [{ name: ' Docs ', url: 'https://docs.example/' }],
    dock_apps: ['terminal', 'files'],
    assistant_suggestions: ['Hello?'],
    launcher_suggestions: [],
  });
  assert.deepEqual(warnings, []);
  assert.equal(settings.statsPollMs, 2000);
  assert.equal(settings.starCount, 0);
  assert.equal(settings.proactiveEnabled, false);
  assert.equal(settings.cpuAlertPercent, 90);
  assert.equal(settings.searchUrl, 'https://duckduckgo.com/?q=%s');
  assert.deepEqual(settings.bookmarks, [{ name: 'Docs', url: 'https://docs.example/' }]);
  assert.deepEqual(settings.dockApps, ['terminal', 'files']);
  assert.deepEqual(settings.assistantSuggestions, ['Hello?']);
  assert.deepEqual(settings.launcherSuggestions, [], 'an empty list means none');
  assert.equal(settings.clockMs, DEFAULTS.clockMs, 'untouched settings keep their defaults');
});

test('bad values fall back to the default and say why', () => {
  const { settings, warnings } = resolveShellSettings({
    stats_poll_ms: 5, // below the minimum
    clock_ms: 'fast',
    star_count: 10.5,
    proactive_enabled: 'yes',
    search_url: 'https://example.com/no-placeholder',
    bookmarks: 'nope',
    dock_apps: [1, 2],
    colour: 'blue',
  });
  assert.equal(settings.statsPollMs, DEFAULTS.statsPollMs);
  assert.equal(settings.clockMs, DEFAULTS.clockMs);
  assert.equal(settings.starCount, DEFAULTS.starCount);
  assert.equal(settings.proactiveEnabled, true);
  assert.equal(settings.searchUrl, DEFAULTS.searchUrl);
  assert.deepEqual(settings.bookmarks, DEFAULTS.bookmarks);
  assert.deepEqual(settings.dockApps, DEFAULTS.dockApps);
  const text = warnings.join('\n');
  for (const expected of ['stats_poll_ms', 'clock_ms', 'star_count', 'proactive_enabled',
    'search_url', 'bookmarks', 'dock_apps', 'unknown setting "colour"']) {
    assert.match(text, new RegExp(expected), `a warning mentions ${expected}`);
  }
});

test('unsafe or malformed bookmarks are skipped individually', () => {
  const { settings, warnings } = resolveShellSettings({
    bookmarks: [
      { name: 'Good', url: 'https://good.example' },
      { name: 'Script', url: 'javascript:alert(1)' },
      { name: '', url: 'https://noname.example' },
      { url: 'https://nokey.example' },
      'text',
    ],
  });
  assert.deepEqual(settings.bookmarks, [{ name: 'Good', url: 'https://good.example' }]);
  assert.equal(warnings.length, 4);
});

test('list sizes are capped and non-tables are refused', () => {
  const many = Array.from({ length: 30 }, (_, i) => `suggestion ${i}`);
  const { settings, warnings } = resolveShellSettings({ assistant_suggestions: many });
  assert.equal(settings.assistantSuggestions.length, 12);
  assert.match(warnings.join(), /first 12/);
  assert.match(resolveShellSettings('text').warnings[0], /expects a table/);
  assert.match(resolveShellSettings([1]).warnings[0], /expects a table/);
});

test('wm.shell{} in Lua reaches the settings, and repeated calls merge', () => {
  const engine = runLua(`
    wm.shell({ stats_poll_ms = 2500, bookmarks = { { name = "A", url = "https://a.example" } } })
    wm.shell({ star_count = 40 })
  `);
  const { settings, warnings } = resolveShellSettings(engine.config.shell);
  assert.deepEqual(warnings, []);
  assert.equal(settings.statsPollMs, 2500);
  assert.equal(settings.starCount, 40);
  assert.deepEqual(settings.bookmarks, [{ name: 'A', url: 'https://a.example' }]);
});

test('an empty Lua table is an empty list', () => {
  const engine = runLua('wm.shell({ bookmarks = {} })');
  assert.deepEqual(resolveShellSettings(engine.config.shell).settings.bookmarks, []);
});

test('the shipped wm.lua runs and produces no warnings', () => {
  const engine = runLua(WM_LUA);
  const { warnings } = resolveShellSettings(engine.config.shell);
  assert.deepEqual(warnings, []);
  assert.ok(engine.bindings.length > 10, 'the bindings were registered');
});

test('uncommenting every documented setting in wm.lua is valid', () => {
  const start = WM_LUA.indexOf('wm.shell({');
  const end = WM_LUA.indexOf('\n})', start);
  // Documented settings are single lines of the form `-- name = value,`.
  const lines = WM_LUA.slice(start, end)
    .split('\n')
    .map((line) => line.match(/^\s*-- (\w+ = .+,.*)$/))
    .filter(Boolean)
    .map((match) => match[1]);
  assert.equal(lines.length, SETTING_NAMES.length, 'one documented example per setting');

  const engine = runLua(`wm.shell({\n${lines.join('\n')}\n})`);
  const { settings, warnings } = resolveShellSettings(engine.config.shell);
  assert.deepEqual(warnings, []);
  // The documented examples are the defaults, except the shortened example lists.
  assert.equal(settings.statsPollMs, DEFAULTS.statsPollMs);
  assert.equal(settings.proactiveEnabled, DEFAULTS.proactiveEnabled);
  assert.equal(settings.searchUrl, DEFAULTS.searchUrl);
  assert.deepEqual(settings.dockApps, DEFAULTS.dockApps);
});

test('every setting is documented in wm.lua', () => {
  for (const name of SETTING_NAMES) {
    assert.ok(WM_LUA.includes(name), `${name} is missing from the wm.shell block in wm.lua`);
  }
});

test('the embedded fallback config matches wm.lua exactly', () => {
  assert.equal(DEFAULT_WM_LUA, WM_LUA);
  const generated = readFileSync(here('../src/wm/default-config.js'), 'utf8').replace(/\r\n/g, '\n');
  assert.equal(generated, renderDefaultConfig(WM_LUA), 'run `npm run sync-config`');
});

test('the config round-trips through the generator for tricky characters', async () => {
  const tricky = 'a `tick` and ${placeholder} and a \\ backslash\n';
  const module = await import(`data:text/javascript,${encodeURIComponent(renderDefaultConfig(tricky))}`);
  assert.equal(module.DEFAULT_WM_LUA, tricky);
});

test('the proactive assistant follows its settings', async () => {
  const suggestions = [];
  const core = new AgentCore({
    engine: {}, invoke: async () => null, suggest: (m) => suggestions.push(m),
  });
  core.configure({ ...DEFAULTS, proactiveEnabled: false });
  assert.equal(core.timer, null, 'no timer when switched off');
  await core.tick();
  assert.deepEqual(suggestions, [], 'a disabled assistant stays quiet');

  core.configure({ ...DEFAULTS, proactiveIntervalMs: 60_000 });
  assert.notEqual(core.timer, null);
  core.configure({ ...DEFAULTS, proactiveEnabled: false });
  assert.equal(core.timer, null, 'turning it off again clears the timer');
  core.stop();
});

test('luaToJS keeps nested lists inside tables', () => {
  const engine = runLua('wm.shell({ bookmarks = { { name = "x", url = "https://x.example" } } })');
  assert.ok(Array.isArray(luaToJS(engine.config.shell).bookmarks));
});
