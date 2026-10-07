import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AgentClient, AgentUnavailableError, tauriTransport } from '../src/wm/agent-client.js';
import { demoTransport } from '../src/wm/agent-demo.js';

/** A scriptable stand-in for the agent service. */
class FakeAgent {
  constructor() {
    this.listeners = new Set();
    this.calls = [];
    this.handlers = {};
  }
  emit = (event) => this.listeners.forEach((fn) => fn(event));
  subscribe(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
  async call(method, params) {
    this.calls.push([method, params]);
    const handler = this.handlers[method];
    if (!handler) throw new Error(`unexpected call ${method}`);
    return handler(params, this.emit);
  }
  callsTo(method) {
    return this.calls.filter(([m]) => m === method).map(([, p]) => p);
  }
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

function setup(chatHandler) {
  const agent = new FakeAgent();
  agent.handlers['agent.chat'] = chatHandler;
  agent.handlers['agent.approve'] = () => ({});
  agent.handlers['agent.client_tool_result'] = () => ({});
  agent.handlers['agent.cancel'] = () => ({ cancelled: true });
  return { agent, client: new AgentClient(agent) };
}

test('streams text and resolves on done, even when events beat the reply', async () => {
  const { client } = setup((params, emit) => {
    // Events arrive before the chat reply is processed.
    emit({ type: 'text', turn_id: 't1', delta: 'Hello ' });
    emit({ type: 'text', turn_id: 't1', delta: 'world' });
    emit({ type: 'done', turn_id: 't1', text: 'Hello world', provider: 'ollama', model: 'qwen', steps: 1 });
    return { turn_id: 't1', conversation_id: 'c1' };
  });
  const seen = [];
  const turn = await client.start('hi', { onText: (d) => seen.push(d) });
  const result = await turn.done;
  assert.deepEqual(seen, ['Hello ', 'world']);
  assert.equal(result.status, 'done');
  assert.equal(result.text, 'Hello world');
  assert.equal(result.provider, 'ollama');
  assert.equal(result.conversationId, 'c1');
});

test('sends options and conversation ids to the service', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't', conversation_id: 'c' }));
  await client.start('hello', {
    conversationId: 'c9',
    model: 'local',
    clientTools: [{ name: 'shell_notify' }],
  });
  assert.deepEqual(agent.callsTo('agent.chat')[0], {
    message: 'hello',
    conversation_id: 'c9',
    model: 'local',
    client_tools: [{ name: 'shell_notify' }],
  });
});

test('tool cards: calls and results reach their callbacks', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c1' }));
  const log = [];
  const turn = await client.start('stats', {
    onToolCall: (e) => log.push(['call', e.name]),
    onToolResult: (e) => log.push(['result', e.name, e.ok]),
    onNotice: (m) => log.push(['notice', m]),
  });
  agent.emit({ type: 'tool_call', turn_id: 't1', call_id: 'a', name: 'get_system_stats' });
  agent.emit({ type: 'notice', turn_id: 't1', message: 'switching to claude' });
  agent.emit({ type: 'tool_result', turn_id: 't1', call_id: 'a', name: 'get_system_stats', ok: true });
  agent.emit({ type: 'done', turn_id: 't1', text: '' });
  await turn.done;
  assert.deepEqual(log, [
    ['call', 'get_system_stats'],
    ['notice', 'switching to claude'],
    ['result', 'get_system_stats', true],
  ]);
});

test('approvals are answered through the service; failures never approve', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c1' }));
  let answerNext = true;
  const turn = await client.start('kill 4242', {
    onApproval: async () => {
      if (answerNext === 'throw') throw new Error('ui crashed');
      return answerNext;
    },
  });
  agent.emit({ type: 'approval_required', turn_id: 't1', call_id: 'k1', name: 'kill_process' });
  await tick();
  answerNext = 'throw';
  agent.emit({ type: 'approval_required', turn_id: 't1', call_id: 'k2', name: 'kill_process' });
  await tick();
  assert.deepEqual(agent.callsTo('agent.approve'), [
    { turn_id: 't1', call_id: 'k1', approved: true },
    { turn_id: 't1', call_id: 'k2', approved: false },
  ]);
  agent.emit({ type: 'done', turn_id: 't1', text: '' });
  await turn.done;
});

