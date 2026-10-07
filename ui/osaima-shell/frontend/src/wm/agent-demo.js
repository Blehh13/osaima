/**
 * agent-demo.js — a scripted stand-in for the agent service, used when the
 * shell runs in a plain browser (no Tauri). It exercises the same protocol,
 * including streaming, tool cards and approvals, but nothing it does is real.
 */

/** @param {{ speed?: number }} [options] speed 0 makes every pause instant (tests). */
export function demoTransport({ speed = 1 } = {}) {
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms * speed));
  const listeners = new Set();
  const waiting = new Map(); // `${turn}:${call}` -> resolve
  let counter = 0;

  const emit = (event) => listeners.forEach((fn) => fn(event));
  const id = (prefix) => `${prefix}${++counter}`;
  const answer = (turn, call) =>
    new Promise((resolve) => waiting.set(`${turn}:${call}`, resolve));

  const spoken = new Map(); // turn -> everything said, for the final `done` event

  async function say(turn, text) {
    spoken.set(turn, (spoken.get(turn) ?? '') + text);
    for (const word of text.split(/(?<= )/)) {
      emit({ type: 'text', turn_id: turn, delta: word });
      await sleep(25);
    }
  }

  async function toolCall(turn, name, title, args, extra = {}) {
    const call = id('call_');
    emit({ type: 'tool_call', turn_id: turn, call_id: call, name, title, arguments: args, ...extra });
    return call;
  }

  async function script(turn, message) {
    const q = message.toLowerCase();
    await sleep(300);
    if (/memory|cpu|slow|busy|using/.test(q)) {
      const call = await toolCall(turn, 'list_processes', 'List processes', { limit: 3, sort_by: 'memory' });
      await sleep(400);
      emit({
        type: 'tool_result', turn_id: turn, call_id: call, name: 'list_processes', ok: true,
        output: 'firefox 1.1 GB\nollama 940 MB\nosaima-shell 210 MB',
        structured: null,
      });
      await say(turn, 'Firefox is using the most memory (1.1 GB), followed by Ollama (940 MB). Want me to close Firefox?');
    } else if (/close|kill|end|quit/.test(q)) {
      const call = await toolCall(turn, 'kill_process', 'End a process', { pid: 4242, signal: 'TERM' },
        { needs_approval: true, destructive: true });
      emit({
        type: 'approval_required', turn_id: turn, call_id: call, name: 'kill_process',
        title: 'End a process', arguments: { pid: 4242, signal: 'TERM' }, destructive: true,
      });
      const approved = await answer(turn, call);
      await sleep(300);
      emit({
        type: 'tool_result', turn_id: turn, call_id: call, name: 'kill_process', ok: approved,
        output: approved ? 'Process 4242 ended (demo).' : 'The user declined this action.',
        structured: null,
      });
      await say(turn, approved ? 'Done. Firefox has been closed (this is a demo, so nothing really happened).' : 'Okay, I left it running.');
    } else if (/open|launch|start/.test(q)) {
      const app = ['terminal', 'files', 'monitor', 'taskmanager', 'browser'].find((a) => q.includes(a)) ?? 'terminal';
      const call = await toolCall(turn, 'shell_open_app', 'Open a built-in app', { app });
      emit({ type: 'client_tool_call', turn_id: turn, call_id: call, name: 'shell_open_app', arguments: { app } });
      const outcome = await answer(turn, call);
      emit({
        type: 'tool_result', turn_id: turn, call_id: call, name: 'shell_open_app',
        ok: outcome.ok, output: outcome.output, structured: null,
      });
      await say(turn, outcome.ok ? `Opened ${app} for you.` : `I couldn't open ${app}: ${outcome.output}`);
    } else {
      await say(turn, "I'm the demo assistant, so I only know a few tricks. Try \"what is using my memory?\", \"close firefox\" or \"open the terminal\". On a real Interstellar OS install, a local model answers anything.");
    }
    emit({ type: 'done', turn_id: turn, text: spoken.get(turn) ?? '', provider: 'demo', model: 'scripted', steps: 1 });
    spoken.delete(turn);
  }

  return {
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    async call(method, params) {
      switch (method) {
        case 'agent.status':
          return {
            local: { name: 'demo', model: 'scripted demo', available: true },
            cloud: null,
            cloud_enabled: false,
          };
        case 'agent.voice.status':
          return {
            enabled: false, can_listen: false, can_speak: false,
            problems: ['Voice needs the real Interstellar OS (this is the demo).'],
          };
        case 'agent.chat': {
          const turn = id('turn_');
          const conversation = params.conversation_id ?? id('convo_');
          setTimeout(() => script(turn, params.message).catch(() => emit({ type: 'error', turn_id: turn, message: 'demo failed' })), 0);
          return { turn_id: turn, conversation_id: conversation };
        }
        case 'agent.approve':
          waiting.get(`${params.turn_id}:${params.call_id}`)?.(params.approved);
          return {};
        case 'agent.client_tool_result':
          waiting.get(`${params.turn_id}:${params.call_id}`)?.({ ok: params.ok, output: params.output });
          return {};
        case 'agent.cancel':
          emit({ type: 'cancelled', turn_id: params.turn_id });
          return { cancelled: true };
        case 'agent.reset':
          return {};
        case 'agent.audit':
          return { entries: [] };
        default:
          throw new Error(`unknown method ${method}`);
      }
    },
  };
}
