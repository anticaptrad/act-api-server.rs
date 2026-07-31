import { resolve, relative } from 'node:path';
import { pathToFileURL } from 'node:url';

import { createRemoteBackup } from './backup-remote.mjs';
import {
  hashTree,
  PACKAGE_ROOT,
  runClasp,
  runPreflight,
  writeJsonAtomic,
} from './lib/clasp.mjs';
import { PROJECT_TARGET } from './project-target.mjs';

export function safePush({
  root = PACKAGE_ROOT,
  requiredProfile,
  skipBackup = false,
  acknowledgeNoBackup = false,
} = {}) {
  if (skipBackup && !acknowledgeNoBackup) {
    throw new Error('--skip-backup requires --acknowledge-no-backup.');
  }
  const preflight = runPreflight({ root, requiredProfile });
  const backup = skipBackup ? null : createRemoteBackup({ root });
  runClasp(['push', '--force'], { root });
  const receipt = {
    schemaVersion: 1,
    pushedAt: new Date().toISOString(),
    scriptId: PROJECT_TARGET.scriptId,
    profile: preflight.profile.name,
    publicHttp: preflight.profile.publicHttp,
    backup: backup ? relative(root, backup.backupDir).replaceAll('\\', '/') : null,
    files: hashTree(resolve(root, PROJECT_TARGET.rootDir)),
  };
  writeJsonAtomic(resolve(root, '.clasp-last-push.json'), receipt, 0o600);
  return { preflight, backup, receipt };
}

export function main(argv = process.argv.slice(2)) {
  const skipBackup = argv.includes('--skip-backup');
  const acknowledgeNoBackup = argv.includes('--acknowledge-no-backup');
  const profileIndex = argv.indexOf('--profile');
  const requiredProfile = profileIndex >= 0 ? argv[profileIndex + 1] : undefined;
  const known = new Set(['--skip-backup', '--acknowledge-no-backup', '--profile', requiredProfile]);
  const unknown = argv.filter((arg) => !known.has(arg));
  if (unknown.length || (profileIndex >= 0 && !requiredProfile)) {
    throw new Error('Usage: push-safe.mjs [--profile name] [--skip-backup --acknowledge-no-backup]');
  }
  const result = safePush({ requiredProfile, skipBackup, acknowledgeNoBackup });
  console.log(`Pushed ${result.receipt.files.length} source files using profile ${result.receipt.profile}.`);
  if (result.receipt.backup) console.log(`Pre-push backup: ${result.receipt.backup}`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
