import { test } from 'node:test';
import assert from 'node:assert/strict';
import { shellTools, LAYOUTS } from '../src/wm/shell-tools.js';
import { ruleBasedReply } from '../src/wm/assistant-rules.js';

function fakeEngine() {
  const engine = {
    appRegistry: new Map([
      ['terminal', { id: 'terminal', title: 'Terminal' }],
      ['files', { id: 'files', title: 'Files' }],
    ]),
    workspaces: [{ name: 'main' }, { name: 'web' }, { name: 'code' }],
    config: { gaps: 8 },
    focusedId: null,
    log: [],
    spawn(app) { this.log.push(['spawn', app]); return { id: 1 }; },
    setLayout(l) { this.log.push(['layout', l]); },
    switchWorkspace(i) { this.log.push(['workspace', i]); },
    notify(m, k) { this.log.push(['notify', m, k]); },
    layout() { this.log.push(['relayout']); },
    closeFocused() { this.log.push(['close']); },
  };
  return engine;
}

test('specs describe every tool with schemas and mark them reversible', () => {
  const { specs } = shellTools(fakeEngine());
  const all = specs();
  assert.deepEqual(all.map((s) => s.name), [
    'shell_open_app', 'shell_set_layout', 'shell_switch_workspace', 'shell_notify',
  ]);
  for (const spec of all) {
    assert.equal(spec.inputSchema.type, 'object');
    assert.equal(spec.annotations.destructiveHint, false);
  }
  assert.deepEqual(all[0].inputSchema.properties.app.enum, ['terminal', 'files']);
  assert.deepEqual(all[1].inputSchema.properties.layout.enum, LAYOUTS);
});

test('open app / layout / workspace / notify run on the engine', () => {
  const engine = fakeEngine();
  const { run } = shellTools(engine);
  assert.equal(run({ name: 'shell_open_app', arguments: { app: 'files' } }).ok, true);
  assert.equal(run({ name: 'shell_set_layout', arguments: { layout: 'spiral' } }).ok, true);
  const ws = run({ name: 'shell_switch_workspace', arguments: { number: 2 } });
  assert.match(ws.output, /workspace 2 \(web\)/);
  assert.equal(run({ name: 'shell_notify', arguments: { message: '  Build done  ' } }).ok, true);
  assert.deepEqual(engine.log, [
    ['spawn', 'files'], ['layout', 'spiral'], ['workspace', 1], ['notify', 'Build done', 'info'],
  ]);
});

test('model-supplied arguments are validated', () => {
  const engine = fakeEngine();
  const { run } = shellTools(engine);
  const bad = [
    { name: 'shell_open_app', arguments: { app: 'rm -rf' } },
    { name: 'shell_open_app', arguments: {} },
    { name: 'shell_set_layout', arguments: { layout: 'diagonal' } },
    { name: 'shell_switch_workspace', arguments: { number: 0 } },
    { name: 'shell_switch_workspace', arguments: { number: 4 } },
    { name: 'shell_switch_workspace', arguments: { number: '2' } },
    { name: 'shell_switch_workspace', arguments: { number: 1.5 } },
    { name: 'shell_notify', arguments: { message: '   ' } },
    { name: 'shell_notify', arguments: { message: 42 } },
    { name: 'shell_unknown', arguments: {} },
    { name: 'shell_notify' },
  ];
  for (const call of bad) {
    const out = run(call);
    assert.equal(out.ok, false, JSON.stringify(call));
    assert.ok(out.output.length > 0);
  }
  assert.deepEqual(engine.log, []);
});

test('long notifications are cut to a sane length', () => {
  const engine = fakeEngine();
  shellTools(engine).run({ name: 'shell_notify', arguments: { message: 'x'.repeat(500) } });
  assert.equal(engine.log[0][1].length, 200);
});

test('rule-based fallback still handles basic desktop commands', async () => {
  const engine = fakeEngine();
  const services = {
    invoke: async () => ({
      kernel: '6.12', cpu: { usage_percent: 12.4, cores: 8 },
      memory: { used_bytes: 4, total_bytes: 8 },
    }),
    rag: null,
  };
  const reply = (q) => ruleBasedReply(q, { wm: engine, services });
  assert.match(await reply('how is the system'), /CPU is at 12%.*memory 50%/);
  assert.match(await reply('use spiral layout'), /spiral/);
  assert.match(await reply('go to workspace 9'), /3 workspaces/);
  assert.match(await reply('go to workspace 2'), /Switched to workspace 2 \(web\)/);
  assert.match(await reply('open the terminal'), /Opened terminal/);
  assert.match(await reply('set gaps to 20'), /20px/);
  assert.match(await reply('close this'), /no focused window/);
  assert.match(await reply('quantum chromodynamics'), /without a language model/);

  const failing = { ...services, invoke: async () => { throw new Error('down'); } };
  assert.match(await ruleBasedReply('stats', { wm: engine, services: failing }), /could not reach the AI Core/);
});
