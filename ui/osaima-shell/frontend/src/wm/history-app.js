/**
 * history-app.js — the History window: everything OSAIMA has done or held
 * back, from the tamper-evident activity log, newest first.
 */
import { iconEl, chip, skeleton, emptyState, withIcon } from './icons.js';

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

const PAGE = 50;
const FETCH = 200;
const cap = (s) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);
const base = (p) => String(p || '').split('/').filter(Boolean).pop() || String(p || '');
const words = (s) => String(s || '').replace(/_/g, ' ').trim();
const OS_EVENTS = ['os_action', 'os_refused', 'os_failed'];
const act = (e) => (e.data && e.data.action) || '';

// events narrows the query on the daemon; match narrows further here.
const FILTERS = [
  { id: 'all', label: 'All', events: [] },
  { id: 'commands', label: 'Commands', events: ['execute', 'execute_failed', 'refused', 'assess'] },
  { id: 'files', label: 'Files', events: OS_EVENTS, match: (e) => act(e) === 'files' },
  { id: 'apps', label: 'Apps & windows', events: OS_EVENTS, match: (e) => ['app', 'window', 'workspace', 'layout'].includes(act(e)) },
  { id: 'settings', label: 'Settings', events: OS_EVENTS, match: (e) => ['settings', 'volume', 'brightness', 'wifi', 'bluetooth', 'lock_screen', 'power'].includes(act(e)) },
  { id: 'tasks', label: 'Tasks', events: ['task_start', 'task_step', 'task_done', 'task_refused'] },
  { id: 'automations', label: 'Automations', events: ['automation_add', 'automation_remove', 'automation_run', 'automation_cancelled'] },
  { id: 'held', label: 'Held back', events: ['refused', 'os_refused', 'task_refused', 'tainted_proposal', 'injection_blocked'] },
  { id: 'problems', label: 'Problems', events: ['execute_failed', 'os_failed', 'injection_blocked', 'automation_run'], match: (e) => e.event !== 'automation_run' || e.data?.ok === false },
];

function describeOs(d) {
  const a = d.action || '';
  const op = d.op || '';
  if (a === 'files') {
    const target = base(d.path || (d.paths || [])[0] || d.name || 'a file');
    const verb = { search: 'searched your files', list: 'listed a folder', open: `opened ${target}`, reveal: `showed ${target} in Files`,
      copy: `copied ${target}`, move: `moved ${target}`, rename: `renamed ${target}`, trash: `moved ${target} to the bin`,
      delete: `deleted ${target}`, mkdir: `made the folder ${d.name || target}`, undo: 'undid the last file change' }[op];
    return verb || `${words(op)} ${target}`.trim();
  }
  if (a === 'app') return `${op === 'close' ? 'close' : 'open'} ${d.app || 'an app'}`;
  if (a === 'window') return d.app ? `${words(op)} ${d.app}` : `${words(op)} a window`;
  if (a === 'volume') return { get: 'checked the volume', set: 'changed the volume', up: 'turned the volume up', down: 'turned the volume down',
    mute: 'muted the sound', unmute: 'unmuted the sound', toggle_mute: 'toggled mute' }[op] || 'changed the volume';
  if (a === 'wifi') return op === 'on' ? 'turned Wi-Fi on' : op === 'off' ? 'turned Wi-Fi off' : 'checked Wi-Fi';
  if (a === 'bluetooth') return op === 'on' ? 'turned Bluetooth on' : op === 'off' ? 'turned Bluetooth off' : 'checked Bluetooth';
  if (a === 'lock_screen') return 'locked the screen';
  if (a === 'power') return `${words(op)} the computer`;
  if (a === 'brightness') return 'changed the brightness';
  if (a === 'settings') return op === 'get' ? 'read the system settings' : `changed a setting (${words(op)})`;
  if (a === 'notify') return 'showed a notification';
  return `${words(a)} ${words(op)}`.trim() || 'did something';
}

