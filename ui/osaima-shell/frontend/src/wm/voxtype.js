/**
 * voxtype.js — Ambient Voice-to-Text Typing Engine for OSAIMA
 *
 * Trigger: Press F9 (or Alt+v / Status Bar button).
 * Behavior:
 *   1. Captures live audio stream and speech recognition in real time.
 *   2. Displays an ambient floating HUD with live waveforms and speech preview.
 *   3. Automatically converts speech to text.
 *   4. Inserts the converted text directly into any selected/focused input, textarea,
 *      contenteditable element, or terminal window.
 *   5. If no input is focused, routes to the AI Command Hub or copies to clipboard.
 *   6. Dispatches to host desktop via xdotool (/api/invoke voxtype_type) for system-wide typing.
 */

export class VoxType {
  constructor(options = {}) {
    this.services = options.services || {};
    this.engine = options.engine || null;
    this.isListening = false;
    this.recognition = null;
    this.audioCtx = null;
    this.analyser = null;
    this.mediaStream = null;
    this.animFrameId = null;
    this.lastTarget = null;
    this.lastTargetInfo = 'None';
    this.f9DownTime = 0;
    this.transcript = '';
    this.interimText = '';

    this._initDom();
    this._initListeners();
    this._initSpeechRecognition();
  }

  // ── 1. Create Floating HUD and Status Elements ──────────────────────────────
  _initDom() {
    // Floating HUD Capsule
    if (!document.getElementById('voxtype-hud')) {
      const hud = document.createElement('div');
      hud.id = 'voxtype-hud';
      hud.className = 'voxtype-hud hidden';
      hud.innerHTML = `
        <div class="voxtype-hud-inner glass-panel">
          <div class="voxtype-top">
            <div class="voxtype-orb-wrap">
              <div class="voxtype-pulse-ring"></div>
              <div class="voxtype-orb">
                <span class="voxtype-icon">🎙</span>
              </div>
            </div>
            <div class="voxtype-info">
              <div class="voxtype-header">
                <span class="voxtype-brand">VOXTYPE</span>
                <span class="voxtype-status-badge" id="voxtype-badge">READY</span>
              </div>
              <div class="voxtype-target-label" id="voxtype-target">Target: None (Press F9)</div>
            </div>
            <button class="voxtype-close-btn" id="voxtype-close" title="Close">✕</button>
          </div>

          <div class="voxtype-wave-container">
            <div class="voxtype-bar" style="--i:1"></div>
            <div class="voxtype-bar" style="--i:2"></div>
            <div class="voxtype-bar" style="--i:3"></div>
            <div class="voxtype-bar" style="--i:4"></div>
            <div class="voxtype-bar" style="--i:5"></div>
            <div class="voxtype-bar" style="--i:6"></div>
            <div class="voxtype-bar" style="--i:7"></div>
            <div class="voxtype-bar" style="--i:8"></div>
          </div>

          <div class="voxtype-transcript-box" id="voxtype-transcript">
            <span class="voxtype-placeholder">Press <kbd>F9</kbd> and speak to type…</span>
          </div>

          <div class="voxtype-footer">
            <span class="voxtype-tip"><kbd>F9</kbd> Toggle / Hold · Speaks text into active cursor</span>
            <div class="voxtype-actions">
              <button class="voxtype-action-btn" id="voxtype-btn-copy" title="Copy to clipboard">📋 Copy</button>
              <button class="voxtype-action-btn" id="voxtype-btn-insert" title="Force insert into current window">↵ Insert</button>
            </div>
          </div>
        </div>
      `;
      document.body.appendChild(hud);

      document.getElementById('voxtype-close').addEventListener('click', () => this.stop(false));
      document.getElementById('voxtype-btn-copy').addEventListener('click', () => {
        if (this.transcript) {
          navigator.clipboard.writeText(this.transcript);
          this._updateBadge('COPIED ✓', 'success');
        }
      });
      document.getElementById('voxtype-btn-insert').addEventListener('click', () => {
        if (this.transcript) {
          this.insertText(this.transcript);
          this._updateBadge('INSERTED ✓', 'success');
        }
      });
    }

    // Status bar button injection
    const statusRight = document.querySelector('.status-right');
    if (statusRight && !document.getElementById('voxtype-toggle')) {
      const btn = document.createElement('button');
      btn.id = 'voxtype-toggle';
      btn.className = 'status-pill voxtype-pill';
      btn.title = 'VoxType Voice-to-Text [F9]';
      btn.innerHTML = `
        <span class="vt-dot"></span>
        <span class="vt-icon">🎙</span>
        <span class="vt-text">VoxType</span>
        <span class="vt-key">F9</span>
      `;
      btn.addEventListener('click', () => this.toggle());
      statusRight.insertBefore(btn, statusRight.firstChild);
    }
  }

