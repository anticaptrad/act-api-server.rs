import { copyFileSync, existsSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

import { PACKAGE_ROOT } from './lib/clasp.mjs';

export function readScriptId(path) {
  if (!existsSync(path)) return '';
  try {
    const config = JSON.parse(readFileSync(path, 'utf8'));
    return typeof config.scriptId === 'string' ? config.scriptId.trim() : '';
  } catch {
    return '';
  }
}

export function createProject({ root = PACKAGE_ROOT, title = 'Anticaptrad YouTube Control Center' } = {}) {
  const claspFile = resolve(root, '.clasp.json');
  const manifest = resolve(root, 'src/appsscript.json');
  const savedManifest = resolve(root, `src/appsscript.json.saved-by-create-${process.pid}`);

  if (existsSync(claspFile)) {
    throw new Error(
      '`create:new` refuses to run while .clasp.json exists. Move it aside only when a separate project is intended.',
    );
  }
  if (!existsSync(manifest)) throw new Error('src/appsscript.json is missing.');

  renameSync(manifest, savedManifest);
  let commandStatus = 1;
  try {
    const command = process.env.CLASP_BIN ||
      (process.platform === 'win32' ? resolve(root, 'node_modules/.bin/clasp.cmd') : resolve(root, 'node_modules/.bin/clasp'));
    const result = spawnSync(command, [
      'create-script',
      '--type', 'standalone',
      '--title', title,
      '--rootDir', 'src',
    ], { cwd: root, stdio: 'inherit' });
    if (result.error) throw result.error;
    commandStatus = result.status ?? 1;
  } finally {
    if (existsSync(manifest)) rmSync(manifest);
    copyFileSync(savedManifest, manifest);
    rmSync(savedManifest);
  }

  const scriptId = readScriptId(claspFile);
  if (commandStatus !== 0 || !scriptId) {
    if (existsSync(claspFile) && !scriptId) rmSync(claspFile);
    throw new Error('Apps Script project creation failed: no valid .clasp.json/scriptId was produced.');
  }
  return scriptId;
}

export function main() {
  const scriptId = createProject();
  console.log(`Separate Apps Script project created: ${scriptId}`);
  console.log('The guarded Anticaptrad preflight and push commands intentionally reject this different Script ID.');
  console.log('Manage it from an isolated package configured for that target; do not use this channel-specific package to push it.');
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
