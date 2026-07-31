import { existsSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

import { assertProfileConsistency, PACKAGE_ROOT, readJson } from './lib/clasp.mjs';

const PROFILE_PATTERN = /const DEPLOYMENT_PROFILE = Object\.freeze\(\{[\s\S]*?\}\);/g;

export function applyProfile(profile, { root = PACKAGE_ROOT } = {}) {
  const source = resolve(root, `profiles/appsscript.${profile}.json`);
  if (!existsSync(source)) throw new Error(`Unknown profile: ${profile}`);
  const profileManifest = readJson(source, `profile ${profile}`);
  const publicHttp = profile === 'http-api';
  const configPath = resolve(root, 'src/00_Config.gs');
  const manifestPath = resolve(root, 'src/appsscript.json');
  const config = readFileSync(configPath, 'utf8');
  const matches = [...config.matchAll(PROFILE_PATTERN)];
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one DEPLOYMENT_PROFILE block; found ${matches.length}.`);
  }
  const nextConfig = config.replace(
    PROFILE_PATTERN,
    `const DEPLOYMENT_PROFILE = Object.freeze({\n  NAME: '${profile}',\n  PUBLIC_HTTP: ${publicHttp}\n});`,
  );
  const manifestTemp = `${manifestPath}.tmp-${process.pid}`;
  const configTemp = `${configPath}.tmp-${process.pid}`;
  writeFileSync(manifestTemp, `${JSON.stringify(profileManifest, null, 2)}\n`);
  writeFileSync(configTemp, nextConfig);
  renameSync(manifestTemp, manifestPath);
  renameSync(configTemp, configPath);
  return assertProfileConsistency({ root, requiredProfile: profile });
}

export function main(argv = process.argv.slice(2)) {
  if (argv.length !== 1) throw new Error('Usage: node scripts/use-profile.mjs <profile>');
  const result = applyProfile(argv[0]);
  console.log(`Applied Apps Script profile: ${result.name} (public HTTP: ${result.publicHttp})`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
