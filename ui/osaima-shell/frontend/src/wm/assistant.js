/**
 * assistant.js — the AI Assistant app: a streaming chat with the agent service.
 *
 * Shows the model's reply as it is written, a card for every action it takes,
 * and Approve / Deny buttons for anything risky. When the agent service isn't
 * running it falls back to the basic keyword assistant so the shell still works.
 *
 * Everything that comes from the model or the system is rendered with
 * textContent, never as HTML.
 */

import { AgentUnavailableError } from './agent-client.js';
import { ruleBasedReply } from './assistant-rules.js';
import { shellTools } from './shell-tools.js';

const instances = new Set();

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** `{"app":"files"}` → `app: files`, shortened for a card header. */
function summarizeArguments(args) {
  const entries = Object.entries(args ?? {});
  if (entries.length === 0) return '';
  const text = entries.map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`).join(', ');
  return text.length > 90 ? `${text.slice(0, 87)}…` : text;
}

/** Ask the assistant a question from outside its window (e.g. the launcher). */
export function askAssistant(engine, text) {
  let instance = [...instances][0];
  if (instance) {
    if (instance.win.minimized) engine.restore(instance.win.id);
    engine.focus(instance.win.id);
  } else {
    engine.spawn('assistant');
    instance = [...instances][0];
  }
  instance?.ask(text);
}

export function makeAssistant(engine, services) {
  return {
    id: 'assistant',
    title: 'AI Assistant',
    icon: '✦',
    width: 520,
    height: 600,
    floating: true,
    mount(root, ctx) {
      root.classList.add('app-assistant');
      const client = services.agent;
      const tools = shellTools(engine);

      // ── layout ──
      const header = el('div', 'as-header');
      const statusDot = el('span', 'as-dot');
      const statusText = el('span', 'as-status', 'Checking…');
      const modelSelect = document.createElement('select');
      modelSelect.className = 'as-model';
      modelSelect.setAttribute('aria-label', 'Which model answers');
      for (const [value, label] of [['auto', 'Auto'], ['local', 'Local only'], ['cloud', 'Cloud only']]) {
        modelSelect.append(new Option(label, value));
      }
      const speaker = el('button', 'as-speak', '🔈');
      speaker.setAttribute('aria-label', 'Read replies aloud');
      speaker.setAttribute('aria-pressed', 'false');
      speaker.disabled = true;
      const newChat = el('button', 'as-new', 'New chat');
      header.append(statusDot, statusText, modelSelect, speaker, newChat);

      const log = el('div', 'as-log');
      log.setAttribute('role', 'log');
      log.setAttribute('aria-live', 'polite');
      const chips = el('div', 'as-chips');
      const inputWrap = el('div', 'as-inputwrap');
      const input = document.createElement('input');
      input.className = 'as-input';
      input.placeholder = 'Ask me anything about this computer…';
      input.spellcheck = false;
      input.setAttribute('aria-label', 'Message');
      const send = el('button', 'as-send', '➤');
      send.setAttribute('aria-label', 'Send');
      const mic = el('button', 'as-mic', '🎤');
      mic.setAttribute('aria-label', 'Talk');
      mic.disabled = true;
      inputWrap.append(input, mic, send);
      root.append(header, log, chips, inputWrap);

      // ── state ──
      let conversationId = null;
      let turn = null;
      let bubble = null;
      let agentOnline = null; // null = unknown
      const approvals = new Set(); // functions that settle a pending approval card
      const cards = new Map();
      // Voice: the service says what works; the shell just starts and stops it.
      let canListen = false;
      let canSpeak = false;
      let voiceState = 'idle';
      let speakReplies = false;
      let voiceInitialised = false;

      const scroll = () => { log.scrollTop = log.scrollHeight; };
      const add = (text, kind) => {
        const row = el('div', `as-msg as-${kind}`, text);
        log.append(row);
        scroll();
        return row;
      };

      function setStatus(state, text) {
        statusDot.dataset.state = state;
        statusText.textContent = text;
      }

      function updateMic() {
        mic.disabled = !canListen || Boolean(turn) || voiceState === 'transcribing';
        mic.dataset.state = voiceState;
        mic.textContent = { listening: '■', transcribing: '…' }[voiceState] ?? '🎤';
        mic.setAttribute('aria-label', voiceState === 'listening' ? 'Stop and send' : 'Talk');
      }

      function setSpeakReplies(on) {
        speakReplies = on;
        speaker.setAttribute('aria-pressed', String(on));
        speaker.textContent = on ? '🔊' : '🔈';
      }

      async function refreshVoice() {
        let status = null;
        try {
          status = await client.voiceStatus();
        } catch { /* the service is down: voice is off */ }
        canListen = Boolean(status?.can_listen);
        canSpeak = Boolean(status?.can_speak);
        const why = status?.problems?.[0] ?? 'Voice is not available';
        mic.title = canListen ? 'Talk to the assistant' : why;
        speaker.title = canSpeak ? 'Read replies aloud' : why;
        speaker.disabled = !canSpeak;
        if (!voiceInitialised && status) {
          voiceInitialised = true;
          setSpeakReplies(canSpeak && Boolean(status.speak_replies));
        }
        if (!canSpeak) setSpeakReplies(false);
        updateMic();
      }

      function onVoiceEvent(event) {
        if (event.type === 'voice_state') {
          voiceState = event.state;
          updateMic();
        } else if (event.type === 'voice_transcript') {
          ask(event.text);
        } else if (event.type === 'voice_error') {
          add(event.message, 'notice');
        }
      }
      const stopVoiceEvents = client.onVoice(onVoiceEvent);

      async function toggleListening() {
        try {
          if (voiceState === 'listening') await client.voiceStop();
          else await client.voiceListen();
        } catch (err) {
          add(`Voice: ${err.message}`, 'notice');
        }
      }

      async function refreshStatus() {
        refreshVoice();
        try {
          const s = await client.status();
          agentOnline = true;
          const local = s.local;
          if (local?.available) setStatus('ok', `Local · ${local.model}`);
          else if (s.cloud?.available) setStatus('ok', `Cloud · ${s.cloud.model}`);
          else if (local) setStatus('warn', `${local.model} is not installed or running`);
          else setStatus('warn', 'No model configured');
          modelSelect.querySelector('[value="cloud"]').disabled = !s.cloud;
          modelSelect.querySelector('[value="local"]').disabled = !s.local;
        } catch {
          agentOnline = false;
          setStatus('off', 'Assistant offline · basic commands only');
        }
      }

      // ── rendering of one request ──
      function startBubble() {
        if (!bubble) bubble = add('', 'agent');
        return bubble;
      }

      function onText(delta) {
        const b = startBubble();
        b.textContent += delta;
        scroll();
      }

      function onToolCall(event) {
        bubble = null;
        const card = el('div', 'as-tool');
        const head = el('button', 'as-tool-head');
        head.append(
          el('span', 'as-tool-icon', event.destructive ? '⚠' : '⚙'),
          el('span', 'as-tool-title', event.title || event.name),
          el('span', 'as-tool-args', summarizeArguments(event.arguments)),
        );
        const state = el('span', 'as-tool-state', event.needs_approval ? 'waiting' : 'running…');
        head.append(state);
        const output = el('pre', 'as-tool-output');
        output.hidden = true;
        head.addEventListener('click', () => { output.hidden = !output.hidden; });
        card.append(head, output);
        log.append(card);
        cards.set(event.call_id, { card, state, output });
        scroll();
      }

      function onToolResult(event) {
        const entry = cards.get(event.call_id);
        if (!entry) return;
        entry.state.textContent = event.ok ? '✓' : '✕';
        entry.card.dataset.result = event.ok ? 'ok' : 'failed';
        entry.output.textContent = event.output || '(no output)';
        entry.card.querySelector('.as-approval')?.remove();
        if (!event.ok) entry.output.hidden = false;
      }

      function onApproval(event) {
        const entry = cards.get(event.call_id);
        return new Promise((resolve) => {
          const box = el('div', 'as-approval');
          box.dataset.risk = event.destructive ? 'high' : 'normal';
          box.append(el('div', 'as-approval-text',
            event.destructive ? 'This can’t easily be undone. Allow it?' : 'Allow this action?'));
          const buttons = el('div', 'as-approval-buttons');
          const deny = el('button', 'as-deny', 'Deny');
          const approve = el('button', 'as-approve', 'Approve');
          buttons.append(deny, approve);
          box.append(buttons);
          let settled = false;
          const settle = (ok, label) => {
            if (settled) return;
            settled = true;
            approvals.delete(expire);
            box.replaceChildren(el('div', 'as-approval-text', label));
            box.dataset.risk = 'done';
            resolve(ok);
          };
          const expire = () => settle(false, 'No answer, so it was not run.');
          approvals.add(expire);
          approve.addEventListener('click', () => settle(true, 'Approved'));
          deny.addEventListener('click', () => settle(false, 'Denied'));
          (entry?.card ?? log).append(box);
          scroll();
        });
      }

      function setRunning(running) {
        send.textContent = running ? '■' : '➤';
        send.setAttribute('aria-label', running ? 'Stop' : 'Send');
        send.dataset.running = running ? '1' : '';
        input.disabled = running;
        updateMic();
        if (!running) input.focus();
      }

      async function ask(text) {
        text = text.trim();
        if (!text || turn) return;
        if (voiceState === 'speaking') client.silence().catch(() => {});
        services.behavior?.recordAssistantQuery(text);
        chips.hidden = true;
        add(text, 'user');
        bubble = null;
        setRunning(true);

        try {
          turn = await client.start(text, {
            conversationId,
            model: modelSelect.value,
            clientTools: conversationId ? undefined : tools.specs(),
            onText,
            onToolCall,
            onToolResult,
            onApproval,
            onNotice: (message) => add(message, 'notice'),
            runClientTool: (event) => tools.run(event),
          });
          conversationId = turn.conversationId;
          agentOnline = true;
          const result = await turn.done;
          if (result.status === 'error') {
            add(result.message, 'error');
            if (/not (set up|running|installed)|No AI model/i.test(result.message)) {
              // Give the user something useful when no model is available.
              add(await ruleBasedReply(text, { wm: engine, services }), 'agent');
            }
          } else if (result.status === 'cancelled') {
            add('Stopped.', 'notice');
          } else if (result.provider) {
            if (speakReplies && result.text) client.speak(result.text).catch(() => {});
            const cloud = result.provider === 'claude';
            const meta = el('div', 'as-meta', `${cloud ? '☁ Cloud' : 'Local'} · ${result.model}`);
            if (cloud) meta.dataset.cloud = '1';
            log.append(meta);
            scroll();
          }
        } catch (err) {
          if (err instanceof AgentUnavailableError) {
            agentOnline = false;
            setStatus('off', 'Assistant offline · basic commands only');
            add(await ruleBasedReply(text, { wm: engine, services }), 'agent');
          } else {
            add(`Something went wrong: ${err.message}`, 'error');
          }
        } finally {
          for (const settle of [...approvals]) settle();
          turn = null;
          setRunning(false);
          if (agentOnline === false) refreshStatus();
        }
      }

      function resetConversation() {
        turn?.cancel();
        if (voiceState === 'speaking') client.silence().catch(() => {});
        client.reset(conversationId).catch(() => {});
        conversationId = null;
        log.replaceChildren();
        cards.clear();
        chips.hidden = false;
        greet();
      }

      function greet() {
        add('Hi, I’m your OS assistant. I can look into what the computer is doing, open apps, '
          + 'change settings and manage windows. I’ll ask before anything risky.', 'agent');
      }

      // ── wiring ──
      for (const text of services.settings().assistantSuggestions) {
        const chip = el('button', 'as-chip', text);
        chip.addEventListener('click', () => ask(text));
        chips.append(chip);
      }
      send.addEventListener('click', () => {
        if (turn) turn.cancel();
        else { const v = input.value; input.value = ''; ask(v); }
      });
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && !e.isComposing) { const v = input.value; input.value = ''; ask(v); }
      });
      newChat.addEventListener('click', resetConversation);
      mic.addEventListener('click', toggleListening);
      speaker.addEventListener('click', () => {
        setSpeakReplies(!speakReplies);
        if (!speakReplies && voiceState === 'speaking') client.silence().catch(() => {});
      });
      root.addEventListener('mousedown', (e) => {
        if (!e.target.closest('button, select')) setTimeout(() => input.focus(), 0);
      });

      greet();
      refreshStatus();
      setTimeout(() => input.focus(), 30);

      const dispose = () => {
        stopVoiceEvents();
        if (voiceState === 'listening') client.voiceCancel().catch(() => {});
        if (voiceState === 'speaking') client.silence().catch(() => {});
      };
      const instance = { win: ctx.win, ask, dispose };
      instances.add(instance);
      ctx.win._assistantInstance = instance;
    },
    unmount(win) {
      win._assistantInstance?.dispose?.();
      instances.delete(win._assistantInstance);
    },
  };
}