/** [outcome kind, plain-language sentence] for one log record. */
function sentence(e) {
  const d = e.data || {};
  switch (e.event) {
    case 'execute': return ['ok', `Ran ${d.command}`];
    case 'execute_failed': return ['bad', `Couldn't run ${d.command}${d.error ? ': ' + d.error : ''}`];
    case 'refused': return ['held', `Held back ${d.command} until you approve it`];
    case 'assess': return ['info', `Checked how risky ${d.command} is`];
    case 'os_action': return ['ok', cap(describeOs(d))];
    case 'os_refused': return ['held', `Held back: ${describeOs(d)}`];
    case 'os_failed': return ['bad', `Couldn't ${describeOs(d)}${d.error ? ': ' + d.error : ''}`];
    case 'task_start': return ['ok', `Started task: ${d.title || d.goal || ''}`];
    case 'task_step': return ['info', `Task step: ${d.detail || d.step || d.command || ''}`.trim()];
    case 'automation_add': return ['ok', `Set up automation: ${d.summary || ''}`];
    case 'automation_remove': return ['ok', 'Removed an automation'];
    case 'automation_run': return [d.ok ? 'ok' : 'bad', `Automation ran: ${d.automation || ''}${d.ok ? '' : ` — failed: ${d.error || 'unknown error'}`}`];
    case 'automation_cancelled': return ['held', `Cancelled: ${d.automation || 'a pending automation'}`];
    case 'task_done': return ['ok', `Finished task: ${d.title || ''}${d.steps ? ` (${d.steps} steps)` : ''}`];
    case 'task_refused': return ['held', `Held back in a task: ${d.detail || ''}${d.why ? ' (' + d.why + ')' : ''}`];
    case 'tainted_proposal': return ['held', `Held back a suggestion that came from outside content${d.query ? ': ' + d.query : ''}`];
    case 'injection_blocked': return ['bad', `Blocked a suspicious reply${d.query ? ' to: ' + d.query : ''}`];
    default: return ['info', cap(words(e.event))];
  }
}

const BADGE = { ok: 'Done', held: 'Held back', bad: 'Problem', info: 'Note' };
const KIND_ICON = { ok: 'check', held: 'pause', bad: 'alert-circle', info: 'info' };

function dayLabel(ts) {
  const d = new Date(ts);
  const start = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((start(new Date()) - start(d)) / 86400000);
  if (diff === 0) return 'Today';
  if (diff === 1) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long', year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric' });
}

