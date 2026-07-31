#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v node >/dev/null 2>&1; then
  echo "Node.js 22+ is required." >&2
  exit 1
fi
node_major="$(node -p 'process.versions.node.split(`.`)[0]')"
if (( node_major < 22 )); then
  echo "Node.js 22+ is required; found $(node -v)." >&2
  exit 1
fi

npm ci
npm run check
npm run bind

echo
cat <<'STEPS'
1. Enable the Apps Script API at https://script.google.com/home/usersettings
2. Run: npm run login
3. Confirm the account: npm run auth:status
4. Validate the exact target/profile/push set: npm run preflight
5. Run the guarded push: npm run push
   - This creates a hashed remote backup before clasp replaces project content.
6. Run: npm run open
7. For the owner dashboard, create/update an owner-only web-app deployment in the Apps Script editor.
8. For the public keyed HTTP deployment:
   npm run profile:http-api
   npm run deploy:http-api
9. Open the app and complete First-run setup with the owning Google account.
STEPS
