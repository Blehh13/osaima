/**
 * launcher-app.js — the Ask bar (Super+Space): a small floating window over
 * whatever the user is doing. Type a request and press Enter: it goes to the
 * assistant, which comes forward and handles it; the bar closes itself.
 * Up/Down pick one of your routines or frequent requests; Esc closes.
 */
import { iconEl } from './icons.js';

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

export function makeLauncher(services) {
  return {
    id: 'launcher',
    title: 'Launcher',
    mount(root) {
      root.classList.add('app-launcher');
      const form = document.createElement('form');
      form.className = 'ln-bar';
      const mark = el('span', 'ln-mark');
      mark.append(iconEl('sparkles', 22));
      const input = document.createElement('input');
      input.className = 'ln-input';
      input.placeholder = 'Ask your computer to do something…';
      input.spellcheck = false;
      input.setAttribute('aria-label', 'Request');
      form.append(mark, input);
      const list = el('ul', 'ln-list');
      const hint = el('p', 'ln-hint');
      for (const [k, t] of [['Enter', 'ask'], ['↑↓', 'pick'], ['Esc', 'close']]) hint.append(el('kbd', null, k), el('span', null, t));
      root.append(form, list, hint);

      let items = [];
      let sel = -1;
      const render = () => {
        list.replaceChildren(...items.map((it, i) => {
          const li = el('li', 'ln-item' + (i === sel ? ' is-sel' : ''));
          const ic = el('span', 'ln-item-icon');
          ic.append(iconEl({ Routine: 'repeat', 'You often ask': 'history', Try: 'sparkles' }[it.kind] || 'sparkles', 16));
          li.append(ic, el('span', 'ln-item-text', it.text), el('span', 'ln-item-kind', it.kind));
          li.addEventListener('mousedown', (e) => { e.preventDefault(); send(it.run); });
          return li;
        }));
      };
      (async () => {
        let routines = [], often = [];
        try { routines = ((await services.invoke('routines', {})) || {}).routines || []; } catch {}
        try { often = ((await services.invoke('suggestions', { n: 4 })) || {}).suggestions || []; } catch {}
        items = [
          ...routines.slice(0, 4).map((r) => ({ text: r.name, kind: 'Routine', run: `run my ${r.name} routine` })),
          ...often.slice(0, 4).map((s) => ({ text: s, kind: 'You often ask', run: s })),
        ];
        // a new user has nothing learned yet: show what the bar is for
        if (!items.length) {
          items = ['Open the file manager', 'What is using the most memory right now?', 'Find PDFs bigger than 10 MB',
                   'Set a timer for 10 minutes', 'Write a short email to my guide']
            .map((t) => ({ text: t, kind: 'Try', run: t }));
        }
        render();
      })();

      const close = () => services.invoke('os_action', { action: 'window', op: 'close', app: 'launcher' }).catch(() => window.close());
      async function send(text) {
        const q = String(text || '').trim();
        if (!q) return;
        input.disabled = true;
        try {
          await services.invoke('ask', { query: q });
          await services.invoke('os_action', { action: 'app', op: 'launch', app: 'assistant' });
        } catch (e) {
          input.disabled = false;
          hint.textContent = "Couldn't reach the assistant: " + (e.message || e);
          return;
        }
        close();
      }
      form.addEventListener('submit', (e) => { e.preventDefault(); send(sel >= 0 && !input.value.trim() ? items[sel].run : input.value); });
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') close();
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          if (!items.length) return;
          sel = (sel + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
          input.value = items[sel].run;
          render();
        }
      });
      input.addEventListener('input', () => { sel = -1; render(); });
      setTimeout(() => input.focus(), 30);
    },
  };
}