export function makeHistory(services) {
  return {
    id: 'history',
    title: 'History',
    mount(root) {
      root.classList.add('app-history');
      const head = el('div', 'hi-head');
      const titleRow = el('div', 'hi-title');
      const integrity = el('span', 'hi-integrity');
      integrity.append(chip('Checking…', 'loader', 'muted', 'is-busy'));
      titleRow.append(el('h2', '', 'History'), integrity);
      head.append(titleRow);
      const searchBox = el('label', 'hi-searchbox');
      const search = el('input', 'hi-search');
      search.type = 'search';
      search.placeholder = 'Search what happened…';
      search.setAttribute('aria-label', 'Search history');
      search.autocomplete = 'off';
      search.spellcheck = false;
      const chipRow = el('div', 'hi-chips');
      chipRow.setAttribute('role', 'group');
      const list = el('div', 'hi-list');
      const more = withIcon(el('button', 'hi-more'), 'chevron-down', 'Load more');
      more.type = 'button';
      more.hidden = true;
      const foot = el('p', 'hi-foot');
      searchBox.append(iconEl('search', 16), search);
      head.append(searchBox, chipRow);
      root.replaceChildren(head, list, foot, more);

      let filter = FILTERS[0];
      let shown = 0;     // rows on screen
      let offset = 0;    // how far into the daemon's results we have read
      let total = 0;     // how many match on the daemon
      let busy = false;
      let token = 0;
      let lastDay = null;

      const chips = FILTERS.map((f) => {
        const b = el('button', 'hi-chip', f.label);
        b.type = 'button';
        b.addEventListener('click', () => { if (filter !== f) { filter = f; paint(); load(true); } });
        chipRow.append(b);
        return [f, b];
      });
      const paint = () => chips.forEach(([f, b]) => { b.classList.toggle('on', f === filter); b.setAttribute('aria-pressed', String(f === filter)); });

      const addRow = (e) => {
        const day = dayLabel(e.ts);
        if (day !== lastDay) { lastDay = day; list.append(el('h3', 'hi-day', day)); }
        const [kind, text] = sentence(e);
        const row = el('div', 'hi-row');
        row.dataset.kind = kind;
        const top = el('button', 'hi-line');
        top.type = 'button';
        top.setAttribute('aria-expanded', 'false');
        const badge = el('span', 'hi-badge');
        badge.title = BADGE[kind];
        badge.append(iconEl(KIND_ICON[kind], 14), el('span', 'hi-sr', BADGE[kind]));
        top.append(
          badge,
          el('span', 'hi-text', text),
          el('time', 'hi-time', new Date(e.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })),
          iconEl('chevron-down', 14, 'hi-chev'));
        let pre = null;
        top.addEventListener('click', () => {
          if (pre) { pre.remove(); pre = null; top.setAttribute('aria-expanded', 'false'); return; }
          pre = el('pre', 'hi-json', JSON.stringify(e, null, 2));
          row.append(pre);
          top.setAttribute('aria-expanded', 'true');
        });
        row.append(top);
        list.append(row);
      };

      async function load(reset) {
        if (busy && !reset) return;
        const mine = ++token;
        busy = true;
        if (reset) { shown = 0; offset = 0; total = 0; lastDay = null; list.replaceChildren(skeleton(5, 'hi-skel')); more.hidden = true; foot.textContent = ''; }
        more.disabled = true;
        const text = search.value.trim();
        const added = [];
        try {
          // A chip that needs more than the event name is applied here, so keep
          // reading until a page is full or the log runs out.
          while (added.length < PAGE) {
            const r = await services.invoke('audit_query', { offset, limit: filter.match ? FETCH : PAGE - added.length, events: filter.events, text });
            if (mine !== token) return;
            const got = r.entries || [];
            total = r.total || 0;
            offset += got.length;
            for (const e of got) if (!filter.match || filter.match(e)) added.push(e);
            if (!got.length || offset >= total) break;
          }
          if (reset || !shown) list.querySelector('.hi-skel')?.remove();
          added.forEach(addRow);
          shown += added.length;
          const done = offset >= total;
          more.hidden = done;
          foot.textContent = shown ? (done ? `${shown} ${shown === 1 ? 'entry' : 'entries'}` : `Showing ${shown}`) : '';
          if (!shown) list.replaceChildren(text || filter.id !== 'all' ? emptyState('search', 'Nothing matches') : emptyState('history', 'Nothing recorded yet'));
        } catch (err) {
          if (mine !== token) return;
          list.replaceChildren(emptyState('alert-circle', "Can't read the activity log" + (err && err.message ? ': ' + err.message : '')));
        }
        busy = false;
        more.disabled = false;
      }

      more.addEventListener('click', () => load(false));
      let timer = null;
      search.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(() => load(true), 300); });

      (async () => {
        try {
          const v = (await services.invoke('audit_tail', { n: 1 }))?.verified || {};
          const c = v.ok ? chip(`Verified · ${v.records}`, 'shield-check', 'ok') : chip(`Altered at entry ${v.line}`, 'shield-alert', 'bad');
          c.title = v.ok ? `Log intact: all ${v.records} entries check out. Each carries a fingerprint of the one before it, so editing or deleting any entry would show here.`
            : `The log was altered at entry ${v.line}.`;
          integrity.replaceChildren(c);
        } catch { integrity.replaceChildren(chip("Can't verify", 'alert-circle', 'bad')); }
      })();

      paint();
      load(true);
    },
  };
}
