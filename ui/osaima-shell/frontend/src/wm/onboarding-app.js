/**
 * onboarding-app.js — the first-run welcome: what OSAIMA is, how it thinks
 * (Fast or Private), a first request, and how approvals and History work.
 * Shown once (a flag kept by the daemon in ~/.config/osaima/ui.json, mirrored
 * in localStorage) and reopenable from Settings → AI.
 */
import { iconEl, withIcon, emptyState, skeleton } from './icons.js';

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
const errText = (e) => String((e && e.message) || e || 'unknown error');

/** True when the welcome has been shown before (daemon flag, else localStorage). */
export async function onboardingDone(invoke) {
  try { if (localStorage.getItem('osaima_onboarded')) return true; } catch {}
  try {
    const r = await invoke('ui_flag', { name: 'onboarded' });
    return !!(r && r.value);
  } catch { return false; }
}

export async function markOnboarded(invoke) {
  try { localStorage.setItem('osaima_onboarded', '1'); localStorage.setItem('osaima_welcomed', '1'); } catch {}
  try { await invoke('ui_flag', { name: 'onboarded', value: true }); } catch {}
}

const STEPS = ['Welcome', 'How it thinks', 'Try it', 'You stay in charge'];

export function makeOnboarding(services) {
  return {
    id: 'onboarding',
    title: 'Welcome to OSAIMA',
    mount(root, ctx) {
      root.classList.add('app-onboarding');
      let step = 0;
      let mode = null;   // 'cloud' | 'local' once saved here

      const shell = el('div', 'ob-shell');
      const dots = el('ol', 'ob-dots');
      dots.setAttribute('aria-label', 'Progress');
      STEPS.forEach((s) => { const li = el('li', 'ob-dot'); li.title = s; dots.append(li); });
      const stage = el('div', 'ob-stage');
      const nav = el('div', 'ob-nav');
      const back = el('button', 'ob-btn', 'Back');
      const next = el('button', 'ob-btn ob-btn-go', 'Next');
      back.type = next.type = 'button';
      const skip = el('button', 'ob-skip', 'Skip');
      skip.type = 'button';
      nav.append(skip, el('span', 'ob-spacer'), back, next);
      shell.append(dots, stage, nav);
      root.append(shell);

      async function finish(openAssistant) {
        await markOnboarded(services.invoke);
        if (openAssistant) services.invoke('os_action', { action: 'app', op: 'launch', app: 'assistant', approved: false }).catch(() => {});
        if (services.desktopMode) services.invoke('os_action', { action: 'window', op: 'close', app: 'onboarding' }).catch(() => {});
        else stage.replaceChildren(el('p', 'ob-lede', 'All set. You can reopen this tour from Settings → AI.'));
      }
      skip.addEventListener('click', () => finish(true));
      back.addEventListener('click', () => go(step - 1));
      next.addEventListener('click', () => (step === STEPS.length - 1 ? finish(true) : go(step + 1)));

      const panel = (title, lede, ic) => {
        const p = el('section', 'ob-panel');
        if (ic) { const m = el('span', 'ob-mark'); m.append(iconEl(ic, 26)); p.append(m); }
        const h = el('h2', 'ob-title', title);
        h.tabIndex = -1;
        p.append(h, el('p', 'ob-lede', lede));
        return p;
      };

      function welcome() {
        const p = panel('Your computer, in plain words', 'Say what you want. OSAIMA shows the plan and acts only after you agree.', 'sparkles');
        const list = el('ul', 'ob-points');
        for (const [ic, h, t] of [
          ['chat', 'Understands you', 'Instantly on this computer, or with a language model.'],
          ['route', 'Shows its work', 'Every reply can show how it was handled.'],
          ['shield-check', 'Keeps a sealed record', 'Every action is logged. Edits would show.'],
        ]) {
          const li = el('li', 'ob-point');
          const i = el('span', 'ob-point-icon');
          i.append(iconEl(ic, 20));
          const txt = el('span', 'ob-point-text');
          txt.append(el('strong', null, h), el('span', null, t));
          li.append(i, txt);
          list.append(li);
        }
        p.append(list);
        return p;
      }

      function thinking() {
        const p = panel('Choose how it thinks', 'You can change this any time in Settings.', 'bulb');
        const choices = el('div', 'ob-choices');
        const msg = el('p', 'ob-msg');
        msg.setAttribute('role', 'status');
        const mk = (id, name, desc, ic) => {
          const b = el('button', 'ob-choice');
          b.type = 'button';
          b.dataset.mode = id;
          const i = el('span', 'ob-choice-icon');
          i.append(iconEl(ic, 22));
          b.append(i, el('span', 'ob-choice-name', name), el('span', 'ob-choice-desc', desc));
          b.setAttribute('aria-pressed', String(mode === id));
          return b;
        };
        const fast = mk('cloud', 'Fast', 'Gemini · answers in seconds · your text goes to Google', 'zap');
        const priv = mk('local', 'Private', 'On this computer · nothing leaves · slower', 'lock');
        choices.append(fast, priv);
        const keyBox = el('div', 'ob-key');
        keyBox.hidden = true;
        const label = el('label', null, 'Gemini API key');
        label.htmlFor = 'ob-key';
        const key = document.createElement('input');
        key.id = 'ob-key'; key.type = 'password'; key.placeholder = 'Paste your key…'; key.autocomplete = 'off'; key.spellcheck = false;
        const save = el('button', 'ob-btn ob-btn-go', 'Save key');
        save.type = 'button';
        keyBox.append(label, key, save, el('p', 'ob-hint', 'Stored only on this computer · free at aistudio.google.com'));
        p.append(choices, keyBox, msg);

        const select = (id) => {
          choices.querySelectorAll('.ob-choice').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.mode === id)));
          keyBox.hidden = id !== 'cloud';
        };
        priv.addEventListener('click', async () => {
          select('local'); msg.className = 'ob-msg'; msg.textContent = 'Saving…';
          try { await services.invoke('set_llm_mode', { mode: 'local' }); mode = 'local'; msg.textContent = 'Private mode is on · the model takes about a minute to start'; }
          catch (e) { msg.className = 'ob-msg is-bad'; msg.textContent = "Couldn't save: " + errText(e); }
        });
        fast.addEventListener('click', async () => {
          select('cloud');
          // a key already saved means there is nothing to type
          try {
            const st = await services.invoke('ai_status');
            if ((st.cloud_fallbacks || []).length) {
              await services.invoke('set_llm_mode', { mode: 'cloud' });
              mode = 'cloud'; keyBox.hidden = true; msg.className = 'ob-msg'; msg.textContent = 'Fast mode is on · Gemini is connected';
            }
          } catch {}
        });
        save.addEventListener('click', async () => {
          const k = key.value.trim();
          if (!k) { msg.className = 'ob-msg is-bad'; msg.textContent = 'Paste your Gemini key first.'; return; }
          save.disabled = true; msg.className = 'ob-msg'; msg.textContent = 'Checking the key…';
          try {
            await services.invoke('validate_api_key', { key: k });
            await services.invoke('set_api_key', { key: k });
            await services.invoke('set_llm_mode', { mode: 'cloud' });
            key.value = ''; mode = 'cloud'; keyBox.hidden = true;
            msg.textContent = 'Fast mode is on.';
          } catch (e) { msg.className = 'ob-msg is-bad'; msg.textContent = "That didn't work: " + errText(e); }
          save.disabled = false;
        });
        if (mode) select(mode);
        return p;
      }

      function tryIt() {
        const p = panel('Try a request', 'See how OSAIMA understands you. Nothing changes.', 'chat');
        const form = document.createElement('form');
        form.className = 'ob-try';
        const input = document.createElement('input');
        input.placeholder = 'What is using the most memory?';
        input.spellcheck = false;
        input.setAttribute('aria-label', 'A request to try');
        const go = withIcon(el('button', 'ob-btn ob-btn-go'), 'arrow-right', 'Try');
        go.type = 'submit';
        form.append(input, go);
        const examples = el('div', 'ob-examples');
        for (const s of ['Set a timer for 10 minutes', 'Find PDFs bigger than 10 MB', 'Open the file manager']) {
          const b = el('button', 'ob-example', s);
          b.type = 'button';
          b.addEventListener('click', () => { input.value = s; form.requestSubmit(); });
          examples.append(b);
        }
        const out = el('div', 'ob-out');
        out.setAttribute('aria-live', 'polite');
        out.append(emptyState('sparkles', 'Pick an example or type your own'));
        const keys = el('p', 'ob-keys');
        keys.title = 'Voice typing (F9) dictates into the box where it is available; Super+Space opens the Ask bar anywhere.';
        keys.append(iconEl('mic', 14), el('kbd', null, 'F9'), el('span', null, 'to speak'), el('kbd', null, 'Super+Space'), el('span', null, 'Ask bar'));
        p.append(form, examples, out, keys);
        form.addEventListener('submit', async (e) => {
          e.preventDefault();
          const q = input.value.trim();
          if (!q) return;
          go.disabled = true;
          out.replaceChildren(skeleton(2));
          try {
            const r = await services.invoke('classify_intent', { query: q, history: [], selected: [] });
            const card = el('div', 'ob-result');
            const how = r.source === 'fast' ? `Understood on this computer in ${Math.round(r.ms || 0)} ms`
              : r.source === 'llm' ? `Answered by ${/^gemini/.test(r.backend || '') ? 'Gemini' : 'the on-device model'} in ${((r.ms || 0) / 1000).toFixed(1)} s`
              : `Handled as ${r.source || 'a reply'}`;
            card.append(el('span', 'ob-chip', String(r.intent || 'unknown').replace(/_/g, ' ')), el('span', 'ob-how', how));
            const reply = r.params && r.params.content ? r.params.content : r.reply;
            const lines = [card];
            if (reply) lines.push(el('p', 'ob-reply', String(reply).slice(0, 400)));
            if (r.params && r.params.command) { const c = el('pre', 'ob-cmd'); c.textContent = r.params.command; lines.push(c); }
            lines.push(el('p', 'ob-hint', 'In the assistant, you would approve this on a card.'));
            out.replaceChildren(...lines);
          } catch (err) {
            const m = el('p', 'ob-msg is-bad', "The AI core didn't answer: " + errText(err));
            out.replaceChildren(m);
          }
          go.disabled = false;
        });
        return p;
      }

      function charge() {
        const p = panel('You stay in charge', 'Anything that changes your computer asks first.', 'shield-check');
        const tiers = el('ul', 'ob-tiers');
        for (const [cls, ic, name, desc] of [
          ['read', 'eye', 'Only looks', 'Runs straight away, in a sandbox.'],
          ['rev', 'undo', 'Makes changes', 'Needs your OK. You can undo it.'],
          ['irr', 'alert', "Can't be undone", 'Needs your OK and a typed phrase.'],
        ]) {
          const li = el('li', 'ob-tier ob-tier-' + cls);
          const i = el('span', 'ob-tier-icon');
          i.append(iconEl(ic, 18));
          const txt = el('span', 'ob-point-text');
          txt.append(el('strong', null, name), el('span', null, desc));
          li.append(i, txt);
          tiers.append(li);
        }
        const open = el('div', 'ob-links');
        const link = (label, app, ic) => {
          const b = withIcon(el('button', 'ob-btn'), ic, label);
          b.type = 'button';
          b.addEventListener('click', () => services.invoke('os_action', { action: 'app', op: 'launch', app, approved: false }).catch(() => {}));
          return b;
        };
        open.append(link('History', 'history', 'history'), link('Security Center', 'security', 'shield'));
        p.append(tiers, open);
        return p;
      }

      const BUILD = [welcome, thinking, tryIt, charge];
      function go(n) {
        step = Math.max(0, Math.min(STEPS.length - 1, n));
        const panelEl = BUILD[step]();
        stage.replaceChildren(panelEl);
        [...dots.children].forEach((d, i) => { d.classList.toggle('on', i === step); d.classList.toggle('past', i < step); d.setAttribute('aria-current', i === step ? 'step' : 'false'); });
        back.hidden = step === 0;
        withIcon(next, step === STEPS.length - 1 ? 'check' : 'arrow-right', step === STEPS.length - 1 ? 'Start using OSAIMA' : step === 0 ? 'Get started' : 'Next');
        skip.hidden = step === STEPS.length - 1;
        const h = panelEl.querySelector('.ob-title');
        if (h) h.focus({ preventScroll: true });
      }
      go(0);
    },
  };
}