test('client tools run in the shell and report back', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c1' }));
  const turn = await client.start('open files', {
    runClientTool: async (event) => ({ ok: true, output: `opened ${event.arguments.app}` }),
  });
  agent.emit({ type: 'client_tool_call', turn_id: 't1', call_id: 'x', name: 'shell_open_app', arguments: { app: 'files' } });
  await tick();
  assert.deepEqual(agent.callsTo('agent.client_tool_result')[0], {
    turn_id: 't1', call_id: 'x', ok: true, output: 'opened files',
  });

  // No runner, or a runner that throws, still answers (the agent is waiting).
  agent.emit({ type: 'client_tool_call', turn_id: 't1', call_id: 'y', name: 'shell_open_app', arguments: {} });
  await tick();
  agent.emit({ type: 'done', turn_id: 't1', text: '' });
  await turn.done;

  const second = setup(() => ({ turn_id: 't2', conversation_id: 'c' }));
  const t2 = await second.client.start('x', { runClientTool: () => { throw new Error('boom'); } });
  second.agent.emit({ type: 'client_tool_call', turn_id: 't2', call_id: 'z', name: 'n', arguments: {} });
  await tick();
  const sent = second.agent.callsTo('agent.client_tool_result')[0];
  assert.equal(sent.ok, false);
  assert.match(sent.output, /boom/);
  second.agent.emit({ type: 'done', turn_id: 't2', text: '' });
  await t2.done;
});

test('errors and cancellation are results, not exceptions', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c1' }));
  const turn = await client.start('x');
  agent.emit({ type: 'error', turn_id: 't1', message: 'Ollama is not running' });
  assert.deepEqual(
    { status: (await turn.done).status, message: (await turn.done).message },
    { status: 'error', message: 'Ollama is not running' },
  );

  const second = setup(() => ({ turn_id: 't2', conversation_id: 'c' }));
  const t2 = await second.client.start('x');
  await t2.cancel();
  assert.deepEqual(second.agent.callsTo('agent.cancel'), [{ turn_id: 't2' }]);
  second.agent.emit({ type: 'cancelled', turn_id: 't2' });
  assert.equal((await t2.done).status, 'cancelled');
});

test('a dropped service fails running turns with AgentUnavailableError', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c1' }));
  const turn = await client.start('x');
  agent.emit({ type: 'disconnected' });
  await assert.rejects(turn.done, AgentUnavailableError);
});

test('service-down errors are distinguished from other failures', async () => {
  const { agent, client } = setup(() => {
    throw new Error('The assistant service is not running (/run/osaima/agent.sock)');
  });
  await assert.rejects(client.start('x'), AgentUnavailableError);
  agent.handlers['agent.chat'] = () => { throw new Error('this conversation is still answering'); };
  await assert.rejects(client.start('x'), (e) => !(e instanceof AgentUnavailableError) && /still answering/.test(e.message));
  // Tauri rejects with plain strings.
  agent.handlers['agent.chat'] = () => Promise.reject('the assistant service disconnected');
  await assert.rejects(client.start('x'), AgentUnavailableError);
});

test('events for unknown turns are buffered with a bound', () => {
  const { client } = setup(() => ({}));
  for (let i = 0; i < 40; i++) client.dispatch({ type: 'text', turn_id: `stray${i}`, delta: 'x' });
  assert.ok(client.buffered.size <= 8);
  for (let i = 0; i < 500; i++) client.dispatch({ type: 'text', turn_id: 'busy', delta: 'x' });
  assert.ok(client.buffered.get('busy').length <= 200);
  client.dispatch({ type: 'text', delta: 'no turn id' }); // ignored
});

