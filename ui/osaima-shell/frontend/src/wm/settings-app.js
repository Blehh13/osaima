/**
 * settings-app.js — the Settings window. A left navigation of pages: how
 * OSAIMA thinks (AI), then the system itself (look, screen, sound, network,
 * keyboard and mouse, notifications, lock, default apps, time). System state
 * comes from os_action {action:'settings', op:'get'}; each change is one
 * os_action call, saved by the daemon and re-applied at login.
 */
import { icon, iconEl, infoTip, chip, skeleton, emptyState, withIcon } from './icons.js';

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

const errText = (e) => (e && e.message ? e.message : String(e));
const KEEP_SECONDS = 15;

export function makeSettings(services) {
  return {
    id: 'settings',
    title: 'Settings',
    mount(root) {
      root.classList.add('app-settings');

      // os_action answers {tier, result}; callers want the result.
      const os = async (action, op, params = {}, approved = false) => {
        const r = await services.invoke('os_action', { action, op, ...params, approved });
        return r && r.result !== undefined ? r.result : r;
      };
      const setting = (op, value, extra = {}, approved = false) => os('settings', op, { value, ...extra }, approved);

      // ── layout ──
      const nav = el('nav', 'st-nav');
      nav.setAttribute('aria-label', 'Settings sections');
      const main = el('div', 'st-main');
      const keepBar = el('div', 'st-keep');
      keepBar.hidden = true;
      keepBar.setAttribute('role', 'alert');
      const keepText = el('span', 'st-keep-text');
      const keepYes = el('button', 'st-btn primary', 'Keep');
      const keepNo = el('button', 'st-btn', 'Revert');
      keepYes.type = keepNo.type = 'button';
      keepBar.append(keepText, keepNo, keepYes);
      const page = el('div', 'st-page');
      main.append(keepBar, page);
      root.replaceChildren(nav, main);

      // ── helpers ──
      /** A labelled row: [label + hint] on the left, control and inline status on the right. */
      const row = (label, hint, ...controls) => {
        const r = el('div', 'st-row');
        const left = el('div', 'st-row-label');
        left.append(el('span', 'st-row-name', label));
        if (hint) left.append(infoTip(hint, `About ${label}`));
        const right = el('div', 'st-row-ctl');
        const msg = el('span', 'st-inline');
        msg.setAttribute('role', 'status');
        right.append(...controls, msg);
        r.append(left, right);
        return { row: r, msg };
      };
      /** Disables the controls while fn runs and says "Saved" or the error beside them. */
      const say = (msg, state, text) => {
        msg.classList.remove('is-err', 'is-ok', 'is-busy');
        clearTimeout(msg._fade);
        if (!text) { msg.replaceChildren(); return; }
        if (state) msg.classList.add('is-' + state);
        msg.replaceChildren(iconEl({ busy: 'loader', ok: 'check', err: 'alert-circle' }[state] || 'info', 14), el('span', null, text));
        if (state === 'ok') msg._fade = setTimeout(() => { if (msg.classList.contains('is-ok')) msg.replaceChildren(); }, 2500);
      };
      const commit = async (controls, msg, fn) => {
        controls.forEach((c) => { c.disabled = true; });
        say(msg, 'busy', 'Saving…');
        let ok = false;
        try { await fn(); say(msg, 'ok', 'Saved'); ok = true; } catch (e) { say(msg, 'err', errText(e)); }
        controls.forEach((c) => { c.disabled = false; });
        return ok;
      };
      const select = (options, current) => {
        const s = el('select', 'st-select');
        for (const [value, label] of options) {
          const o = el('option', '', label);
          o.value = String(value);
          s.append(o);
        }
        s.value = String(current);
        return s;
      };
      const toggle = (on) => {
        const c = el('input', 'st-switch');
        c.type = 'checkbox';
        c.setAttribute('role', 'switch');
        c.checked = !!on;
        return c;
      };
      const slider = (min, max, step, value) => {
        const s = el('input', 'st-slider');
        s.type = 'range';
        s.min = min; s.max = max; s.step = step; s.value = value;
        return s;
      };
      const heading = (title, lede) => {
        const h = el('div', 'st-head');
        h.append(el('h2', 'st-title', title));
        if (lede) h.append(infoTip(lede, `About ${title}`));
        return h;
      };

      // ── "Keep these settings?" (display changes that could leave the screen unreadable) ──
      let pending = null;
      const endPending = () => { if (pending) { clearInterval(pending.timer); pending = null; } keepBar.hidden = true; };
      const resolvePending = async (keep) => {
        const p = pending;
        if (!p) return;
        clearInterval(p.timer);
        keepYes.disabled = keepNo.disabled = true;
        if (keep) {
          keepText.textContent = 'Kept.';
        } else {
          keepText.textContent = 'Reverting…';
          try { await p.revert(); } catch (e) { p.fail(errText(e)); }
        }
        pending = null;
        keepYes.disabled = keepNo.disabled = false;
        keepBar.hidden = true;
        p.done(keep);
      };
      keepYes.addEventListener('click', () => resolvePending(true));
      keepNo.addEventListener('click', () => resolvePending(false));
      /** Shows the bar; if nobody answers in 15 seconds the old value comes back. */
      const askKeep = (what, revert, done, fail) => {
        let left = KEEP_SECONDS;
        const tick = () => { keepText.textContent = `Keep this ${what}? Reverting in ${left} s`; };
        tick();
        keepBar.hidden = false;
        keepNo.focus();
        pending = {
          revert, done, fail,
          timer: setInterval(() => { left -= 1; if (left <= 0) resolvePending(false); else tick(); }, 1000),
        };
      };

      // ── pages ──
      let state = null;
      const loadState = async () => { state = await os('settings', 'get'); return state; };

      function aiPage(box) {
        box.classList.add('st-ai');
        box.innerHTML = `
          <div class="st-head"><h2 class="st-title">AI</h2><span class="st-ai-tip"></span></div>
          <ul class="ai-status-list st-status" aria-label="What OSAIMA needs"></ul>
          <fieldset class="mode-pick">
            <legend class="st-sr">How should OSAIMA think?</legend>
            <label class="mode-option">
              <input type="radio" name="st-mode" value="local" />
              <span class="mode-icon">${icon('lock', 20)}</span>
              <span class="mode-name">Private · free, the default</span>
              <span class="mode-desc">Stays on this computer · no account · slower</span>
            </label>
            <label class="mode-option">
              <input type="radio" name="st-mode" value="cloud" />
              <span class="mode-icon">${icon('zap', 20)}</span>
              <span class="mode-name">Fast · optional</span>
              <span class="mode-desc">Gemini with your own key · your text goes to Google</span>
            </label>
          </fieldset>
          <div class="cloud-key st-key" hidden>
            <label for="st-key">Gemini API key</label>
            <input type="password" id="st-key" placeholder="Paste your key…" autocomplete="off" spellcheck="false" />
            <p class="modal-hint">Stored only on this computer · free at aistudio.google.com</p>
          </div>
          <div class="st-save-row"><p class="st-msg" role="status"></p><button class="st-btn primary st-save">Save</button></div>
          <h3 class="st-h">Safety</h3>
          <div class="st-links">
            <span class="st-log"></span>
            <button class="st-btn st-tour" type="button">${icon('sparkles', 16)}<span>Welcome tour</span></button>
            <button class="st-btn st-sec" type="button">${icon('shield', 16)}<span>Security Center</span></button>
          </div>`;
        box.querySelector('.st-ai-tip').append(infoTip('Fast uses Google Gemini and answers in a few seconds; what you type is sent to Google. Private uses a model on this computer: nothing leaves the machine, but answers take longer. Either way, commands run only here, after you approve them.', 'About the AI modes'));
        const openApp = (app) => services.invoke('os_action', { action: 'app', op: 'launch', app, approved: false }).catch(() => {});
        box.querySelector('.st-tour').addEventListener('click', () => openApp('onboarding'));
        box.querySelector('.st-sec').addEventListener('click', () => openApp('security'));

        const modes = [...box.querySelectorAll('input[name="st-mode"]')];
        const keyBox = box.querySelector('.st-key');
        const keyInput = box.querySelector('#st-key');
        const statusList = box.querySelector('.st-status');
        const msg = box.querySelector('.st-msg');
        const save = box.querySelector('.st-save');
        let status = null;
        const chosen = () => (modes.find((m) => m.checked) || {}).value || 'local';

        const render = () => {
          const cloud = (status?.cloud_fallbacks || []).length > 0;
          keyBox.hidden = chosen() !== 'cloud' || cloud;
          const local = status?.local || {};
          // [state, short label, the full sentence on hover]
          const rows = [
            [status ? 'ok' : 'bad', 'AI core', status ? 'AI core is running' : "AI core isn't running"],
            [status?.router?.reachable ? 'ok' : 'bad', 'Instant commands', status?.router?.reachable ? 'Instant commands are ready' : "Instant commands aren't available"],
            chosen() === 'local'
              ? [local.reachable ? 'ok' : 'wait', 'On-device model', local.reachable ? 'On-device model is ready' : 'On-device model is starting (about a minute)']
              : [cloud ? 'ok' : 'wait', 'Gemini', cloud ? 'Gemini is connected' : 'Add a Gemini key to use Fast mode'],
            [status?.desktop?.sway ? 'ok' : 'wait', 'Window control', status?.desktop?.sway ? 'Window control is on' : 'Window control needs the OSAIMA desktop session'],
          ];
          statusList.replaceChildren(...rows.map(([s, label, full]) => {
            const li = el('li', s);
            li.append(iconEl(s === 'ok' ? 'check-circle' : s === 'bad' ? 'x-circle' : 'clock', 14), el('span', null, label));
            li.title = full;
            li.setAttribute('aria-label', full);
            return li;
          }));
        };
        const refresh = async (setMode) => {
          try { status = await services.invoke('ai_status'); } catch { status = null; }
          if (setMode) modes.forEach((m) => { m.checked = m.value === (status?.mode || 'local'); });
          render();
        };
        modes.forEach((m) => m.addEventListener('change', render));

        // ── This computer: graphics card + a better on-device model, if one fits ──
        const mb = (n) => (n >= 1024 ? (n / 1024).toFixed(1) + ' GB' : Math.round(n) + ' MB');
        const clock = (s) => (s == null ? '' : s >= 3600 ? `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min` : s >= 60 ? `${Math.ceil(s / 60)} min` : `${s} s`);
        const pc = el('div', 'st-pc');
        pc.append(el('h3', 'st-h', 'This computer'));
        const pcBody = el('div', 'st-pc-body');
        pcBody.append(skeleton(2));
        pc.append(pcBody);
        box.querySelector('.st-save-row').after(pc);
        let pcTimer = null;
        const stopPoll = () => { if (pcTimer) { clearInterval(pcTimer); pcTimer = null; } };
        const showProgress = (rec) => {
          const bar = el('progress', 'st-progress');
          bar.max = 100;
          const line = el('div', 'st-pc-line');
          const cancel = el('button', 'st-btn', 'Cancel');
          cancel.type = 'button';
          const out = el('div', 'st-inline st-block');
          out.setAttribute('role', 'status');
          cancel.addEventListener('click', async () => {
            cancel.disabled = true;
            try { await services.invoke('model_cancel'); } catch (e) { out.textContent = errText(e); }
          });
          pcBody.replaceChildren(el('div', 'st-row-name', 'Downloading ' + rec.name), bar, line, cancel, out);
          const tick = async () => {
            if (!box.isConnected) { stopPoll(); return; }
            let st;
            try { st = await services.invoke('model_status'); } catch (e) { out.classList.add('is-err'); out.textContent = errText(e); return; }
            if (st.state === 'downloading' || st.state === 'verifying') {
              const pct = st.total ? Math.min(100, (st.bytes / st.total) * 100) : 0;
              bar.value = pct;
              line.textContent = st.state === 'verifying' ? 'Checking the file…'
                : `${Math.floor(pct)}%  ·  ${mb(st.bytes / 1048576)} of ${mb(st.total / 1048576)}`
                  + (st.speed_bps ? `  ·  ${mb(st.speed_bps / 1048576)}/s` : '')
                  + (st.eta_s != null ? `  ·  ${clock(st.eta_s)} left` : '');
            } else {
              stopPoll();
              if (st.state === 'done') {
                pcBody.replaceChildren(el('div', 'st-row-name', `Ready. Private mode will use ${rec.name}.`),
                  el('div', 'st-row-hint', 'It takes effect the next time the on-device model starts.'));
              } else if (st.state === 'error') {
                loadPc(st.error || 'The download failed.');
              } else { loadPc(); }
            }
          };
          stopPoll();
          tick();
          pcTimer = setInterval(tick, 1000);
        };
        async function loadPc(problem) {
          let info;
          try { info = await services.invoke('gpu_info'); } catch (e) { pcBody.textContent = "Couldn't check this computer: " + errText(e); return; }
          const nodes = [];
          if (problem) { const p = el('div', 'st-inline is-err st-block', problem); nodes.push(p); }
          const g = info.gpus || [];
          const gpuList = el('div', 'st-gpus');
          if (!g.length) gpuList.append(el('div', 'st-row-name', 'No graphics card found'));
          for (const x of g) {
            const line = el('div', 'st-gpu');
            line.append(iconEl('cpu', 16), el('span', 'st-gpu-name', x.name),
              chip(x.integrated ? 'Built-in' : x.vram_mb ? `${x.vram_estimated ? '~' : ''}${mb(x.vram_mb)}` : 'GPU'));
            line.title = x.integrated ? `${x.name} (built-in, shares system memory)` : x.vram_mb ? `${x.name} (${x.vram_estimated ? 'about ' : ''}${mb(x.vram_mb)} video memory)` : x.name;
            gpuList.append(line);
          }
          nodes.push(gpuList);
          const facts = el('div', 'st-pc-facts');
          facts.append(chip(`${mb(info.ram_mb || 0)} memory`, 'memory'));
          if (info.current_model) facts.append(chip(info.current_model, 'sparkles', 'info'));
          nodes.push(facts);
          const rec = info.recommendation;
          if (rec) {
            nodes.push(el('div', 'st-pc-rec', `${rec.name} (${mb(rec.size_mb)})`), el('div', 'st-row-hint', rec.why));
            if (info.offload_note) nodes.push(el('div', 'st-row-hint', info.offload_note));
            const dl = withIcon(el('button', 'st-btn primary'), 'arrow-right', `Download (${mb(rec.size_mb)})`);
            dl.type = 'button';
            const ask = el('div', 'st-pc-ask');
            ask.hidden = true;
            const ok = el('button', 'st-btn primary', 'OK, download');
            const no = el('button', 'st-btn', 'Not now');
            ok.type = no.type = 'button';
            const note = el('span', 'st-inline', `This downloads ${mb(rec.size_mb)} from Hugging Face and uses that much disk space.`);
            ask.append(note, no, ok);
            const msg2 = el('div', 'st-inline st-block');
            msg2.setAttribute('role', 'status');
            dl.addEventListener('click', () => { dl.hidden = true; ask.hidden = false; });
            no.addEventListener('click', () => { ask.hidden = true; dl.hidden = false; });
            ok.addEventListener('click', async () => {
              ok.disabled = no.disabled = true;
              try {
                await services.invoke('model_download', { id: rec.id, approved: true });
                showProgress(rec);
              } catch (e) {
                ok.disabled = no.disabled = false;
                msg2.classList.add('is-err');
                msg2.textContent = errText(e);
              }
            });
            nodes.push(dl, ask, msg2);
          } else {
            const why = g.length && info.no_recommendation_reason ? info.no_recommendation_reason.replace(/;?\s*paste a Gemini key.*$/i, '') : '';
            const hint = el('div', 'st-pc-note');
            hint.append(el('span', null, 'No better on-device model fits · use Fast for quick answers'));
            if (why) hint.append(infoTip(why + '.', 'Why'));
            nodes.push(hint);
          }
          pcBody.replaceChildren(...nodes);
          // pick up a download that is already running (e.g. window reopened)
          try {
            const st = await services.invoke('model_status');
            if (rec && (st.state === 'downloading' || st.state === 'verifying') && st.id === rec.id) showProgress(rec);
          } catch {}
        }
        loadPc();

        save.addEventListener('click', async () => {
          save.disabled = true;
          say(msg);
          try {
            const key = keyInput.value.trim();
            if (chosen() === 'cloud' && key) {
              say(msg, 'busy', 'Checking the key…');
              await services.invoke('validate_api_key', { key });
              await services.invoke('set_api_key', { key });
              keyInput.value = '';
            }
            await services.invoke('set_llm_mode', { mode: chosen() });
            let firstStart = false;
            try { firstStart = !localStorage.getItem('osaima_welcomed'); localStorage.setItem('osaima_welcomed', '1'); } catch {}
            say(msg, 'ok', 'Saved');
            // First start: the choice is made, so get out of the way of the assistant.
            if (firstStart && services.desktopMode) {
              setTimeout(() => services.invoke('os_action', { action: 'window', op: 'close', app: 'settings' }).catch(() => {}), 600);
            }
          } catch (e) {
            say(msg, 'err', "Couldn't save: " + errText(e));
          }
          save.disabled = false;
          refresh(false);
        });

        (async () => {
          try {
            const r = await services.invoke('audit_tail', { n: 1 });
            const v = r?.verified || {};
            const c = v.ok ? chip(`Log intact · ${v.records} entries`, 'shield-check', 'ok') : chip(`Log altered at entry ${v.line}`, 'shield-alert', 'bad');
            c.title = 'Each entry carries a fingerprint of the one before it, so editing or deleting any entry would show here.';
            box.querySelector('.st-log').replaceChildren(c);
          } catch { box.querySelector('.st-log').replaceChildren(chip("Can't read the log", 'alert-circle', 'bad')); }
        })();

        refresh(true);
        const t = setInterval(() => { if (!box.isConnected) clearInterval(t); else refresh(false); }, 5000);
      }

      function appearancePage(box) {
        box.append(heading('Appearance'));
        const group = el('div', 'st-cards');
        const msg = el('p', 'st-inline st-block');
        msg.setAttribute('role', 'status');
        const cards = [];
        const mark = (id) => cards.forEach(([wid, b]) => { b.classList.toggle('on', wid === id); b.setAttribute('aria-pressed', String(wid === id)); });
        for (const w of state.wallpapers || []) {
          const b = el('button', 'st-card', w.name);
          b.type = 'button';
          b.addEventListener('click', async () => {
            const ok = await commit([...cards.map((c) => c[1]), own, apply], msg, () => setting('wallpaper', w.id));
            if (ok) { state.wallpaper = w.id; mark(w.id); }
          });
          cards.push([w.id, b]);
          group.append(b);
        }
        const own = el('input', 'st-text');
        own.type = 'text';
        own.placeholder = '~/Pictures/photo.jpg…';
        own.setAttribute('aria-label', 'Path to your own image');
        own.spellcheck = false;
        const apply = el('button', 'st-btn', 'Use');
        apply.type = 'button';
        apply.addEventListener('click', async () => {
          const path = own.value.trim();
          if (!path) { say(msg, 'err', 'Type the path of a PNG, JPEG or WebP image first.'); return; }
          const ok = await commit([...cards.map((c) => c[1]), own, apply], msg, () => setting('wallpaper', path));
          if (ok) { state.wallpaper = path; mark(null); }
        });
        mark(state.wallpaper);
        const r = row('Own image', 'A PNG, JPEG or WebP image in your home folder', own, apply);
        box.append(group, r.row, msg);
        // the row has its own status slot; share it
        r.msg.remove();
      }

      function displayPage(box) {
        box.append(heading('Display', 'If a change makes the screen hard to read, it reverts by itself after 15 seconds.'));
        const night = toggle(state.night_light);
        night.setAttribute('aria-label', 'Night light');
        const rn = row('Night light', 'Warmer colours, easier on the eyes at night', night);
        night.addEventListener('change', async () => {
          const want = night.checked;
          const ok = await commit([night], rn.msg, () => setting('night_light', want));
          if (ok) state.night_light = want; else night.checked = !want;
        });
        box.append(rn.row);
        const outs = state.outputs || [];
        if (!outs.length) box.append(emptyState('monitor', 'No screen found · needs the OSAIMA desktop session'));
        const all = [];
        const lock = (on) => all.forEach((c) => { c.disabled = on; });

        outs.forEach((o) => {
          const modes = o.modes.includes(o.current) ? o.modes : [o.current, ...o.modes];
          const sel = select(modes.map((m) => [m, m.replace('x', ' × ')]), o.current);
          all.push(sel);
          const title = [o.make, o.model].filter(Boolean).join(' ').trim();
          const r = row(`Resolution · ${o.name}`, title || undefined, sel);
          let current = o.current;
          sel.addEventListener('change', async () => {
            const next = sel.value;
            const prev = current;
            if (next === prev) return;
            lock(true);
            say(r.msg, 'busy', 'Applying…');
            try {
              await setting('resolution', next, { output: o.name }, true);
            } catch (e) {
              sel.value = prev;
              say(r.msg, 'err', errText(e));
              lock(false);
              return;
            }
            current = next;
            say(r.msg);
            askKeep('resolution', async () => { await setting('resolution', prev, { output: o.name }, true); },
              (kept) => {
                if (!kept) { current = prev; sel.value = prev; }
                say(r.msg, 'ok', kept ? 'Saved' : 'Reverted');
                lock(false);
              },
              (err) => { say(r.msg, 'err', "Couldn't revert: " + err); });
          });
          box.append(r.row);
        });

        if (outs.length) {
          const cur = Number(outs[0].scale) || 1;
          const btns = (state.scales || [1, 1.25, 1.5, 1.75, 2]).map((s) => {
            const b = el('button', 'st-seg', `${Math.round(s * 100)}%`);
            b.type = 'button';
            b.dataset.scale = String(s);
            b.setAttribute('aria-pressed', String(s === cur));
            b.classList.toggle('on', s === cur);
            return b;
          });
          all.push(...btns);
          const r = row('Scale', undefined, ...btns);
          let curScale = cur;
          const mark = (v) => btns.forEach((b) => { const on = Number(b.dataset.scale) === v; b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on)); });
          btns.forEach((b) => b.addEventListener('click', async () => {
            const next = Number(b.dataset.scale);
            const prev = curScale;
            if (next === prev) return;
            lock(true);
            say(r.msg, 'busy', 'Applying…');
            try { await setting('scale', next); } catch (e) {
              say(r.msg, 'err', errText(e)); lock(false); return;
            }
            curScale = next;
            mark(next);
            say(r.msg);
            askKeep('scale', async () => { await setting('scale', prev); },
              (kept) => { if (!kept) { curScale = prev; mark(prev); } say(r.msg, 'ok', kept ? 'Saved' : 'Reverted'); lock(false); },
              (err) => { say(r.msg, 'err', "Couldn't revert: " + err); });
          }));
          box.append(r.row);
        }
      }

      async function soundPage(box) {
        box.append(heading('Sound'));
        let vol = { percent: 0, muted: false };
        try { vol = await os('volume', 'get'); } catch (e) { box.append(chip(errText(e), 'alert-circle', 'bad', 'st-block')); }
        const level = Math.max(0, Math.min(100, Number(vol.percent) || 0));
        const sl = slider(0, 100, 1, level);
        sl.setAttribute('aria-label', 'Volume');
        const num = el('span', 'st-value', `${level}%`);
        const mute = toggle(vol.muted);
        mute.setAttribute('aria-label', 'Mute');
        const rv = row('Volume', undefined, sl, num);
        const rm = row('Mute', undefined, mute);
        sl.addEventListener('input', () => { num.textContent = `${sl.value}%`; });
        sl.addEventListener('change', () => commit([sl, mute], rv.msg, () => os('volume', 'set', { value: Number(sl.value) })));
        mute.addEventListener('change', async () => {
          const want = mute.checked;
          const ok = await commit([sl, mute], rm.msg, () => os('volume', want ? 'mute' : 'unmute'));
          if (!ok) mute.checked = !want;
        });
        box.append(rv.row, rm.row);
        const sinks = state.sinks || [];
        if (sinks.length) {
          const cur = (sinks.find((s) => s.default) || sinks[0]).id;
          const sel = select(sinks.map((s) => [s.id, s.name]), cur);
          const r = row('Output', 'Where sound plays', sel);
          let prev = String(cur);
          sel.addEventListener('change', async () => {
            const ok = await commit([sel, sl, mute], r.msg, () => setting('sound_output', sel.value));
            if (!ok) { sel.value = prev; return; }
            prev = sel.value;
            try {
              const v = await os('volume', 'get');
              sl.value = Number(v.percent) || 0; num.textContent = `${sl.value}%`; mute.checked = !!v.muted;
            } catch {}
          });
          box.append(r.row);
        } else {
          box.append(emptyState('volume', 'No sound output found'));
        }
      }

      async function networkPage(box) {
        box.append(heading('Network'));
        const net = state.network || {};
        const card = el('div', 'st-net ' + (net.online ? 'ok' : 'bad'));
        const netIcon = el('span', 'st-net-icon');
        netIcon.append(iconEl(net.online ? 'wifi' : 'wifi-off', 22));
        const netText = el('div', 'st-net-text');
        netText.append(el('span', 'st-net-name', net.online ? (net.wifi || 'Connected') : 'Offline'),
          el('span', 'st-net-sub', net.online ? [net.interface, net.address].filter(Boolean).join(' · ') : 'Not connected to a network'));
        card.append(netIcon, netText);
        box.append(card);
        let wifi = null;
        try { wifi = await os('wifi', 'get'); } catch (e) { box.append(emptyState('wifi-off', "Wi-Fi can't be controlled here"), chip(errText(e), 'info', 'muted', 'st-block')); return; }
        const sw = toggle(wifi.wifi_enabled);
        sw.setAttribute('aria-label', 'Wi-Fi');
        const r = row('Wi-Fi', 'Turning it off can disconnect you', sw);
        const confirm = el('div', 'st-confirm');
        confirm.hidden = true;
        const yes = el('button', 'st-btn danger', 'Turn off');
        const no = el('button', 'st-btn', 'Cancel');
        yes.type = no.type = 'button';
        confirm.append(el('span', '', 'Turn Wi-Fi off? You may lose your connection.'), no, yes);
        confirm.prepend(iconEl('alert', 16));
        const set = async (on) => {
          confirm.hidden = true;
          const ok = await commit([sw, yes, no], r.msg, () => os('wifi', on ? 'on' : 'off', {}, !on));
          if (!ok) sw.checked = !on;
        };
        sw.addEventListener('change', () => {
          if (sw.checked) { set(true); return; }
          say(r.msg);
          confirm.hidden = false;
          yes.focus();
        });
        yes.addEventListener('click', () => set(false));
        no.addEventListener('click', () => { confirm.hidden = true; sw.checked = true; });
        box.append(r.row, confirm);
      }

      function inputPage(box) {
        box.append(heading('Keyboard & mouse'));
        const layouts = state.layouts || [];
        const lsel = select(layouts.map((l) => [l.id, l.name]), state.layout);
        const rl = row('Keyboard layout', undefined, lsel);
        let prevLayout = lsel.value;
        lsel.addEventListener('change', async () => {
          const ok = await commit([lsel], rl.msg, () => setting('keyboard', lsel.value));
          if (ok) prevLayout = lsel.value; else lsel.value = prevLayout;
        });

        const rate = slider(10, 60, 1, state.repeat_rate);
        rate.setAttribute('aria-label', 'Key repeat rate');
        const rateNum = el('span', 'st-value', `${state.repeat_rate}/s`);
        const rr = row('Key repeat', 'Characters per second when a key is held', rate, rateNum);
        rate.addEventListener('input', () => { rateNum.textContent = `${rate.value}/s`; });
        rate.addEventListener('change', () => commit([rate], rr.msg, () => setting('repeat_rate', Number(rate.value))));

        const flag = (op, label, hint, on) => {
          const sw = toggle(on);
          sw.setAttribute('aria-label', label);
          const r = row(label, hint, sw);
          sw.addEventListener('change', async () => {
            const want = sw.checked;
            const ok = await commit([sw], r.msg, () => setting(op, want));
            if (!ok) sw.checked = !want;
          });
          return r.row;
        };

        const speed = slider(-1, 1, 0.1, state.pointer_speed);
        speed.setAttribute('aria-label', 'Pointer speed');
        const speedNum = el('span', 'st-value', Number(state.pointer_speed).toFixed(1));
        const rs = row('Pointer speed', undefined, speed, speedNum);
        speed.addEventListener('input', () => { speedNum.textContent = Number(speed.value).toFixed(1); });
        speed.addEventListener('change', () => commit([speed], rs.msg, () => setting('pointer_speed', Number(speed.value))));

        box.append(rl.row, rr.row, rs.row,
          flag('tap', 'Tap to click', 'Touchpad', state.tap),
          flag('natural_scroll', 'Natural scrolling', 'Touchpad: the content follows your fingers', state.natural_scroll));
      }

      function notificationsPage(box) {
        box.append(heading('Notifications'));
        const sw = toggle(state.dnd);
        sw.setAttribute('aria-label', 'Do Not Disturb');
        const r = row('Do Not Disturb', 'Hide pop-up notifications until you turn this off', sw);
        sw.addEventListener('change', async () => {
          const want = sw.checked;
          const ok = await commit([sw], r.msg, () => setting('dnd', want));
          if (!ok) sw.checked = !want;
        });
        box.append(r.row);
      }

      function automationsPage(box) {
        box.append(heading('Automations', 'Things OSAIMA does on a schedule or when something happens. Ask the assistant to make one, e.g. "keep brightness at 40% on weekdays from 9 pm to 6 am".'));
        const list0 = skeleton(3);
        const list = el('div', 'st-auto-list');
        list.append(list0);
        box.append(list);
        const load = async () => {
          let d;
          try { d = await services.invoke('automations', {}); } catch (e) { list.replaceChildren(emptyState('alert-circle', errText(e))); return; }
          list.replaceChildren();
          for (const p of d.pending || []) {
            const stop = el('button', 'st-btn primary', 'Cancel');
            stop.type = 'button';
            const r = row('About to run', `In ${p.runs_in} seconds`, stop);
            stop.addEventListener('click', () => commit([stop], r.msg, () => services.invoke('automation_cancel', {})).then(load));
            list.append(r.row);
          }
          const rules = d.automations || [];
          if (!rules.length && !(d.pending || []).length) list.append(emptyState('zap', 'No automations yet · ask the assistant'));
          for (const a of rules) {
            const del = el('button', 'st-btn', 'Remove');
            del.type = 'button';
            const state = a.active ? 'On now' : a.enabled === false ? 'Finished' : 'Waiting';
            const r = row(a.summary || a.id, state, del);
            del.addEventListener('click', () => commit([del], r.msg, () => services.invoke('automation_remove', { id: a.id })).then(load));
            list.append(r.row);
          }
        };
        return load();
      }

      function lockPage(box) {
        box.append(heading('Screen lock'));
        const choices = [[0, 'Never'], [1, '1 minute'], [2, '2 minutes'], [5, '5 minutes'], [10, '10 minutes'], [15, '15 minutes'], [30, '30 minutes'], [60, '1 hour']];
        const cur = Number(state.lock_minutes);
        if (!choices.some(([m]) => m === cur)) choices.push([cur, `${cur} minutes`]);
        choices.sort((a, b) => (a[0] || Infinity) - (b[0] || Infinity));
        const sel = select(choices, cur);
        const r = row('Lock after', 'How long the computer is idle before it locks', sel);
        let prev = sel.value;
        sel.addEventListener('change', async () => {
          const ok = await commit([sel], r.msg, () => setting('lock_minutes', Number(sel.value)));
          if (ok) prev = sel.value; else sel.value = prev;
        });
        const lockBtn = withIcon(el('button', 'st-btn'), 'lock', 'Lock now');
        lockBtn.type = 'button';
        const rn = row('Lock the screen', 'You will need your password to come back', lockBtn);
        lockBtn.addEventListener('click', () => commit([lockBtn], rn.msg, () => os('lock_screen', 'lock')).then(() => say(rn.msg)));
        box.append(r.row, rn.row);
      }

      function defaultsPage(box) {
        box.append(heading('Default apps', 'Which app opens each kind of file.'));
        const names = { browser: 'Web browser', files: 'Folders', pdf: 'PDF documents', images: 'Pictures', video: 'Video and music', text: 'Text files' };
        for (const d of state.default_apps || []) {
          // the same app can be listed under two desktop entries: show it once
          const seen = new Set();
          const opts = [...d.options].sort((a, b) => (b.id === d.current) - (a.id === d.current))
            .filter((o) => (seen.has(o.name) ? false : (seen.add(o.name), true)));
          const match = d.options.find((o) => o.id === d.current);
          const chosen = match ? opts.find((o) => o.name === match.name) : null;
          const sel = select(opts.map((o) => [o.id, o.name]), chosen ? chosen.id : '');
          if (!chosen) {
            const none = el('option', '', d.current ? 'Another app' : 'Not set');
            none.value = '';
            sel.prepend(none);
            sel.value = '';
          }
          const r = row(names[d.kind] || d.kind, undefined, sel);
          let prev = sel.value;
          sel.addEventListener('change', async () => {
            if (!sel.value) { sel.value = prev; return; }
            const ok = await commit([sel], r.msg, () => setting('default_app', sel.value, { kind: d.kind }));
            if (ok) prev = sel.value; else sel.value = prev;
          });
          box.append(r.row);
        }
      }

      function timePage(box) {
        box.append(heading('Date & time'));
        const input = el('input', 'st-text');
        input.type = 'text';
        input.value = state.timezone || '';
        input.placeholder = 'Asia/Kolkata';
        input.setAttribute('aria-label', 'Time zone');
        input.spellcheck = false;
        input.setAttribute('list', 'st-zones');
        const dl = el('datalist');
        dl.id = 'st-zones';
        try { for (const z of Intl.supportedValuesOf('timeZone')) { const o = el('option'); o.value = z; dl.append(o); } } catch {}
        const apply = el('button', 'st-btn', 'Change');
        apply.type = 'button';
        const r = row('Time zone', 'Region/City, for example Asia/Kolkata', input, apply, dl);
        const now = el('div', 'st-now');
        const tick = () => {
          const d = new Date();
          now.replaceChildren(el('span', 'st-now-time', d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })),
            el('span', 'st-now-date', d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })));
        };
        tick();
        const t = setInterval(() => { if (!now.isConnected) clearInterval(t); else tick(); }, 15000);
        let prev = input.value;
        const go = async () => {
          const zone = input.value.trim();
          if (!zone) { say(r.msg, 'err', 'Type a time zone, like Asia/Kolkata.'); return; }
          const ok = await commit([input, apply], r.msg, () => setting('timezone', zone, {}, true));
          if (ok) { prev = zone; tick(); } else input.value = prev;
        };
        apply.addEventListener('click', go);
        input.addEventListener('keydown', (e) => { if (e.key === 'Enter') go(); });
        box.append(now, r.row);
      }

      const PAGES = [
        ['ai', 'AI', aiPage, false, 'sparkles'],
        ['appearance', 'Appearance', appearancePage, true, 'image'],
        ['display', 'Display', displayPage, true, 'monitor'],
        ['sound', 'Sound', soundPage, true, 'volume'],
        ['network', 'Network', networkPage, true, 'wifi'],
        ['input', 'Keyboard & mouse', inputPage, true, 'keyboard'],
        ['notifications', 'Notifications', notificationsPage, true, 'bell'],
        ['lock', 'Screen lock', lockPage, true, 'lock'],
        ['automations', 'Automations', automationsPage, false, 'zap'],
        ['defaults', 'Default apps', defaultsPage, true, 'grid'],
        ['time', 'Date & time', timePage, true, 'clock'],
      ];
      const navButtons = new Map();
      let shownToken = 0;
      async function show(id) {
        const [, , render, needsState] = PAGES.find((p) => p[0] === id);
        const mine = ++shownToken;
        navButtons.forEach((b, k) => { b.classList.toggle('on', k === id); b.setAttribute('aria-current', k === id ? 'page' : 'false'); });
        const box = el('div', 'st-box');
        page.replaceChildren(box);
        if (needsState) {
          box.append(skeleton(4, 'st-skel'));
          try { await loadState(); } catch (e) {
            if (mine !== shownToken) return;
            box.replaceChildren(emptyState('alert-circle', "Can't read the system settings: " + errText(e)));
            return;
          }
          if (mine !== shownToken) return;
          box.replaceChildren();
        }
        await render(box);
        page.scrollTop = 0;
        root.scrollTop = 0;
      }
      for (const [id, label, , , ic] of PAGES) {
        const b = withIcon(el('button', 'st-nav-item'), ic, label);
        b.type = 'button';
        b.addEventListener('click', () => show(id));
        navButtons.set(id, b);
        nav.append(b);
      }
      show('ai');
    },
  };
}
