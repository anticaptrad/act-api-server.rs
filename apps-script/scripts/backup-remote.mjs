import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const SCRIPT_ID = '17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ';
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const backupDir = resolve('backups', `remote-before-push-${stamp}`);
mkdirSync(backupDir, { recursive: true });

const command = process.platform === 'win32' ? 'npx.cmd' : 'npx';
const result = spawnSync(command, [
  'clasp', 'clone-script', SCRIPT_ID, '--rootDir', 'src'
], { cwd: backupDir, stdio: 'inherit' });

if ((result.status ?? 1) !== 0) {
  console.error('Remote backup failed; no push was performed by this command.');
  process.exit(result.status ?? 1);
}
console.log(`Remote project backup saved to ${backupDir}`);
