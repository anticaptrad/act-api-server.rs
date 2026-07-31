import { pathToFileURL } from 'node:url';
import { runPreflight } from './lib/clasp.mjs';

export function main(argv = process.argv.slice(2)) {
  const profileIndex = argv.indexOf('--profile');
  const requiredProfile = profileIndex >= 0 ? argv[profileIndex + 1] : undefined;
  if (profileIndex >= 0 && !requiredProfile) throw new Error('--profile requires a value.');
  const allowed = profileIndex >= 0 ? ['--profile', requiredProfile] : [];
  if (argv.some((arg, index) => arg !== allowed[index])) throw new Error('Unknown preflight option.');
  const result = runPreflight({ requiredProfile });
  console.log(`clasp preflight passed for ${result.profile.name}; ${result.expectedFiles.length} files will be pushed.`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  try { main(); } catch (error) { console.error(error.message); process.exit(1); }
}
