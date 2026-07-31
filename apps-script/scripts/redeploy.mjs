import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

import { safePush } from './push-safe.mjs';
import { PACKAGE_ROOT, runClasp, writeJsonAtomic } from './lib/clasp.mjs';
import { PROJECT_TARGET } from './project-target.mjs';

export function parseDeploymentJson(stdout) {
  let deployment;
  try {
    deployment = JSON.parse(String(stdout).trim());
  } catch (error) {
    throw new Error(`clasp create-deployment --json returned invalid JSON: ${error.message}`);
  }
  if (deployment.deploymentId !== PROJECT_TARGET.deploymentId) {
    throw new Error(
      `clasp updated unexpected deployment ${deployment.deploymentId || '(missing)'}; ` +
      `expected ${PROJECT_TARGET.deploymentId}.`,
    );
  }
  if (!Number.isInteger(deployment.versionNumber) || deployment.versionNumber < 1) {
    throw new Error('clasp deployment response did not contain a positive immutable version number.');
  }
  return deployment;
}

export function redeploy({ root = PACKAGE_ROOT, profile = 'http-api', description } = {}) {
  if (profile !== 'http-api') throw new Error('Only the approved http-api deployment is automated.');
  const push = safePush({ root, requiredProfile: profile });
  const deploymentDescription = description ||
    `Anticaptrad ${profile} ${new Date().toISOString()}`;
  const result = runClasp([
    'create-deployment',
    '--deploymentId', PROJECT_TARGET.deploymentId,
    '--description', deploymentDescription,
    '--json',
  ], { root, capture: true });
  const deployment = parseDeploymentJson(result.stdout);
  const receipt = {
    schemaVersion: 1,
    deployedAt: new Date().toISOString(),
    scriptId: PROJECT_TARGET.scriptId,
    deploymentId: deployment.deploymentId,
    versionNumber: deployment.versionNumber,
    webAppUrl: PROJECT_TARGET.webAppUrl,
    profile,
    description: deployment.description || deploymentDescription,
    pushReceipt: '.clasp-last-push.json',
  };
  writeJsonAtomic(resolve(root, '.clasp-last-deployment.json'), receipt, 0o600);
  return { push, deployment, receipt };
}

export function main(argv = process.argv.slice(2)) {
  const profileIndex = argv.indexOf('--profile');
  const descriptionIndex = argv.indexOf('--description');
  const profile = profileIndex >= 0 ? argv[profileIndex + 1] : 'http-api';
  const description = descriptionIndex >= 0 ? argv[descriptionIndex + 1] : undefined;
  if ((profileIndex >= 0 && !profile) || (descriptionIndex >= 0 && !description)) {
    throw new Error('Missing redeploy option value.');
  }
  const consumed = new Set([profileIndex, profileIndex + 1, descriptionIndex, descriptionIndex + 1]);
  if (argv.some((_, index) => !consumed.has(index))) {
    throw new Error('Usage: redeploy.mjs [--profile http-api] [--description text]');
  }
  if (profile !== 'http-api') throw new Error('Only the approved http-api deployment is automated.');
  const result = redeploy({ profile, description });
  console.log(
    `Redeployed ${result.receipt.deploymentId} at immutable version ${result.receipt.versionNumber} ` +
    `with profile ${profile}.`,
  );
  console.log(result.receipt.webAppUrl);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
