import test from 'node:test';
import assert from 'node:assert/strict';
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { bindProject } from '../scripts/bind-project.mjs';
import { createRemoteBackup } from '../scripts/backup-remote.mjs';
import { createProject } from '../scripts/create-project.mjs';
import { readJson, runPreflight } from '../scripts/lib/clasp.mjs';
import { openProject } from '../scripts/open-project.mjs';
import { expectedClaspConfig, PROJECT_TARGET } from '../scripts/project-target.mjs';
import { safePush } from '../scripts/push-safe.mjs';
import { redeploy } from '../scripts/redeploy.mjs';
import { applyProfile } from '../scripts/use-profile.mjs';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function fixture() {
  const root = mkdtempSync(resolve(tmpdir(), 'anticaptrad-clasp-test-'));
  cpSync(resolve(REPO_ROOT, 'src'), resolve(root, 'src'), { recursive: true });
  cpSync(resolve(REPO_ROOT, 'profiles'), resolve(root, 'profiles'), { recursive: true });
  cpSync(resolve(REPO_ROOT, '.claspignore'), resolve(root, '.claspignore'));
  return root;
}

function installFakeClasp(root) {
  const bin = resolve(root, 'fake-clasp.mjs');
  writeFileSync(bin, `#!/usr/bin/env node
import { appendFileSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
const args = process.argv.slice(2);
if (process.env.FAKE_CLASP_LOG) appendFileSync(process.env.FAKE_CLASP_LOG, JSON.stringify({ cwd: process.cwd(), args }) + '\\n');
if (args[0] === '--version') { console.log('3.3.0'); process.exit(0); }
if (args[0] === 'show-file-status') {
  if (process.env.FAKE_STATUS_JSON) { console.log(process.env.FAKE_STATUS_JSON); process.exit(0); }
  const walk = (dir) => readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(dir, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
  const source = resolve(process.cwd(), 'src');
  const filesToPush = walk(source).filter((path) => /(?:\\.gs|\\.html|appsscript\\.json)$/.test(path))
    .map((path) => 'src/' + relative(source, path).replaceAll('\\\\', '/')).sort();
  console.log(JSON.stringify({ filesToPush, untrackedFiles: ['.clasp.json'] }));
  process.exit(0);
}
if (args[0] === 'clone-script') {
  if (process.env.FAKE_FAIL_CLONE === '1') process.exit(7);
  const rootDir = args[args.indexOf('--rootDir') + 1];
  mkdirSync(resolve(process.cwd(), rootDir), { recursive: true });
  writeFileSync(resolve(process.cwd(), rootDir, 'Code.gs'), 'function remoteBackup() {}\\n');
  writeFileSync(resolve(process.cwd(), rootDir, 'appsscript.json'), '{"runtimeVersion":"V8"}\\n');
  writeFileSync(resolve(process.cwd(), '.clasp.json'), JSON.stringify({ scriptId: args[1], rootDir }, null, 2));
  process.exit(0);
}
if (args[0] === 'create-script') {
  if (process.env.FAKE_FAIL_CREATE === '1') process.exit(8);
  const rootDir = args[args.indexOf('--rootDir') + 1];
  mkdirSync(resolve(process.cwd(), rootDir), { recursive: true });
  writeFileSync(resolve(process.cwd(), rootDir, 'appsscript.json'), '{"timeZone":"Etc/UTC"}\\n');
  writeFileSync(resolve(process.cwd(), '.clasp.json'), JSON.stringify({ scriptId: 'NEW_SCRIPT_ID', rootDir }, null, 2));
  process.exit(0);
}
if (args[0] === 'create-deployment') {
  const deploymentId = process.env.FAKE_DEPLOYMENT_ID || args[args.indexOf('--deploymentId') + 1];
  const description = args[args.indexOf('--description') + 1];
  console.log(JSON.stringify({ deploymentId, versionNumber: 42, description }, null, 2));
  process.exit(0);
}
if (args[0] === 'push') process.exit(0);
if (args[0] === 'open-script') process.exit(0);
console.error('unsupported fake clasp command', args);
process.exit(9);
`);
  chmodSync(bin, 0o755);
  return bin;
}

