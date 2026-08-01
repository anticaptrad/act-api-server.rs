import { pathToFileURL } from 'node:url';

import { assertClaspTarget, PACKAGE_ROOT, runClasp } from './lib/clasp.mjs';
import { PROJECT_TARGET } from './project-target.mjs';

export function openProject({ root = PACKAGE_ROOT } = {}) {
  assertClaspTarget({ root });
  runClasp(['open-script'], { root });
  return PROJECT_TARGET.scriptId;
}

export function main(argv = process.argv.slice(2)) {
  if (argv.length) throw new Error('`npm run open` takes no arguments.');
  const scriptId = openProject();
  console.log(`Opened the approved Anticaptrad Apps Script project: ${scriptId}`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
