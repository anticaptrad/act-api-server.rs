import { existsSync, mkdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

import { hashTree, PACKAGE_ROOT, runClasp, writeJsonAtomic } from './lib/clasp.mjs';
import { PROJECT_TARGET } from './project-target.mjs';

export function createRemoteBackup({ root = PACKAGE_ROOT, outputDir } = {}) {
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const backupDir = resolve(root, outputDir || `backups/remote-before-push-${stamp}`);
  if (existsSync(backupDir)) throw new Error(`Backup destination already exists: ${backupDir}`);
  mkdirSync(backupDir, { recursive: true });
  runClasp([
    'clone-script', PROJECT_TARGET.scriptId, '--rootDir', PROJECT_TARGET.rootDir,
  ], { root, cwd: backupDir });
  const version = runClasp(['--version'], { root, capture: true }).stdout.trim();
  const manifest = {
    schemaVersion: 1,
    createdAt: new Date().toISOString(),
    scriptId: PROJECT_TARGET.scriptId,
    rootDir: PROJECT_TARGET.rootDir,
    claspVersion: version,
    files: hashTree(backupDir),
  };
  writeJsonAtomic(resolve(backupDir, 'backup-manifest.json'), manifest, 0o600);
  return { backupDir, manifest };
}

export function main(argv = process.argv.slice(2)) {
  const index = argv.indexOf('--output-dir');
  const outputDir = index >= 0 ? argv[index + 1] : undefined;
  if (index >= 0 && !outputDir) throw new Error('--output-dir requires a path.');
  if (argv.length !== (index >= 0 ? 2 : 0)) throw new Error('Usage: backup-remote.mjs [--output-dir path]');
  const result = createRemoteBackup({ outputDir });
  console.log(`Remote project backup saved to ${relative(PACKAGE_ROOT, result.backupDir)}`);
  console.log(`Backup manifest contains ${result.manifest.files.length} hashed files.`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
