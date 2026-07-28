import { copyFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const profile = process.argv[2] || 'default';
const source = resolve(`profiles/appsscript.${profile}.json`);
const target = resolve('src/appsscript.json');
if (!existsSync(source)) {
  console.error(`Unknown profile: ${profile}`);
  process.exit(2);
}
copyFileSync(source, target);
const configPath = resolve('src/00_Config.gs');
const publicHttp = profile === 'http-api';
let config = readFileSync(configPath, 'utf8');
config = config.replace(
  /const DEPLOYMENT_PROFILE = Object\.freeze\(\{[\s\S]*?\}\);/,
  `const DEPLOYMENT_PROFILE = Object.freeze({\n  NAME: '${profile}',\n  PUBLIC_HTTP: ${publicHttp}\n});`
);
writeFileSync(configPath, config);
console.log(`Applied Apps Script manifest profile: ${profile} (public HTTP: ${publicHttp})`);
