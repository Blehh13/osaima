/**
 * voice-ui.js: push-to-talk for the assistant window.
 *
 * The voice service (osaima-voice) records, transcribes and speaks; this just
 * starts and stops it and reads the result back. It has no DOM code, so it can be
 * tested on its own. The service pushes nothing: while a recording is being
 * listened to or transcribed, `status()` is polled until the service is idle, and
 * a new `last.seq` means there is a new transcript (`last.text`) or failure
 * (`last.error`).
 *
 *   const voice = createVoice({ invoke, onText, onNotice, onChange, alive });
 *   await voice.refresh();      // what works; call again if the service was down
 *   voice.toggle();             // start listening, or stop and transcribe
 *   voice.speak('Hello');       // only when reading replies aloud is on
 */

export const POLL_MS = 400;

/**
 * @param {object} options
 * @param {(cmd: string, args?: object) => Promise<any>} options.invoke   voice_status, voice_listen, ...
 * @param {(text: string) => void} options.onText      the words that were heard
 * @param {(message: string) => void} [options.onNotice]  something the user should read
 * @param {() => void} [options.onChange]              state or abilities changed (re-render buttons)
 * @param {() => boolean} [options.alive]              false once the window is gone: polling stops
 * @param {number} [options.pollMs]
 */
export function createVoice({ invoke, onText, onNotice = () => {}, onChange = () => {}, alive = () => true, pollMs = POLL_MS }) {
  const v = {
    state: 'idle', // idle | listening | transcribing | speaking
    canListen: false,
    canSpeak: false,
    speakReplies: false,
    why: '', // the service's first problem, for a tooltip
  };
  let lastSeq = 0;
  let initialised = false;
  let timer = null;
  let polling = false;

  const message = (err) => (typeof err === 'string' ? err : err?.message ?? String(err));

  function apply(status) {
    v.state = status.state ?? 'idle';
    v.canListen = Boolean(status.can_listen);
    v.canSpeak = Boolean(status.can_speak);
    v.why = status.problems?.[0] ?? '';
    if (!initialised) {
      initialised = true;
      lastSeq = status.last?.seq ?? 0; // anything older was not for this window
      v.speakReplies = v.canSpeak && Boolean(status.speak_replies);
    }
    if (!v.canSpeak) v.speakReplies = false;
  }

  /** Read what works. A service that is not running just means voice is off. */
  async function refresh() {
    try {
      apply(await invoke('voice_status'));
    } catch (err) {
      v.state = 'idle';
      v.canListen = false;
      v.canSpeak = false;
      v.speakReplies = false;
      v.why = message(err);
    }
    onChange();
  }

  function schedule() {
    if (timer || !alive()) return;
    timer = setTimeout(poll, pollMs);
  }

  async function poll() {
    timer = null;
    if (!alive() || polling) return;
    polling = true;
    try {
      const status = await invoke('voice_status');
      apply(status);
      const last = status.last;
      if (last && last.seq > lastSeq) {
        lastSeq = last.seq;
        if (last.text) onText(last.text);
        else if (last.error) onNotice(last.error);
      }
      onChange();
      if (v.state !== 'idle') schedule();
    } catch (err) {
      v.state = 'idle';
      onNotice(message(err));
      onChange();
    } finally {
      polling = false;
    }
  }

  /** Start listening, or (when already listening) stop and have it transcribed. */
  async function toggle() {
    if (!v.canListen) return;
    try {
      if (v.state === 'listening') {
        await invoke('voice_stop');
        v.state = 'transcribing';
      } else {
        await invoke('voice_listen');
        v.state = 'listening';
      }
      onChange();
      schedule();
    } catch (err) {
      onNotice(message(err));
      await refresh();
    }
  }

  function setSpeakReplies(on) {
    v.speakReplies = v.canSpeak && Boolean(on);
    if (!v.speakReplies && v.state === 'speaking') silence();
    onChange();
  }

  /** Read `text` aloud, if the user turned that on. Failures are not worth interrupting for. */
  function speak(text) {
    if (!v.speakReplies || !text) return;
    invoke('voice_speak', { text: String(text) })
      .then(() => {
        v.state = 'speaking';
        onChange();
        schedule();
      })
      .catch(() => {});
  }

  function silence() {
    if (v.state === 'speaking') invoke('voice_silence').catch(() => {});
  }

  /** The window is closing: never leave the microphone open. */
  function dispose() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (v.state === 'listening') invoke('voice_cancel').catch(() => {});
    silence();
  }

  return { view: v, refresh, toggle, speak, silence, setSpeakReplies, dispose };
}
