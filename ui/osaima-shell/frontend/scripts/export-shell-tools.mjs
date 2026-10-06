// Prints the tools the shell offers the assistant, as JSON, for the evaluation
// harness (ai-core/agent/evals/shell-tools.snapshot.json). The app list comes
// from the shell's real app registration, so it can't drift.
//
//   node scripts/export-shell-tools.mjs > ../../../ai-core/agent/evals/shell-tools.snapshot.json
//
// A test fails if the committed snapshot no longer matches.

import { fileURLToPath } from 'node:url';
import { registerApps } from '../src/wm/apps.js';
import { shellTools } from '../src/wm/shell-tools.js';

export function exportShellTools() {
  const appRegistry = new Map();
  // registerApps only builds app definitions; nothing runs until a window mounts.
  registerApps({ registerApp: (app) => appRegistry.set(app.id, app) }, {});
  return shellTools({ appRegistry }).specs();
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.stdout.write(`${JSON.stringify(exportShellTools(), null, 2)}\n`);
}
