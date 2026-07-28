import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SCRIPT_ID = '17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ';
const PROJECT_URL = 'https://script.google.com/home/projects/17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ/edit';
const claspFile = resolve('.clasp.json');
const expected = { scriptId: SCRIPT_ID, rootDir: 'src' };

let current = null;
if (existsSync(claspFile)) {
  try {
    current = JSON.parse(readFileSync(claspFile, 'utf8'));
  } catch (error) {
    console.error(`Existing .clasp.json is invalid: ${error.message}`);
    process.exit(1);
  }
}

if (current?.scriptId && current.scriptId !== SCRIPT_ID && !process.argv.includes('--force')) {
  console.error(`Refusing to replace a different Script ID (${current.scriptId}).`);
  console.error('Run `node scripts/bind-project.mjs --force` only after confirming the target.');
  process.exit(1);
}

writeFileSync(claspFile, `${JSON.stringify(expected, null, 2)}\n`, { mode: 0o600 });
console.log(`Bound local source to existing Apps Script project: ${SCRIPT_ID}`);
console.log(PROJECT_URL);
console.log('The existing Apps Script project name is preserved; clasp push changes project files, not its title.');
