/**
 * trace.js — "How I handled this": the real path one request took.
 *
 * Every stage is filled from data the daemon actually returned (router scores,
 * which backend answered and how long it took, the risk tier it assessed, the
 * sandbox layers it reports, the audit record it wrote). A stage with no data
 * says so ("not needed", "not reached"); nothing is invented.
 */
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

const pct = (x) => `${Math.round((Number(x) || 0) * 100)}%`;
const ms = (x) => (x >= 1000 ? `${(x / 1000).toFixed(1)} s` : `${Math.round(x)} ms`);
const words = (s) => String(s || '').replace(/_/g, ' ');

// Intents that change something on the computer (the daemon gates these).
const ACTION_INTENTS = new Set(['execute_command', 'os_action', 'orchestrate', 'power', 'files', 'open_app', 'close_window',
  'close_all_windows', 'focus_app', 'layout', 'workspace', 'search_web', 'minimize_all', 'shutdown']);

export const TIER_LABEL = { read_only: 'Read-only', reversible: 'Reversible', irreversible: 'Irreversible' };

/** What the sandbox string from the daemon ("bubblewrap+landlock+seccomp") means. */
export function sandboxLayers(s) {
  const t = String(s || '');
  return {
    bubblewrap: t.includes('bubblewrap'),
    landlock: t.includes('landlock(partial)') ? 'partial' : t.includes('landlock') ? 'full' : 'off',
    seccomp: t.includes('seccomp'),
    fallback: t.includes('systemd'),
    none: t === 'none',
  };
}

/** Trace patch for a finished execute_command reply. */
export function fromExecute(res) {
  if (!res) return {};
  return {
    risk: res.tier ? { tier: res.tier } : undefined,
    sandbox: res.sandbox,
    result: { exit: res.exit_code, bytes: (res.output || '').length },
    audit: res.audit,
  };
}

/** Trace patch for a finished os_action reply. */
export function fromOs(res) {
  if (!res) return {};
  return { risk: res.tier ? { tier: res.tier } : undefined, os: true, result: { done: true }, audit: res.audit };
}