  // ── 2. Target Tracking and Key Listeners ────────────────────────────────────
  _initListeners() {
    // Continuously remember what editable element had focus
    const trackFocus = (e) => {
      const el = e.target;
      if (!el || el.closest('#voxtype-hud')) return;

      if (this._isEditable(el)) {
        this.lastTarget = el;
        this.lastTargetInfo = this._describeTarget(el);
        this._updateTargetDisplay();
      }
    };

    document.addEventListener('focusin', trackFocus, true);
    document.addEventListener('click', trackFocus, true);

    // Global F9 key handling
    window.addEventListener('keydown', (e) => {
      if (e.key === 'F9' || e.code === 'F9') {
        e.preventDefault();
        e.stopPropagation();

        if (e.repeat) return; // Ignore auto-repeat
        this.f9DownTime = Date.now();

        if (!this.isListening) {
          this.start();
        } else {
          this.stop(true);
        }
      }
    }, true);

    window.addEventListener('keyup', (e) => {
      if (e.key === 'F9' || e.code === 'F9') {
        const heldDuration = Date.now() - this.f9DownTime;
        // If held down for > 450ms, act as Push-to-Talk (stop on release)
        if (heldDuration > 450 && this.isListening) {
          this.stop(true);
        }
      }
    }, true);
  }

  // ── 3. Speech Recognition Setup ────────────────────────────────────────────
  _initSpeechRecognition() {
    const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!SpeechRecognition) {
      console.warn('[VoxType] SpeechRecognition API not supported in this browser.');
      return;
    }

