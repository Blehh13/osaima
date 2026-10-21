/**
 * bridge.js — one way to call the backend from any OSAIMA page.
 *
 * Inside the Tauri app it uses Tauri's invoke; in a plain browser (dev on
 * :8777) it posts to server.py's /api/invoke. Both reject on refusal, so a
 * refused action is never mistaken for success.
 */
// Tauri's invoke only for pages bundled in the app; a page loaded from the dev
// server (OSAIMA_DEV_URL) is remote to Tauri, so it talks to server.py.
function tauriInvoke() {
  const bundled = location.protocol === 'tauri:' || location.hostname === 'tauri.localhost';
  const t = window.__TAURI__;
  return bundled && t && (t.core?.invoke || t.invoke || t.tauri?.invoke);
}

export async function invoke(cmd, args = {}) {
  const native = tauriInvoke();
  // Tauri commands take named arguments; os_action takes the whole request,
  // and the task_* calls share one whitelisted command
  if (native) {
    if (cmd === 'tasks' || cmd.startsWith('task_') || cmd.startsWith('automation') || cmd === 'gpu_info' || cmd.startsWith('model_')) return native('task_rpc', { method: 'system.' + cmd, params: args });
    // voice_status, voice_listen, ... go to the voice service as voice.status, voice.listen, ...
    if (cmd.startsWith('voice_')) return native('voice_call', { method: 'voice.' + cmd.slice(6), params: args });
    return native(cmd, cmd === 'os_action' ? { request: args } : args);
  }
  const resp = await fetch('/api/invoke', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ cmd, args }),
  });
  const data = await resp.json().catch(() => null);
  if (!data) throw new Error(`${cmd}: no answer from the shell server`);
  if (data.ok === false) throw new Error(data.error || `${cmd} failed`);
  return data.result;
}
