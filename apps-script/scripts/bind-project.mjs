import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

import { assertClaspTarget, PACKAGE_ROOT, readJson, writeJsonAtomic } from './lib/clasp.mjs';
import { expectedClaspConfig, PROJECT_TARGET } from './project-target.mjs';

export function bindProject({ root = PACKAGE_ROOT, force = false } = {}) {
  const claspFile = resolve(root, '.clasp.json');
  if (existsSync(claspFile)) {
    const current = readJson(claspFile, '.clasp.json');
    if (current.scriptId && current.scriptId !== PROJECT_TARGET.scriptId && !force) {
      throw new Error(
        `Refusing to replace a different Script ID (${current.scriptId}). ` +
        'Re-run with --force only after confirming the target.',
      );
    }
  }
  writeJsonAtomic(claspFile, expectedClaspConfig());
  assertClaspTarget({ root });
  return claspFile;
}

export function main(argv = process.argv.slice(2)) {
  const unknown = argv.filter((arg) => arg !== '--force');
  if (unknown.length) throw new Error(`Unknown bind option(s): ${unknown.join(', ')}`);
  bindProject({ force: argv.includes('--force') });
  console.log(`Bound local source to existing Apps Script project: ${PROJECT_TARGET.scriptId}`);
  console.log(PROJECT_TARGET.projectUrl);
  console.log('The existing Apps Script project name is preserved; clasp push changes files, not its title.');
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