function withFakeClasp(root, fn) {
  const names = [
    'CLASP_BIN',
    'FAKE_CLASP_LOG',
    'FAKE_STATUS_JSON',
    'FAKE_FAIL_CLONE',
    'FAKE_FAIL_CREATE',
    'FAKE_DEPLOYMENT_ID',
  ];
  const previous = Object.fromEntries(names.map((name) => [name, process.env[name]]));
  const log = resolve(root, 'clasp.log');
  process.env.CLASP_BIN = installFakeClasp(root);
  process.env.FAKE_CLASP_LOG = log;
  for (const name of names.slice(2)) delete process.env[name];
  try { return fn(log); }
  finally {
    for (const name of names) {
      if (previous[name] === undefined) delete process.env[name];
      else process.env[name] = previous[name];
    }
  }
}

function logEntries(log) {
  if (!existsSync(log)) return [];
  return readFileSync(log, 'utf8').trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);
}

test('bind writes the exact approved target with restrictive permissions', () => {
  const root = fixture();
  const path = bindProject({ root });
  assert.deepEqual(readJson(path), expectedClaspConfig());
  assert.equal(statSync(path).mode & 0o777, 0o600);
});

test('bind refuses a different target unless force is explicit', () => {
  const root = fixture();
  writeFileSync(resolve(root, '.clasp.json'), JSON.stringify({ scriptId: 'WRONG', rootDir: 'src' }));
  assert.throws(() => bindProject({ root }), /Refusing to replace/);
  bindProject({ root, force: true });
  assert.equal(readJson(resolve(root, '.clasp.json')).scriptId, PROJECT_TARGET.scriptId);
});

test('profile application is fail-closed and does not touch the manifest when the marker is missing', () => {
  const root = fixture();
  const manifest = resolve(root, 'src/appsscript.json');
  const before = readFileSync(manifest, 'utf8');
  writeFileSync(resolve(root, 'src/00_Config.gs'), 'const OTHER = true;\n');
  assert.throws(() => applyProfile('http-api', { root }), /exactly one DEPLOYMENT_PROFILE/);
  assert.equal(readFileSync(manifest, 'utf8'), before);
});

test('preflight verifies profile consistency and the exact clasp push set', () => {
  const root = fixture();
  bindProject({ root });
  applyProfile('http-api', { root });
  withFakeClasp(root, () => {
    const result = runPreflight({ root, requiredProfile: 'http-api' });
    assert.equal(result.profile.publicHttp, true);
    assert.ok(result.expectedFiles.includes('appsscript.json'));
    assert.ok(result.expectedFiles.includes('00_Config.gs'));
  });
});

test('preflight rejects an unexpected or incomplete push set', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, () => {
    process.env.FAKE_STATUS_JSON = JSON.stringify({ filesToPush: ['src/appsscript.json'], untrackedFiles: [] });
    assert.throws(() => runPreflight({ root }), /push set does not match/);
  });
});

test('remote backup records clasp version and SHA-256 integrity metadata', () => {
  const root = fixture();
  withFakeClasp(root, () => {
    const result = createRemoteBackup({ root, outputDir: 'backups/test-backup' });
    const manifest = readJson(resolve(result.backupDir, 'backup-manifest.json'));
    assert.equal(manifest.claspVersion, '3.3.0');
    assert.equal(manifest.scriptId, PROJECT_TARGET.scriptId);
    assert.ok(manifest.files.some((entry) => entry.path === 'src/Code.gs' && entry.sha256.length === 64));
  });
});

test('safe push requires a backup by default and records a receipt', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, (log) => {
    const result = safePush({ root });
    assert.ok(result.backup);
    assert.ok(existsSync(resolve(root, '.clasp-last-push.json')));
    const commands = logEntries(log).map((entry) => entry.args[0]);
    assert.ok(commands.indexOf('clone-script') < commands.indexOf('push'));
    assert.deepEqual(logEntries(log).find((entry) => entry.args[0] === 'push').args, ['push', '--force']);
  });
});

