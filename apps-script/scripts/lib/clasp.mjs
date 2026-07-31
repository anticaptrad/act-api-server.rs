import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

import { expectedClaspConfig, PROJECT_TARGET } from '../project-target.mjs';

export const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

export function stableJson(value) {
  if (Array.isArray(value)) return value.map(stableJson);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, stableJson(value[key])]));
  }
  return value;
}

export function writeJsonAtomic(path, value, mode = 0o600) {
  const target = resolve(path);
  mkdirSync(dirname(target), { recursive: true });
  const temporary = `${target}.tmp-${process.pid}-${Date.now()}`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode });
  chmodSync(temporary, mode);
  renameSync(temporary, target);
  chmodSync(target, mode);
}

export function readJson(path, label = path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    throw new Error(`${label} is missing or invalid JSON: ${error.message}`);
  }
}

export function assertClaspTarget({ root = PACKAGE_ROOT } = {}) {
  const configPath = resolve(root, '.clasp.json');
  const actual = readJson(configPath, '.clasp.json');
  const expected = expectedClaspConfig();
  if (JSON.stringify(stableJson(actual)) !== JSON.stringify(stableJson(expected))) {
    throw new Error(
      `.clasp.json does not match the approved Anticaptrad target. Run \`npm run bind\`.\n` +
      `Expected ${JSON.stringify(expected)} but found ${JSON.stringify(actual)}.`,
    );
  }
  return actual;
}

export function readActiveProfile({ root = PACKAGE_ROOT } = {}) {
  const config = readFileSync(resolve(root, 'src/00_Config.gs'), 'utf8');
  const match = config.match(
    /const DEPLOYMENT_PROFILE = Object\.freeze\(\{\s*NAME: '([^']+)',\s*PUBLIC_HTTP: (true|false)\s*\}\);/m,
  );
  if (!match) throw new Error('Unable to read DEPLOYMENT_PROFILE from src/00_Config.gs.');
  return { name: match[1], publicHttp: match[2] === 'true' };
}

export function assertProfileConsistency({ root = PACKAGE_ROOT, requiredProfile } = {}) {
  const active = readActiveProfile({ root });
  if (requiredProfile && active.name !== requiredProfile) {
    throw new Error(`Active profile is ${active.name}; ${requiredProfile} is required.`);
  }
  if (active.publicHttp !== (active.name === 'http-api')) {
    throw new Error(`DEPLOYMENT_PROFILE.PUBLIC_HTTP is inconsistent for profile ${active.name}.`);
  }
  const activeManifest = stableJson(readJson(resolve(root, 'src/appsscript.json')));
  const profileManifest = stableJson(readJson(resolve(root, `profiles/appsscript.${active.name}.json`)));
  if (JSON.stringify(activeManifest) !== JSON.stringify(profileManifest)) {
    throw new Error(`src/appsscript.json does not exactly match profile ${active.name}.`);
  }
  return active;
}

export function claspBinary({ root = PACKAGE_ROOT } = {}) {
  if (process.env.CLASP_BIN) return process.env.CLASP_BIN;
  const executable = process.platform === 'win32' ? 'clasp.cmd' : 'clasp';
  const candidate = resolve(root, 'node_modules/.bin', executable);
  if (!existsSync(candidate)) {
    throw new Error('Pinned clasp executable is missing. Run `npm ci` first.');
  }
  return candidate;
}

export function runClasp(args, {
  root = PACKAGE_ROOT,
  cwd = root,
  capture = false,
  env = process.env,
} = {}) {
  const result = spawnSync(claspBinary({ root }), args, {
    cwd,
    env,
    encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
  });
  if (result.error) throw new Error(`Unable to run clasp: ${result.error.message}`);
  if ((result.status ?? 1) !== 0) {
    const details = capture ? `\n${result.stderr || result.stdout || ''}` : '';
    throw new Error(`clasp ${args.join(' ')} failed with status ${result.status}.${details}`);
  }
  return result;
}

function walkFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...walkFiles(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function normalizeLocalPath(path, rootDir) {
  let value = String(path).replaceAll('\\', '/').replace(/^\.\//, '');
  const prefix = `${rootDir.replaceAll('\\', '/').replace(/\/$/, '')}/`;
  if (value.startsWith(prefix)) value = value.slice(prefix.length);
  return value;
}

export function expectedPushFiles({ root = PACKAGE_ROOT } = {}) {
  const sourceRoot = resolve(root, PROJECT_TARGET.rootDir);
  return walkFiles(sourceRoot)
    .filter((path) => path.endsWith('.gs') || path.endsWith('.html') || path.endsWith('appsscript.json'))
    .map((path) => relative(sourceRoot, path).split(sep).join('/'))
    .sort();
}

export function parseStatusJson(stdout) {
  const lines = String(stdout).trim().split(/\r?\n/).filter(Boolean);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    try {
      const parsed = JSON.parse(lines[index]);
      if (Array.isArray(parsed.filesToPush) && Array.isArray(parsed.untrackedFiles)) return parsed;
    } catch {
      // Ignore non-JSON spinner/log lines and continue upward.
    }
  }
  throw new Error('clasp show-file-status --json returned no valid status object.');
}

export function runPreflight({ root = PACKAGE_ROOT, requiredProfile } = {}) {
  assertClaspTarget({ root });
  const profile = assertProfileConsistency({ root, requiredProfile });
  const result = runClasp(['show-file-status', '--json'], { root, capture: true });
  const status = parseStatusJson(result.stdout);
  const actual = status.filesToPush
    .map((path) => normalizeLocalPath(path, PROJECT_TARGET.rootDir))
    .sort();
  const expected = expectedPushFiles({ root });
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `clasp push set does not match src/.\nExpected: ${expected.join(', ')}\nActual: ${actual.join(', ')}`,
    );
  }
  return { profile, status, expectedFiles: expected };
}

export function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

export function hashTree(directory) {
  return walkFiles(directory)
    .filter((path) => !path.endsWith('backup-manifest.json'))
    .map((path) => ({
      path: relative(directory, path).split(sep).join('/'),
      bytes: statSync(path).size,
      sha256: sha256File(path),
    }))
    .sort((a, b) => a.path.localeCompare(b.path));
}
