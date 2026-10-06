/**
 * agent-client.js — talks to the OSAIMA agent service.
 *
 * The transport (Tauri in the real OS, a scripted fake in browser demos and
 * tests) provides `call(method, params)` and `subscribe(listener)`. This class
 * turns the agent's event stream into per-request callbacks:
 *
 *   const turn = await client.start('close firefox', {
 *     onText, onToolCall, onToolResult, onApproval, onNotice,
 *     runClientTool,            // runs tools the shell implements itself
 *   });
 *   const result = await turn.done;   // { status, text, provider, model, ... }
 *
 * Replies and events travel on different paths, so an event can arrive before
 * the reply that names its turn; such events are buffered until the turn is known.
 */

const MAX_BUFFERED_TURNS = 8;
const MAX_BUFFERED_EVENTS = 200;

export class AgentUnavailableError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AgentUnavailableError';
  }
}

export class AgentClient {
  /** @param {{call: Function, subscribe: Function}} transport */
  constructor(transport) {
    this.transport = transport;
    this.turns = new Map();
    this.buffered = new Map();
    this.unsubscribe = transport.subscribe((event) => this.dispatch(event));
  }

  /** Configured models and whether they respond. Throws AgentUnavailableError if the service is down. */
  async status() {
    return this.#call('agent.status', {});
  }

  async reset(conversationId) {
    if (conversationId) await this.#call('agent.reset', { conversation_id: conversationId });
  }

  async audit(limit = 50) {
    return (await this.#call('agent.audit', { limit })).entries;
  }

  /**
   * Begin a request. Resolves once the service accepted it.
   * @param {string} message
   * @param {object} [options]
   * @param {string} [options.conversationId]
   * @param {'auto'|'local'|'cloud'} [options.model]
   * @param {object[]} [options.clientTools]   tools the shell implements (sent on a conversation's first message)
   * @param {Function} [options.runClientTool] (event) => Promise<{ok, output}>
   * @param {Function} [options.onText] (delta)
   * @param {Function} [options.onToolCall] (event)
   * @param {Function} [options.onToolResult] (event)
   * @param {Function} [options.onApproval] (event) => Promise<boolean>
   * @param {Function} [options.onNotice] (message)
   */
  async start(message, options = {}) {
    const params = { message };
    if (options.conversationId) params.conversation_id = options.conversationId;
    if (options.model) params.model = options.model;
    if (options.clientTools?.length) params.client_tools = options.clientTools;
    const accepted = await this.#call('agent.chat', params);

    const turn = {
      id: accepted.turn_id,
      conversationId: accepted.conversation_id,
      options,
      text: '',
    };
    turn.done = new Promise((resolve, reject) => {
      turn.resolve = resolve;
      turn.reject = reject;
    });
    turn.cancel = () => this.#call('agent.cancel', { turn_id: turn.id }).catch(() => {});
    this.turns.set(turn.id, turn);

    for (const event of this.buffered.get(turn.id) ?? []) this.dispatch(event);
    this.buffered.delete(turn.id);
    return turn;
  }

  /** Stop listening (tests, window close). */
  close() {
    this.unsubscribe?.();
    this.#failAll(new AgentUnavailableError('The assistant was closed.'));
  }

  dispatch(event) {
    if (event.type === 'disconnected') {
      this.#failAll(new AgentUnavailableError('The assistant service disconnected.'));
      return;
    }
    const turn = this.turns.get(event.turn_id);
    if (!turn) {
      this.#buffer(event);
      return;
    }
    const o = turn.options;
    switch (event.type) {
      case 'text':
        turn.text += event.delta;
        o.onText?.(event.delta);
        break;
      case 'tool_call':
        o.onToolCall?.(event);
        break;
      case 'tool_result':
        o.onToolResult?.(event);
        break;
      case 'notice':
        o.onNotice?.(event.message);
        break;
      case 'approval_required':
        this.#answerApproval(turn, event);
        break;
      case 'client_tool_call':
        this.#runClientTool(turn, event);
        break;
      case 'done':
        this.#finish(turn, {
          status: 'done',
          text: event.text ?? turn.text,
          provider: event.provider,
          model: event.model,
          steps: event.steps,
          detail: event.detail ?? '',
        });
        break;
      case 'error':
        this.#finish(turn, { status: 'error', text: turn.text, message: event.message });
        break;
      case 'cancelled':
        this.#finish(turn, { status: 'cancelled', text: turn.text });
        break;
      default:
        break;
    }
  }

  async #answerApproval(turn, event) {
    let approved = false;
    try {
      approved = Boolean(await turn.options.onApproval?.(event));
    } catch {
      approved = false; // an approval UI that fails never approves
    }
    await this.#call('agent.approve', {
      turn_id: turn.id,
      call_id: event.call_id,
      approved,
    }).catch(() => {});
  }

  async #runClientTool(turn, event) {
    let outcome;
    try {
      outcome = await turn.options.runClientTool?.(event);
    } catch (err) {
      outcome = { ok: false, output: `The shell could not run this: ${err.message ?? err}` };
    }
    outcome ??= { ok: false, output: 'The shell does not implement this tool.' };
    await this.#call('agent.client_tool_result', {
      turn_id: turn.id,
      call_id: event.call_id,
      ok: Boolean(outcome.ok),
      output: String(outcome.output ?? ''),
    }).catch(() => {});
  }

  #finish(turn, result) {
    this.turns.delete(turn.id);
    turn.resolve({ ...result, conversationId: turn.conversationId });
  }

  #failAll(error) {
    for (const turn of this.turns.values()) turn.reject(error);
    this.turns.clear();
    this.buffered.clear();
  }

  #buffer(event) {
    if (!event.turn_id) return;
    let events = this.buffered.get(event.turn_id);
    if (!events) {
      if (this.buffered.size >= MAX_BUFFERED_TURNS) {
        this.buffered.delete(this.buffered.keys().next().value);
      }
      events = [];
      this.buffered.set(event.turn_id, events);
    }
    if (events.length < MAX_BUFFERED_EVENTS) events.push(event);
  }

  async #call(method, params) {
    try {
      return await this.transport.call(method, params);
    } catch (err) {
      const message = typeof err === 'string' ? err : err?.message ?? String(err);
      // The Tauri bridge reports "not running" / "disconnected" / "did not respond".
      if (/not running|disconnected|did not respond|lost connection|unavailable/i.test(message)) {
        throw new AgentUnavailableError(message);
      }
      throw new Error(message);
    }
  }
}

/** Transport over Tauri: `agent_call` commands plus the `agent-event` event. */
export function tauriTransport(tauri) {
  return {
    call: (method, params) => tauri.core.invoke('agent_call', { method, params }),
    subscribe(listener) {
      let stop = null;
      let cancelled = false;
      tauri.event
        .listen('agent-event', (e) => listener(e.payload))
        .then((unlisten) => {
          if (cancelled) unlisten();
          else stop = unlisten;
        });
      return () => {
        cancelled = true;
        stop?.();
      };
    },
  };
}
