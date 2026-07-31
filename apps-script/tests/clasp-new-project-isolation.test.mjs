import test from 'node:test';
import assert from 'node:assert/strict';
import {
  chmodSync,
  cpSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createProject } from '../scripts/create-project.mjs';
import { runPreflight } from '../scripts/lib/clasp.mjs';

const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function fixture() {
  const root = mkdtempSync(resolve(tmpdir(), 'anticaptrad-new-project-isolation-'));
  cpSync(resolve(PACKAGE_ROOT, 'src'), resolve(root, 'src'), { recursive: true });
  cpSync(resolve(PACKAGE_ROOT, 'profiles'), resolve(root, 'profiles'), { recursive: true });
  cpSync(resolve(PACKAGE_ROOT, '.claspignore'), resolve(root, '.claspignore'));
  return root;
}

function installCreateOnlyClasp(root) {
  const path = resolve(root, 'fake-create-clasp.mjs');
  writeFileSync(path, `#!/usr/bin/env node
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const args = process.argv.slice(2);
if (args[0] !== 'create-script') process.exit(9);
const rootDir = args[args.indexOf('--rootDir') + 1];
writeFileSync(resolve(process.cwd(), rootDir, 'appsscript.json'), '{"timeZone":"Etc/UTC"}\\n');
writeFileSync(resolve(process.cwd(), '.clasp.json'), JSON.stringify({ scriptId: 'NEW_SCRIPT_ID', rootDir }, null, 2));
`);
  chmodSync(path, 0o755);
  return path;
}

test('create:new remains isolated from the Anticaptrad guarded push workflow', () => {
  const root = fixture();
  const manifest = resolve(root, 'src/appsscript.json');
  const expectedManifest = readFileSync(manifest, 'utf8');
  const previous = process.env.CLASP_BIN;
  process.env.CLASP_BIN = installCreateOnlyClasp(root);

  try {
    assert.equal(createProject({ root }), 'NEW_SCRIPT_ID');
    assert.equal(readFileSync(manifest, 'utf8'), expectedManifest);
    assert.throws(
      () => runPreflight({ root }),
      /does not match the approved Anticaptrad target/,
    );
  } finally {
    if (previous === undefined) delete process.env.CLASP_BIN;
    else process.env.CLASP_BIN = previous;
  }
});