test('tauriTransport invokes agent_call and forwards events', async () => {
  const invoked = [];
  let handler;
  let unlistened = 0;
  const tauri = {
    core: { invoke: async (cmd, args) => { invoked.push([cmd, args]); return { ok: 1 }; } },
    event: {
      listen: async (name, fn) => {
        assert.equal(name, 'agent-event');
        handler = fn;
        return () => { unlistened++; };
      },
    },
  };
  const transport = tauriTransport(tauri);
  const received = [];
  const stop = transport.subscribe((e) => received.push(e));
  await tick();
  handler({ payload: { type: 'text', delta: 'a' } });
  assert.deepEqual(received, [{ type: 'text', delta: 'a' }]);
  assert.deepEqual(await transport.call('agent.status', {}), { ok: 1 });
  assert.deepEqual(invoked, [['agent_call', { method: 'agent.status', params: {} }]]);
  stop();
  assert.equal(unlistened, 1);

  // Unsubscribing before the listener is registered still cleans up.
  const early = tauriTransport(tauri).subscribe(() => {});
  early();
  await tick();
  assert.equal(unlistened, 2);
});

// ── the scripted demo agent speaks the same protocol ────────────────────────

async function demoRun(message, handlers) {
  const client = new AgentClient(demoTransport({ speed: 0 }));
  const turn = await client.start(message, handlers);
  return turn.done;
}

test('demo: a question uses a tool and answers', async () => {
  const calls = [];
  const result = await demoRun('what is using my memory?', { onToolCall: (e) => calls.push(e.name) });
  assert.deepEqual(calls, ['list_processes']);
  assert.equal(result.status, 'done');
  assert.match(result.text, /Firefox is using the most memory/);
  assert.equal(result.provider, 'demo');
});

test('demo: destructive actions wait for approval, and denial is respected', async () => {
  let asked = 0;
  const outputs = [];
  const result = await demoRun('close firefox', {
    onApproval: async (e) => { asked++; assert.equal(e.destructive, true); return false; },
    onToolResult: (e) => outputs.push([e.ok, e.output]),
  });
  assert.equal(result.status, 'done');
  assert.equal(asked, 1);
  assert.deepEqual(outputs, [[false, 'The user declined this action.']]);
});

test('demo: client tools round-trip through the shell', async () => {
  const opened = [];
  const result = await demoRun('open the files app', {
    runClientTool: async (e) => { opened.push(e.arguments.app); return { ok: true, output: 'opened Files' }; },
  });
  assert.equal(result.status, 'done');
  assert.deepEqual(opened, ['files']);
});

test('voice calls map to the service methods', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't', conversation_id: 'c' }));
  for (const method of ['status', 'listen', 'stop', 'cancel', 'speak', 'silence']) {
    agent.handlers[`agent.voice.${method}`] = () => (method === 'status' ? { can_listen: true } : {});
  }
  assert.deepEqual(await client.voiceStatus(), { can_listen: true });
  await client.voiceListen();
  await client.voiceStop();
  await client.voiceCancel();
  await client.speak('Hello there');
  await client.silence();
  assert.deepEqual(agent.calls.map(([m]) => m), [
    'agent.voice.status', 'agent.voice.listen', 'agent.voice.stop',
    'agent.voice.cancel', 'agent.voice.speak', 'agent.voice.silence',
  ]);
  assert.deepEqual(agent.callsTo('agent.voice.speak'), [{ text: 'Hello there' }]);
});

test('voice events go to voice listeners, not to chat turns', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't1', conversation_id: 'c' }));
  const heard = [];
  const stop = client.onVoice((event) => heard.push(event));
  const turn = await client.start('hi', {});
  agent.emit({ type: 'voice_state', state: 'listening' });
  agent.emit({ type: 'voice_transcript', text: 'open the terminal' });
  agent.emit({ type: 'done', turn_id: 't1', text: 'ok' });
  assert.equal((await turn.done).status, 'done');
  assert.deepEqual(heard.map((e) => e.type), ['voice_state', 'voice_transcript']);
  stop();
  agent.emit({ type: 'voice_error', message: 'x' });
  assert.equal(heard.length, 2);
});

test('losing the service tells voice listeners it is idle', async () => {
  const { agent, client } = setup(() => ({ turn_id: 't', conversation_id: 'c' }));
  const states = [];
  client.onVoice((event) => states.push(event.state));
  agent.emit({ type: 'disconnected' });
  assert.deepEqual(states, ['idle']);
});

test('demo: voice reports that it needs the real OS', async () => {
  const client = new AgentClient(demoTransport());
  const status = await client.voiceStatus();
  assert.equal(status.can_listen, false);
  assert.equal(status.can_speak, false);
  assert.match(status.problems[0], /real Interstellar OS/);
  client.close();
});
