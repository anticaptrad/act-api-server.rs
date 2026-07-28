import { copyFileSync, existsSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const claspFile = resolve('.clasp.json');
const manifest = resolve('src/appsscript.json');
const savedManifest = resolve('src/appsscript.json.saved-by-create');

function readScriptId() {
  if (!existsSync(claspFile)) return '';
  try {
    const config = JSON.parse(readFileSync(claspFile, 'utf8'));
    return typeof config.scriptId === 'string' ? config.scriptId.trim() : '';
  } catch {
    return '';
  }
}

const existingScriptId = readScriptId();
if (existingScriptId) {
  console.log(`.clasp.json already exists; using Apps Script project ${existingScriptId}.`);
  process.exit(0);
}
if (existsSync(claspFile)) {
  console.error('Existing .clasp.json is invalid or has no scriptId. Remove or repair it before continuing.');
  process.exit(1);
}

// clasp create-script writes a starter manifest. Temporarily move our manifest
// away, then restore it after the remote standalone project is created.
renameSync(manifest, savedManifest);
let commandStatus = 1;
try {
  const command = process.platform === 'win32' ? 'npx.cmd' : 'npx';
  const result = spawnSync(command, [
    'clasp', 'create-script',
    '--type', 'standalone',
    '--title', 'Anticaptrad YouTube Control Center',
    '--rootDir', 'src'
  ], { stdio: 'inherit' });
  commandStatus = result.status ?? 1;
} finally {
  if (existsSync(manifest)) rmSync(manifest);
  copyFileSync(savedManifest, manifest);
  rmSync(savedManifest);
}

const scriptId = readScriptId();
if (commandStatus !== 0 || !scriptId) {
  if (existsSync(claspFile) && !scriptId) rmSync(claspFile);
  console.error('Apps Script project creation failed: no valid .clasp.json/scriptId was produced.');
  console.error('Confirm the Apps Script API is enabled, then run `npm run create` again.');
  process.exit(commandStatus || 1);
}

console.log(`Apps Script project created: ${scriptId}`);
console.log('Run `npm run push` next, then `npm run open`.');
