import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createVoice } from '../src/wm/voice-ui.js';

/** A scriptable stand-in for the voice service. */
function fakeService(overrides = {}) {
  const service = {
    calls: [],
    state: 'idle',
    last: null,
    status: { can_listen: true, can_speak: true, problems: [], speak_replies: false },
    ...overrides,
  };
  service.invoke = async (cmd, args) => {
    service.calls.push([cmd, args]);
    if (service.fail?.[cmd]) throw new Error(service.fail[cmd]);
    if (cmd === 'voice_status') return { ...service.status, state: service.state, last: service.last };
    return {};
  };
  service.names = () => service.calls.map(([c]) => c);
  return service;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function setup(service, extra = {}) {
  const heard = [];
  const notices = [];
  let changes = 0;
  const voice = createVoice({
    invoke: service.invoke,
    onText: (t) => heard.push(t),
    onNotice: (m) => notices.push(m),
    onChange: () => { changes += 1; },
    pollMs: 5,
    ...extra,
  });
  return { voice, heard, notices, changes: () => changes };
}

test('refresh reads what works, and the defaults from the service', async () => {
  const service = fakeService({ status: { can_listen: true, can_speak: true, problems: [], speak_replies: true } });
  const { voice } = setup(service);
  await voice.refresh();
  assert.equal(voice.view.canListen, true);
  assert.equal(voice.view.canSpeak, true);
  assert.equal(voice.view.speakReplies, true);
});

test('a service that is not running just means voice is off', async () => {
  const service = fakeService({ fail: { voice_status: 'The voice service is not running' } });
  const { voice } = setup(service);
  await voice.refresh();
  assert.equal(voice.view.canListen, false);
  assert.equal(voice.view.canSpeak, false);
  assert.match(voice.view.why, /not running/);
  await voice.toggle(); // does nothing
  assert.deepEqual(service.names(), ['voice_status']);
});

test('the first problem is kept for the tooltip, and nothing is spoken without speaking being possible', async () => {
  const service = fakeService({ status: { can_listen: true, can_speak: false, problems: ['piper is not installed'], speak_replies: true } });
  const { voice } = setup(service);
  await voice.refresh();
  assert.equal(voice.view.why, 'piper is not installed');
  assert.equal(voice.view.speakReplies, false);
  voice.speak('hello');
  assert.ok(!service.names().includes('voice_speak'));
});

test('listen, stop, then the transcript arrives through status', async () => {
  const service = fakeService();
  const { voice, heard } = setup(service);
  await voice.refresh();

  await voice.toggle();
  assert.equal(voice.view.state, 'listening');
  assert.deepEqual(service.names().slice(-1), ['voice_listen']);

  service.state = 'listening';
  await voice.toggle();
  assert.equal(voice.view.state, 'transcribing');
  assert.deepEqual(service.names().filter((n) => n === 'voice_stop'), ['voice_stop']);

  service.state = 'transcribing';
  await wait(30);
  service.last = { seq: 1, text: 'open the terminal' };
  service.state = 'idle';
  await wait(60);
  assert.deepEqual(heard, ['open the terminal']);
  assert.equal(voice.view.state, 'idle');

  const polls = service.names().filter((n) => n === 'voice_status').length;
  await wait(40);
  assert.equal(service.names().filter((n) => n === 'voice_status').length, polls, 'polling stops when idle');
});

test('the same transcript is never delivered twice, and old ones are ignored', async () => {
  const service = fakeService({ last: { seq: 4, text: 'from an earlier window' } });
  const { voice, heard } = setup(service);
  await voice.refresh(); // remembers seq 4
  await voice.toggle();
  service.state = 'idle';
  await wait(40);
  assert.deepEqual(heard, []);
  service.last = { seq: 5, text: 'new' };
  await voice.toggle();
  await wait(40);
  assert.deepEqual(heard, ['new']);
  await voice.toggle();
  await wait(40);
  assert.deepEqual(heard, ['new']);
});

test('a failure is shown as a notice, not typed into the chat', async () => {
  const service = fakeService();
  const { voice, heard, notices } = setup(service);
  await voice.refresh();
  await voice.toggle();
  service.last = { seq: 1, error: "I didn't hear anything." };
  service.state = 'idle';
  await wait(40);
  assert.deepEqual(heard, []);
  assert.deepEqual(notices, ["I didn't hear anything."]);
});

test('a failed start is reported and leaves nothing recording', async () => {
  const service = fakeService({ fail: { voice_listen: 'no microphone tool found' } });
  const { voice, notices } = setup(service);
  await voice.refresh();
  await voice.toggle();
  assert.deepEqual(notices, ['no microphone tool found']);
  assert.equal(voice.view.state, 'idle');
});

test('replies are read aloud only when that is on', async () => {
  const service = fakeService();
  const { voice } = setup(service);
  await voice.refresh();
  voice.speak('Your memory is fine.');
  assert.ok(!service.names().includes('voice_speak'));

  voice.setSpeakReplies(true);
  service.state = 'speaking'; // what the real service reports once speech has started
  voice.speak('Your memory is fine.');
  await wait(20);
  assert.deepEqual(service.calls.filter(([c]) => c === 'voice_speak'), [['voice_speak', { text: 'Your memory is fine.' }]]);

  voice.setSpeakReplies(false);
  await wait(10);
  assert.ok(service.names().includes('voice_silence'));
  service.state = 'idle'; // the service stopped talking, so polling ends
  await wait(30);
});

test('closing the window cancels a recording and stops speech', async () => {
  const service = fakeService();
  const { voice } = setup(service);
  await voice.refresh();
  await voice.toggle();
  voice.dispose();
  assert.ok(service.names().includes('voice_cancel'));
});

test('polling stops by itself when the window is gone', async () => {
  const service = fakeService();
  let open = true;
  const { voice } = setup(service, { alive: () => open });
  await voice.refresh();
  await voice.toggle();
  service.state = 'listening';
  await wait(25);
  open = false;
  await wait(20);
  const polls = service.names().filter((n) => n === 'voice_status').length;
  await wait(40);
  assert.equal(service.names().filter((n) => n === 'voice_status').length, polls);
});
