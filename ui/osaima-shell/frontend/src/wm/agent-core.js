/**
 * agent-core.js — the proactive "AI Core" of OSAIMA.
 *
 * This version polls the MCP daemon (which runs the LLM proactive loop)
 * to retrieve intelligent suggestions based on system context.
 */

export class AgentCore {
  /**
   * @param {object} opts
   * @param {import('./window-manager.js').WindowManager} opts.engine
   * @param {Function} opts.invoke   MCP invoke(cmd)
   * @param {Function} opts.suggest  (message, { actionLabel, onAction }) => void
   */
  constructor({ engine, invoke, suggest, behavior = null }) {
    this.engine = engine;
    this.invoke = invoke;
    this.suggest = suggest;
    this.behavior = behavior;

    this.lastAnyAt = 0;             // last time ANY suggestion fired
    this.globalQuietMs = 12000;     // min gap between any two suggestions
    this.timer = null;
  }

  start(intervalMs = 8000) {
    // A short delay so the desktop settles before the agent speaks up.
    setTimeout(() => this.tick(), 4000);
    this.timer = setInterval(() => this.tick(), intervalMs);
  }

  stop() { if (this.timer) clearInterval(this.timer); }

  async tick() {
    const now = Date.now();
    if (now - this.lastAnyAt < this.globalQuietMs) return;

    try {
      const suggestion = await this.invoke('system_get_suggestion');
      if (suggestion && suggestion !== null && typeof suggestion === 'object' && suggestion.message) {
        this.lastAnyAt = now;
        
        // Construct the onAction callback based on the command provided by the LLM
        let onAction = () => {};
        if (suggestion.actionCommand) {
          onAction = () => this.engine.spawn(suggestion.actionCommand);
        } else if (suggestion.actionLabel === 'Got it') {
          onAction = () => {};
        }

        this.suggest(suggestion.message, {
          actionLabel: suggestion.actionLabel || "Got it",
          onAction: onAction,
        });
      }
    } catch (err) {
      console.warn("AI Core MCP poll failed:", err);
    }
  }
}