    try {
      this.recognition = new SpeechRecognition();
      this.recognition.continuous = true;
      this.recognition.interimResults = true;
      this.recognition.lang = navigator.language || 'en-US';

      this.recognition.onstart = () => {
        this.isListening = true;
        this._updateBadge('LISTENING', 'active');
        this._setUIActive(true);
        this._playTone(440, 880, 0.12);
      };

      this.recognition.onresult = (event) => {
        let interim = '';
        let finalized = '';

        for (let i = event.resultIndex; i < event.results.length; ++i) {
          const res = event.results[i];
          if (res.isFinal) {
            finalized += res[0].transcript;
          } else {
            interim += res[0].transcript;
          }
        }

        if (finalized) {
          const cleanText = finalized.trim();
          this.transcript = (this.transcript ? this.transcript + ' ' : '') + cleanText;
          this.interimText = '';
          this._displayTranscript(this.transcript);

          // Locally implement text into selected target right away!
          this.insertText(cleanText + ' ');
        } else if (interim) {
          this.interimText = interim;
          this._displayTranscript(this.transcript + ' ' + `<span class="vt-interim">${interim}</span>`);
        }
      };

      this.recognition.onerror = (event) => {
        console.warn('[VoxType] Speech recognition error:', event.error);
        if (event.error === 'not-allowed') {
          this._updateBadge('MIC BLOCKED', 'err');
          this._displayTranscript('<span style="color:#ff6b6b;">Microphone access blocked. Please allow mic in browser.</span>');
        } else if (event.error !== 'no-speech') {
          this._updateBadge('ERROR: ' + event.error, 'err');
        }
      };

      this.recognition.onend = () => {
        if (this.isListening) {
          // Restart if still marked as listening (browser auto-stopped on silence)
          try { this.recognition.start(); } catch {}
        } else {
          this._setUIActive(false);
        }
      };
    } catch (err) {
      console.error('[VoxType] Failed to initialize SpeechRecognition:', err);
    }
  }

  // ── 4. Audio Visualizer (Live Waveforms) ───────────────────────────────────
  async _startAudioVisualizer() {
    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) return;
      this.mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true });
      this.audioCtx = new (window.AudioContext || window.webkitAudioContext)();
      const source = this.audioCtx.createMediaStreamSource(this.mediaStream);
      this.analyser = this.audioCtx.createAnalyser();
      this.analyser.fftSize = 32;
      source.connect(this.analyser);

      const bufferLength = this.analyser.frequencyBinCount;
      const dataArray = new Uint8Array(bufferLength);
      const bars = document.querySelectorAll('.voxtype-bar');

      const draw = () => {
        if (!this.isListening) return;
        this.analyser.getByteFrequencyData(dataArray);
        
        let avg = 0;
        for (let i = 0; i < bufferLength; i++) avg += dataArray[i];
        avg /= bufferLength;

        bars.forEach((bar, idx) => {
          const val = dataArray[idx % bufferLength] || avg;
          const scale = Math.max(0.15, Math.min(1.0, val / 140));
          bar.style.transform = `scaleY(${scale})`;
        });

        this.animFrameId = requestAnimationFrame(draw);
      };

      draw();
    } catch (e) {
      // Audio stream optional; recognition still runs
    }
  }

  _stopAudioVisualizer() {
    if (this.animFrameId) cancelAnimationFrame(this.animFrameId);
    if (this.mediaStream) {
      this.mediaStream.getTracks().forEach((t) => t.stop());
      this.mediaStream = null;
    }
    if (this.audioCtx && this.audioCtx.state !== 'closed') {
      this.audioCtx.close().catch(() => {});
      this.audioCtx = null;
    }
    const bars = document.querySelectorAll('.voxtype-bar');
    bars.forEach((b) => (b.style.transform = 'scaleY(0.2)'));
  }

  // ── 5. Core Control: Start, Stop, Toggle ───────────────────────────────────
  start() {
    if (this.isListening) return;
    this.transcript = '';
    this.interimText = '';

    // Snapshot current active target before opening HUD
    this._snapshotActiveTarget();
    this._showHUD();

    if (this.recognition) {
      try {
        this.recognition.start();
      } catch (e) {
        // May already be active
      }
    } else {
      this._updateBadge('FALLBACK MODE', 'warn');
      this._displayTranscript('SpeechRecognition not supported in browser. Use keyboard or Chromium.');
    }

    this._startAudioVisualizer();
  }

  stop(insertFinal = true) {
    if (!this.isListening) return;
    this.isListening = false;

    this._playTone(880, 440, 0.1);
    if (this.recognition) {
      try { this.recognition.stop(); } catch {}
    }
    this._stopAudioVisualizer();
    this._setUIActive(false);

    if (this.transcript) {
      this._updateBadge('DONE ✓', 'success');
      if (insertFinal && this.lastTarget) {
        this.insertText(''); // Ensure focus retained
      }
      setTimeout(() => this._hideHUD(), 2200);
    } else {
      this._updateBadge('CANCELLED', 'muted');
      setTimeout(() => this._hideHUD(), 800);
    }
  }

  toggle() {
    if (this.isListening) this.stop(true);
    else this.start();
  }

  // ── 6. Text Insertion Engine ("Locally implements text if any place selected") ──
  insertText(text) {
    if (!text) return;

    // 1. Check direct active element or snapshotted target
    let target = document.activeElement;
    if (!this._isEditable(target)) {
      target = this.lastTarget;
    }

    // 2. Check if a window in Window Manager is focused and has an input/terminal
    if (!this._isEditable(target) && window.WM) {
      const win = window.WM.focusedWindow;
      if (win && win.el) {
        // Check for terminal first
        const termContainer = win.el.querySelector('.term-container');
        if (termContainer && termContainer._insertText) {
          termContainer._insertText(text);
          this._notifyHost(text);
          return true;
        }

        const inputInside = win.el.querySelector('input:not([type=range]), textarea');
        if (inputInside) {
          target = inputInside;
        }
      }
    }

    // 3. If target is an input or textarea: insert at cursor / selection
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
      target.focus();
      const start = target.selectionStart !== null ? target.selectionStart : target.value.length;
      const end = target.selectionEnd !== null ? target.selectionEnd : target.value.length;
      const oldVal = target.value;

      target.value = oldVal.substring(0, start) + text + oldVal.substring(end);
      const newPos = start + text.length;
      target.selectionStart = target.selectionEnd = newPos;

      // Dispatch input events so app reacts (React/Vue/vanilla listeners)
      target.dispatchEvent(new Event('input', { bubbles: true }));
      target.dispatchEvent(new Event('change', { bubbles: true }));

      this._notifyHost(text);
      return true;
    }

    // 4. If target is contentEditable
    if (target && target.isContentEditable) {
      target.focus();
      document.execCommand('insertText', false, text);
      this._notifyHost(text);
      return true;
    }

    // 5. If terminal container has focus
    if (target && target.closest && target.closest('.term-container')) {
      const termContainer = target.closest('.term-container');
      if (termContainer._insertText) {
        termContainer._insertText(text);
        this._notifyHost(text);
        return true;
      }
    }

    // 6. Check if AI Command Hub Launcher is visible
    const launcher = document.getElementById('launcher');
    const launcherInput = document.getElementById('launcher-input');
    if (launcher && !launcher.classList.contains('hidden') && launcherInput) {
      launcherInput.focus();
      launcherInput.value += text;
      launcherInput.dispatchEvent(new Event('input', { bubbles: true }));
      this._notifyHost(text);
      return true;
    }

    // 7. Fallback: If no place selected, put into AI Hub or copy to clipboard
    if (window.openLauncher && launcherInput) {
      window.openLauncher();
      launcherInput.value = text;
      launcherInput.focus();
      this._notifyHost(text);
      return true;
    }

    // System clipboard fallback
    navigator.clipboard.writeText(text).catch(() => {});
    this._notifyHost(text);
    return false;
  }

  // ── 7. Host Integration (/api/invoke -> xdotool) ──────────────────────────
  async _notifyHost(text) {
    if (!text || !this.services.invoke) return;
    try {
      await this.services.invoke('voxtype_type', { text: text });
    } catch (e) {
      // Backend optional
    }
  }

  // ── 8. Helpers & UI Updates ───────────────────────────────────────────────
  _isEditable(el) {
    if (!el || el.disabled || el.readOnly) return false;
    const tag = el.tagName;
    if (tag === 'INPUT' && el.type !== 'button' && el.type !== 'submit' && el.type !== 'range') return true;
    if (tag === 'TEXTAREA') return true;
    if (el.isContentEditable) return true;
    if (el.classList && el.classList.contains('term-container')) return true;
    return false;
  }

  _describeTarget(el) {
    if (!el) return 'None (Global)';
    if (el.id === 'launcher-input') return 'AI Command Hub';
    if (el.classList.contains('as-input')) return 'AI Assistant';
    if (el.classList.contains('cfg-editor')) return 'wm.lua Config Editor';
    if (el.classList.contains('rag-input')) return 'Knowledge Base Query';
    if (el.classList.contains('br-search')) return 'Browser Address Bar';
    if (el.closest && el.closest('.term-container')) return 'Terminal Command Line';
    return `${el.tagName.toLowerCase()}${el.placeholder ? ' ("' + el.placeholder.slice(0, 16) + '…")' : ''}`;
  }

  _snapshotActiveTarget() {
    const active = document.activeElement;
    if (this._isEditable(active)) {
      this.lastTarget = active;
      this.lastTargetInfo = this._describeTarget(active);
    } else {
      // Check active window
      if (window.WM && window.WM.focusedWindow) {
        const win = window.WM.focusedWindow;
        this.lastTargetInfo = `Window [${win.app?.title || win.id}]`;
      } else {
        this.lastTargetInfo = 'Global (will route to AI Launcher)';
      }
    }
    this._updateTargetDisplay();
  }

  _updateTargetDisplay() {
    const el = document.getElementById('voxtype-target');
    if (el) el.textContent = `Target: ${this.lastTargetInfo}`;
  }

  _displayTranscript(html) {
    const el = document.getElementById('voxtype-transcript');
    if (el) el.innerHTML = html || '<span class="voxtype-placeholder">Listening for speech…</span>';
  }

  _updateBadge(text, state = 'active') {
    const badge = document.getElementById('voxtype-badge');
    if (badge) {
      badge.textContent = text;
      badge.className = `voxtype-status-badge ${state}`;
    }
  }

  _setUIActive(active) {
    const pill = document.getElementById('voxtype-toggle');
    if (pill) {
      if (active) pill.classList.add('active', 'recording');
      else pill.classList.remove('active', 'recording');
    }

    const hud = document.getElementById('voxtype-hud');
    if (hud) {
      if (active) hud.classList.add('recording');
      else hud.classList.remove('recording');
    }
  }

  _showHUD() {
    const hud = document.getElementById('voxtype-hud');
    if (hud) {
      hud.classList.remove('hidden');
      this._updateTargetDisplay();
      this._displayTranscript(this.transcript);
    }
  }

  _hideHUD() {
    const hud = document.getElementById('voxtype-hud');
    if (hud && !this.isListening) hud.classList.add('hidden');
  }

  _playTone(freqStart, freqEnd, duration = 0.1) {
    try {
      const ctx = new (window.AudioContext || window.webkitAudioContext)();
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = 'sine';
      osc.frequency.setValueAtTime(freqStart, ctx.currentTime);
      osc.frequency.exponentialRampToValueAtTime(freqEnd, ctx.currentTime + duration);
      gain.gain.setValueAtTime(0.08, ctx.currentTime);
      gain.gain.linearRampToValueAtTime(0.001, ctx.currentTime + duration);
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start();
      osc.stop(ctx.currentTime + duration);
    } catch {}
  }
}

export function initVoxType(options = {}) {
  const instance = new VoxType(options);
  window.VOXTYPE = instance;
  return instance;
}