export function createTrace(r, meta) {
  const intent = r && r.intent;
  const isAction = ACTION_INTENTS.has(intent);
  const d = {
    r: r || {}, meta: meta || {}, intent,
    risk: intent === 'execute_command' && r.params && r.params.risk ? { tier: r.params.risk.tier, reasons: r.params.risk.reasons } : null,
    approval: null, sandbox: null, os: false, result: null, audit: null,
  };

  const root = el('details', 'as-trace');
  const sum = el('summary', 'as-trace-sum');
  const sumMain = el('span', 'as-trace-title', 'How I handled this');
  const sumHint = el('span', 'as-trace-hint');
  sum.append(el('span', 'as-trace-chev'), sumMain, sumHint);
  const list = el('ol', 'as-trace-list');
  root.append(sum, list);

  const rows = {};
  for (const key of ['router', 'engine', 'guard', 'risk', 'approval', 'sandbox', 'result', 'audit']) {
    const li = el('li', 'as-trace-row');
    li.dataset.stage = key;
    const dot = el('span', 'as-trace-dot');
    const body = el('div', 'as-trace-body');
    const name = el('span', 'as-trace-name');
    const note = el('span', 'as-trace-note');
    body.append(name, note);
    li.append(dot, body);
    list.append(li);
    rows[key] = { li, name, note, last: '' };
  }
  // the order of lighting up on open
  Object.values(rows).forEach((r2, i) => r2.li.style.setProperty('--i', i));

  function set(key, state, title, note) {
    const row = rows[key];
    const sig = `${state}|${title}|${note}`;
    row.name.textContent = title;
    row.note.textContent = note || '';
    if (row.last && row.last !== sig) {
      row.li.classList.remove('is-flash');
      void row.li.offsetWidth;
      row.li.classList.add('is-flash');
    }
    row.last = sig;
    row.li.dataset.state = state;
  }

  function render() {
    const { r: res, meta: m } = d;
    // 1. intent router
    const router = res.router;
    if (router && router.intent) {
      const scores = (router.scores || []).map((s) => `${words(s.intent)} ${pct(s.confidence)}`);
      const head = scores.length ? scores.join(' · ') : `${words(router.intent)} ${pct(router.confidence)}`;
      set('router', 'done', 'Intent router', `${head}${router.ms != null ? ` · ${ms(router.ms)}` : ''}`);
    } else if (m.source === 'offline') {
      set('router', 'skip', 'Intent router', 'Not reached: the AI core did not answer');
    } else if (res.context_items > 0 && !res.router) {
      set('router', 'skip', 'Intent router', 'Skipped: outside content came with the request, so it goes to the model and the guard');
    } else {
      set('router', 'skip', 'Intent router', 'No verdict (the router service did not answer in time)');
    }

    // 2. fast path or language model
    if (m.source === 'fast') {
      set('engine', 'done', 'Fast path', `Understood on this computer, no language model used${m.ms != null ? ` · ${ms(m.ms)}` : ''}`);
    } else if (m.source === 'routine') {
      set('engine', 'done', 'Saved routine', 'Matched one of your routines; its steps still ask for approval');
    } else if (m.source === 'llm') {
      const b = String(m.backend || '');
      const who = /^gemini/i.test(b) ? 'Gemini (cloud)' : b ? `${b} (on this computer)` : 'Language model';
      const task = res.llm_task ? `${res.llm_task} · ` : '';
      set('engine', 'done', 'Language model', `${who} · ${task}${m.ms != null ? ms(m.ms) : ''}`.replace(/ · $/, ''));
    } else {
      set('engine', 'bad', 'Offline reply', 'The AI core did not answer, so this is a basic built-in reply');
    }

    // 3. outside-content guard
    const items = res.context_items || 0;
    const flags = (res.untrusted_flags || []).length;
    if (res.injection_detected) {
      set('guard', 'bad', 'Outside-content guard', 'The canary leaked: the reply was discarded');
    } else if (res.tainted) {
      const where = (res.context_from || []).filter((k) => k !== 'knowledge').join(', ');
      set('guard', flags ? 'warn' : 'done', 'Outside-content guard',
        `${items} outside item${items === 1 ? '' : 's'} fenced as untrusted${where ? ` (${where})` : ''}${flags ? ` · ${flags} instruction-like span${flags === 1 ? '' : 's'} removed` : ''} · anything proposed is marked tainted`);
    } else {
      set('guard', 'skip', 'Outside-content guard', 'No outside content in this request');
    }

    // 4. risk tier
    if (d.risk && d.risk.tier) {
      const why = (d.risk.reasons || []).join('; ').replace(/`/g, '');
      set('risk', d.risk.tier === 'irreversible' ? 'warn' : 'done', `Risk tier: ${TIER_LABEL[d.risk.tier] || d.risk.tier}`, why || 'Assessed by the daemon, not the interface');
    } else if (isAction) {
      set('risk', 'pending', 'Risk tier', 'Assessed by the daemon when the action is requested');
    } else {
      set('risk', 'skip', 'Risk tier', 'No action on the system was proposed');
    }

    // 5. approval
    if (d.approval) {
      const a = d.approval;
      set('approval', a.state === 'cancelled' ? 'warn' : 'done', a.title, a.note);
    } else if (isAction && !d.audit) {
      const tainted = !!res.tainted && !(res.context_from || []).every((k) => k === 'folder' || k === 'knowledge');
      set('approval', 'pending', 'Approval', tainted ? 'Needed: the action was suggested by outside content' : 'Waiting, if the action needs it');
    } else if (isAction && d.audit) {
      set('approval', 'done', 'Approval', 'Policy allowed this without asking');
    } else {
      set('approval', 'skip', 'Approval', 'Nothing to approve');
    }

    // 6. sandbox
    if (d.sandbox) {
      const L = sandboxLayers(d.sandbox);
      const parts = [];
      if (L.bubblewrap) parts.push('bubblewrap');
      if (L.landlock !== 'off') parts.push(L.landlock === 'full' ? 'Landlock' : 'Landlock (partial)');
      if (L.seccomp) parts.push('seccomp');
      if (L.fallback) parts.push('systemd transient service');
      set('sandbox', L.none ? 'warn' : 'done', 'Sandbox', parts.length ? parts.join(' + ') : 'No isolation reported');
    } else if (d.os) {
      set('sandbox', 'skip', 'Sandbox', 'Not a shell command: applied through the daemon’s action gate');
    } else if (res.intent === 'execute_command' || res.intent === 'orchestrate') {
      set('sandbox', 'pending', 'Sandbox', 'bubblewrap + Landlock + seccomp when it runs');
    } else {
      set('sandbox', 'skip', 'Sandbox', 'No command to run');
    }

    // 7. result
    if (d.result) {
      const x = d.result;
      if (x.exit != null) set('result', x.exit === 0 ? 'done' : 'warn', 'Result', `Exit code ${x.exit} · ${x.bytes} byte${x.bytes === 1 ? '' : 's'} of output`);
      else set('result', 'done', 'Result', 'Done');
    } else if (isAction) {
      set('result', 'pending', 'Result', 'Not run yet');
    } else {
      set('result', 'done', 'Result', 'Answered');
    }

    // 8. audit record
    if (d.audit && d.audit.seq) {
      set('audit', 'done', `Audit record #${d.audit.seq}`, `hash ${String(d.audit.hash || '').slice(0, 12)}… · chained to the record before it`);
    } else if (res.tainted && !isAction) {
      set('audit', 'done', 'Audit log', 'Tainted proposals are recorded');
    } else if (isAction) {
      set('audit', 'pending', 'Audit record', 'Written when the action runs');
    } else {
      set('audit', 'skip', 'Audit record', 'Nothing changed, so nothing to record');
    }

    const stages = Object.values(rows).filter((x) => x.li.dataset.state === 'done' || x.li.dataset.state === 'warn').length;
    const last = m.source === 'fast' ? `fast path · ${ms(m.ms || 0)}` : m.source === 'llm' ? `${/^gemini/i.test(m.backend || '') ? 'Gemini' : 'on-device model'} · ${ms(m.ms || 0)}` : words(m.source);
    sumHint.textContent = `${stages}/8 stages`;
    sumHint.title = last;
  }

  render();

  return {
    el: root,
    update(patch) {
      if (!patch) return;
      for (const k of Object.keys(patch)) if (patch[k] !== undefined) d[k] = patch[k];
      render();
    },
    /** The approval stage: how the user (or the policy) answered. */
    approval(state, typed) {
      d.approval = state === 'approved'
        ? { state, title: typed ? 'Approved with the typed phrase' : 'Approved by you', note: typed ? 'This can’t be undone, so you typed the confirm phrase' : 'You pressed Allow' }
        : state === 'cancelled'
          ? { state, title: 'Cancelled by you', note: 'Nothing ran' }
          : { state, title: 'Approval', note: '' };
      render();
    },
    notNeeded() {
      if (!d.approval) { d.approval = { state: 'auto', title: 'Approval', note: 'Not needed for a read-only command, and nothing outside suggested it' }; render(); }
    },
  };
}
