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

npm install
npm run check

echo
cat <<'STEPS'
1. Enable the Apps Script API at https://script.google.com/home/usersettings
2. Run: npx clasp login
3. Run: npm run bind
4. Optional safety copy: npm run backup:remote
5. Run: npm run push
6. Run: npm run open
7. In Apps Script: Deploy > New deployment > Web app
   - Execute as: Me
   - Who has access: Only myself
8. Open the web app and click “First-run setup”.
STEPS
