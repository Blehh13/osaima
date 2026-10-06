// Regenerates src/wm/default-config.js (the embedded fallback copy) from src/config/wm.lua.
// Run `npm run sync-config` after editing wm.lua; a test fails if they differ.
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const here = (path) => fileURLToPath(new URL(path, import.meta.url));

/** Make `text` safe to place inside a JavaScript template literal. */
export function escapeForTemplate(text) {
  return text.replace(/\\/g, '\\\\').replace(/`/g, '\\`').replace(/\$\{/g, '\\${');
}

export function renderDefaultConfig(lua) {
  return `/**
 * default-config.js: fallback copy of config/wm.lua embedded as a string.
 *
 * GENERATED from src/config/wm.lua by scripts/sync-default-config.mjs; do not
 * edit by hand (run \`npm run sync-config\`). At boot the shell tries to fetch()
 * wm.lua; when it can't (e.g. opened over file:// where fetch is blocked), it
 * falls back to this embedded copy so the WM always boots.
 */

export const DEFAULT_WM_LUA = \`${escapeForTemplate(lua)}\`;
`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const lua = readFileSync(here('../src/config/wm.lua'), 'utf8').replace(/\r\n/g, '\n');
  writeFileSync(here('../src/wm/default-config.js'), renderDefaultConfig(lua));
  console.log('default-config.js regenerated from wm.lua');
}