test('skipping a backup requires an explicit acknowledgement', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, () => {
    assert.throws(() => safePush({ root, skipBackup: true }), /requires --acknowledge-no-backup/);
    const result = safePush({ root, skipBackup: true, acknowledgeNoBackup: true });
    assert.equal(result.backup, null);
  });
});

test('create:new refuses an existing binding and restores the desired manifest', () => {
  const root = fixture();
  bindProject({ root });
  assert.throws(() => createProject({ root }), /refuses to run/);
  const before = readFileSync(resolve(root, 'src/appsscript.json'), 'utf8');
  writeFileSync(resolve(root, '.clasp.json'), '');
  assert.throws(() => createProject({ root }), /refuses to run/);
  const fresh = fixture();
  withFakeClasp(fresh, () => {
    const scriptId = createProject({ root: fresh });
    assert.equal(scriptId, 'NEW_SCRIPT_ID');
    assert.equal(readFileSync(resolve(fresh, 'src/appsscript.json'), 'utf8'), before);
  });
});

test('redeploy only automates the approved public HTTP deployment and records its immutable version', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, (log) => {
    assert.throws(() => redeploy({ root, profile: 'default' }), /http-api|Active profile/);
    applyProfile('http-api', { root });
    const result = redeploy({ root, profile: 'http-api', description: 'test deployment' });
    assert.equal(result.receipt.deploymentId, PROJECT_TARGET.deploymentId);
    assert.equal(result.receipt.versionNumber, 42);
    const deployment = logEntries(log).find((entry) => entry.args[0] === 'create-deployment');
    assert.ok(deployment.args.includes(PROJECT_TARGET.deploymentId));
    assert.ok(deployment.args.includes('--json'));
  });
});

test('redeploy rejects an unexpected deployment ID returned by clasp', () => {
  const root = fixture();
  bindProject({ root });
  applyProfile('http-api', { root });
  withFakeClasp(root, () => {
    process.env.FAKE_DEPLOYMENT_ID = 'WRONG_DEPLOYMENT';
    assert.throws(
      () => redeploy({ root, profile: 'http-api', description: 'mismatch test' }),
      /unexpected deployment/,
    );
    assert.equal(existsSync(resolve(root, '.clasp-last-deployment.json')), false);
  });
});

test('a failed backup blocks push before any remote overwrite', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, (log) => {
    process.env.FAKE_FAIL_CLONE = '1';
    assert.throws(() => safePush({ root }), /clone-script .* failed/);
    assert.equal(logEntries(log).some((entry) => entry.args[0] === 'push'), false);
  });
});

test('push preflight explains the bind remedy when .clasp.json is absent', () => {
  const root = fixture();
  withFakeClasp(root, () => {
    assert.throws(() => runPreflight({ root }), /\.clasp\.json is missing[\s\S]*npm run bind/);
  });
});

test('open explains the bind remedy when .clasp.json is absent', () => {
  const root = fixture();
  withFakeClasp(root, (log) => {
    assert.throws(() => openProject({ root }), /\.clasp\.json is missing[\s\S]*npm run bind/);
    assert.equal(logEntries(log).length, 0);
  });
});

test('open verifies the approved binding before invoking clasp open-script', () => {
  const root = fixture();
  bindProject({ root });
  withFakeClasp(root, (log) => {
    assert.equal(openProject({ root }), PROJECT_TARGET.scriptId);
    assert.deepEqual(logEntries(log).at(-1).args, ['open-script']);
  });
  writeFileSync(resolve(root, '.clasp.json'), JSON.stringify({ scriptId: 'WRONG', rootDir: 'src' }));
  withFakeClasp(root, () => {
    assert.throws(() => openProject({ root }), /does not match the approved Anticaptrad target/);
  });
});

test('create:new restores the desired manifest when clasp creation fails', () => {
  const root = fixture();
  const manifest = resolve(root, 'src/appsscript.json');
  const before = readFileSync(manifest, 'utf8');
  withFakeClasp(root, () => {
    process.env.FAKE_FAIL_CREATE = '1';
    assert.throws(() => createProject({ root }), /creation failed/);
    assert.equal(readFileSync(manifest, 'utf8'), before);
    assert.equal(existsSync(resolve(root, '.clasp.json')), false);
  });
});
