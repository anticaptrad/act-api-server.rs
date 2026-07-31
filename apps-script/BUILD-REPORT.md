# Build report

Built for `github.com/anticaptrad` and the YouTube channel handle `@anticaptrad` on 2026-07-27.

## Included

- Apps Script web dashboard and authenticated JSON HTTP API
- YouTube Data API v3 channel, video, playlist, thumbnail, and publishing operations
- YouTube Analytics API v2 reports and Drive exports
- Resumable uploads with private-by-default visibility, retry reconciliation, audit records, and idempotency protection
- Drive source-video backups, metadata manifests, analytics reports, and audit logs
- Gmail attachment ingestion and operational notifications
- Optional YouTube monetization, YouTube Partner/Content ID, and Google Workspace Admin SDK profiles
- Owner-only default deployment and a separate API-key-protected anonymous HTTP deployment profile
- Linux/macOS and PowerShell installation helpers, profile switcher, and static validation tests

## Validation

Command:

```bash
npm run check
```

Result:

```text
VALIDATION PASSED
- 10 server files
- manifest services and scopes verified
- server and browser JavaScript syntax verified
- private-by-default, idempotency, HTTP isolation, and resumable-upload contracts verified
- no obvious committed secrets found

PROFILE VALIDATION PASSED
- default
- monetization
- partner
- workspace-admin
- http-api
```

## Live-account boundary

Static validation confirms that the package, Apps Script manifest profiles, and JavaScript syntax are internally consistent. A live end-to-end authorization and upload cannot be completed without signing in as the Google account that owns or manages `@anticaptrad`. YouTube Partner/Content ID and Admin SDK operations additionally require the corresponding partner or Google Workspace administrator entitlements.

## Installer correction in 1.0.3

- Creates a standalone Apps Script project; web-app behavior is selected during deployment.
- Treats a missing or invalid `.clasp.json` as a hard failure even if `clasp` returns exit status zero.
- Prevents `push` and `open` instructions from being shown after unsuccessful project creation.

## Existing-project binding fix

- Pre-bound to Script ID `17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ`.
- Preserves the existing project title `youtube channel anticaptrad mgmt http interface`.
- `npm run create` now safely binds this existing project; `npm run create:new` is the explicit new-project path.
- Added optional pre-push remote backup and file-status commands.

## Clasp workflow hardening in 1.1.0

- Replaced unconditional force push with target/profile/file-set preflight, hashed remote backup, guarded force push, and local receipts.
- Generates `.clasp.json` locally with restrictive permissions and deterministic `filePushOrder`; it is no longer committed.
- Uses the pinned local clasp binary and reproducible `npm ci` installation.
- Added versioned redeployment for the approved public HTTP deployment ID.
- Added 12 mocked workflow tests covering failure paths as well as successful binding, backup, push, and redeployment.
