/**
 * behavior.js — the behavior-learning store.
 *
 * Matches the OSAIMA PDR "Behavior Store & Style-Template Engine" subsystem
 * (§3.7): OSAIMA's memory of who you are and how you work. The PDR uses SQLite +
 * ChromaDB on-device; here we persist a compact profile to localStorage (the
 * browser's on-device store) so the demo learns across sessions with no backend.
 *
 * It learns:
 *   - app usage frequency        (most-used apps)
 *   - preferred layout           (which layout you pick most)
 *   - workspace usage            (where you spend time)
 *   - time-of-day activity       (hourly histogram)
 *   - assistant query history    (what you ask)
 *   - suggestion acceptance rate (does the proactive agent help?)
 *
 * The learned profile feeds the proactive AI Core (better suggestions) and the
 * Behavior app (shows the user what OSAIMA has learned) — closing the PDR loop of
 * "learn from behavior, then inject it back into the agent."
 */

const KEY = 'osaima.behavior';

function emptyProfile() {
  return {
    appCounts: {},        // appId -> times launched
    layoutCounts: {},     // layout -> times chosen
    workspaceCounts: {},  // ws index -> times visited
    hourly: {},           // hour (0-23) -> activity count
    assistantQueries: [], // recent NL queries (capped)
    suggestionsShown: 0,
    suggestionsAccepted: 0,
    firstApp: {},         // appId -> times it was the first app of a session
    totalEvents: 0,
    createdAt: Date.now(),
  };
}

export class BehaviorStore {
  constructor() {
    this.profile = this._load();
    this._sessionHadApp = false;
    this._saveTimer = null;
  }

  _load() {
    try {
      const raw = localStorage.getItem(KEY);
      if (raw) return Object.assign(emptyProfile(), JSON.parse(raw));
    } catch {}
    return emptyProfile();
  }

  _save() {
    // Throttle writes so rapid events don't thrash localStorage.
    if (this._saveTimer) return;
    this._saveTimer = setTimeout(() => {
      this._saveTimer = null;
      try { localStorage.setItem(KEY, JSON.stringify(this.profile)); } catch {}
    }, 400);
  }

  _bump(map, key, by = 1) {
    map[key] = (map[key] || 0) + by;
  }

  // ── Event recording ──────────────────────────────────────────────────────
  recordAppLaunch(appId) {
    this._bump(this.profile.appCounts, appId);
    this._bump(this.profile.hourly, new Date().getHours());
    if (!this._sessionHadApp) {
      this._bump(this.profile.firstApp, appId);
      this._sessionHadApp = true;
    }
    this.profile.totalEvents++;
    this._save();
  }

  recordLayout(layout) {
    this._bump(this.profile.layoutCounts, layout);
    this.profile.totalEvents++;
    this._save();
  }

  recordWorkspace(idx) {
    this._bump(this.profile.workspaceCounts, idx);
    this._save();
  }

  recordAssistantQuery(query) {
    this.profile.assistantQueries.unshift({ q: query, at: Date.now() });
    this.profile.assistantQueries = this.profile.assistantQueries.slice(0, 25);
    this._bump(this.profile.hourly, new Date().getHours());
    this.profile.totalEvents++;
    this._save();
  }

  recordSuggestionShown() { this.profile.suggestionsShown++; this._save(); }
  recordSuggestionAccepted() { this.profile.suggestionsAccepted++; this._save(); }

  // ── Learned insights ─────────────────────────────────────────────────────
  topApps(n = 3) {
    return Object.entries(this.profile.appCounts)
      .sort((a, b) => b[1] - a[1])
      .slice(0, n)
      .map(([id, count]) => ({ id, count }));
  }

  preferredLayout() {
    const e = Object.entries(this.profile.layoutCounts).sort((a, b) => b[1] - a[1]);
    return e.length ? e[0][0] : null;
  }

  // Predict the app you'll likely want next (most-used, or your usual first app).
  predictNextApp() {
    const first = Object.entries(this.profile.firstApp).sort((a, b) => b[1] - a[1]);
    if (first.length && first[0][1] >= 2) return first[0][0];
    const top = this.topApps(1);
    return top.length ? top[0].id : null;
  }

  acceptanceRate() {
    if (this.profile.suggestionsShown === 0) return null;
    return this.profile.suggestionsAccepted / this.profile.suggestionsShown;
  }

  peakHour() {
    const e = Object.entries(this.profile.hourly).sort((a, b) => b[1] - a[1]);
    return e.length ? parseInt(e[0][0], 10) : null;
  }

  summary() {
    return {
      totalEvents: this.profile.totalEvents,
      topApps: this.topApps(5),
      preferredLayout: this.preferredLayout(),
      predictedNext: this.predictNextApp(),
      acceptanceRate: this.acceptanceRate(),
      peakHour: this.peakHour(),
      recentQueries: this.profile.assistantQueries.slice(0, 8),
    };
  }

  reset() {
    this.profile = emptyProfile();
    try { localStorage.removeItem(KEY); } catch {}
  }
}
