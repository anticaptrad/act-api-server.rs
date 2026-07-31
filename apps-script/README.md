# Anticaptrad YouTube Control Center (Google Apps Script)

A deployable Apps Script web app and JSON HTTP endpoint for the YouTube channel at `youtube.com/@anticaptrad`.

## Included

- YouTube Data API v3: channel details, recent uploads, metadata updates, playlists, thumbnails, and video uploads.
- YouTube Analytics API v2: date-range reports and CSV/JSON exports to Drive.
- Resumable uploads from Google Drive in 8 MB chunks, continued by a five-minute Apps Script trigger.
- Source-file backup in Drive **before** an upload begins.
- Gmail label ingestion for small video attachments and Gmail status/digest notifications.
- A browser dashboard and a keyed `doPost` JSON API.
- Upload idempotency keys and resumable-session reconciliation to prevent duplicate retries and byte-range corruption.
- Optional YouTube Content ID / Partner, monetization analytics, public HTTP, and Workspace Admin manifest profiles.

## Important behavior

1. Every upload starts as **private**.
2. Public and unlisted transitions are disabled until explicitly enabled.
3. The app validates the authorized channel against `@anticaptrad` and then pins the channel ID.
4. The HTTP API key is returned once; only its SHA-256 hash is stored.
5. This project treats the Drive source as the authoritative backup and copies it **before uploading**; it does not try to reconstruct or download an original from YouTube.
6. Gmail is useful for notifications and small clip ingestion, not full-size video backup. Gmail attachments are capped and this app enforces a 20 MB ingestion limit.

## First install

### Requirements

- The Google account that owns or manages `@anticaptrad`.
- Node.js 22 or newer.
- The Apps Script API enabled in the account's Apps Script settings.

### macOS / Linux

```bash
unzip anticaptrad-youtube-gas.zip
cd anticaptrad-youtube-gas
./scripts/install.sh
npm run login
npm run auth:status
npm run preflight
npm run push
npm run open
```

### Windows PowerShell

```powershell
Expand-Archive .\anticaptrad-youtube-gas.zip
cd .\anticaptrad-youtube-gas
.\scripts\install.ps1
npm run login
npm run auth:status
npm run preflight
npm run push
npm run open
```

Then in the Apps Script editor:

1. Choose **Deploy → New deployment → Web app**.
2. Execute as **Me**.
3. Set access to **Only myself**.
4. Deploy, accept the requested scopes, and open the web app URL.
5. Click **First-run setup**.
6. Verify the displayed channel is `@anticaptrad` before queueing a video.

### Existing Apps Script project binding

This Anticaptrad package targets the existing standalone Apps Script project:

- Name: **youtube channel anticaptrad mgmt http interface**
- Script ID: `17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ`
- Editor: `https://script.google.com/home/projects/17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ/edit`

`npm run bind` generates a verified, mode-`0600` `.clasp.json` with the approved Script ID, `rootDir: "src"`, and deterministic `filePushOrder`. The generated file is ignored by Git and is not shipped as source. Binding does not rename the Apps Script project.

`npm run push` is guarded: it runs the full test suite, verifies the exact target/profile/file set through `clasp show-file-status --json`, clones and hashes the remote project into `backups/`, and only then invokes the required whole-project `clasp push --force`. A failed backup or preflight prevents the push. The emergency `npm run push:unsafe` command bypasses those protections and should not be used during normal operation.

The earlier `Invalid container file type` run did **not** create or bind a project. `npm run create` is now intentionally disabled because the name is ambiguous. Use `npm run bind` for this existing project or `npm run create:new` only when a separate standalone Apps Script project is deliberately required. A separately created Script ID is intentionally rejected by this package's Anticaptrad preflight and push commands and must be managed from an isolated package configured for that target.

The setup action creates this Drive tree:

```text
Anticaptrad YouTube/
  01 Inbox/
  02 Source Backups/
  03 Thumbnails/
  04 Metadata/
  05 Analytics Reports/
  06 Audit Logs/
```

## Upload workflow

1. Put a source video in Drive.
2. Copy the Drive file ID from its URL.
3. Open the web app's **Upload** tab.
4. Enter metadata, explicitly choose the audience setting, and confirm rights.
5. The app makes a server-side Drive backup, starts a YouTube resumable session, and queues the job.
6. The UI or trigger transfers chunks until complete.
7. The final video remains private and a completion email is sent.

For a new Google Cloud project, YouTube may restrict API uploads to private status until the project passes YouTube's API compliance audit. This app is deliberately compatible with that restriction.

## HTTP interface

The default profile is owner-only and is safest. The JSON endpoint is still available to the signed-in owner:

```json
POST <WEB_APP_URL>
Content-Type: application/json

{
  "action": "channel",
  "apiKey": "acp_..."
}
```

Main actions:

- `health` — no API key required
- `channel`
- `videos`
- `analytics`
- `exportAnalytics`
- `jobs`
- `startUpload` — accepts an optional `idempotencyKey`; reuse it when retrying the same request
- `processUpload`
- `processAllUploads`
- `updateVideo`
- `publishVideo`
- `createPlaylist`
- `addToPlaylist`
- `ingestGmail`
- `sendDigest`
- `partnerStatus`, `partnerOwners`, `partnerClaims` — partner profile only
- `adminStatus`, `workspaceUsers` — Workspace Admin profile only

To update the approved server-to-server endpoint without Google sign-in:

```bash
npm run profile:http-api
npm run deploy:http-api
```

`deploy:http-api` performs a guarded push and updates the existing versioned deployment ID. It refuses to deploy any profile other than `http-api`. This uses `ANYONE_ANONYMOUS`; protect the generated API key, rotate it if disclosed, and do not put it in a repository, URL, browser log, or chat. The API key belongs in the JSON POST body. The profile command also disables the HTML/RPC dashboard; privileged GET actions and anonymous `google.script.run` calls are rejected.

To return the source tree to the owner-only default:

```bash
npm run profile:default
npm run push
```

Create or update the owner-only dashboard deployment separately in the Apps Script editor. Do not accidentally repoint the approved public HTTP deployment to the owner profile.

## Optional profiles

### Monetization analytics

```bash
npm run profile:monetization
npm run push
```

Adds the monetary analytics OAuth scope. It does not by itself make a channel eligible for monetization.

### YouTube Partner / Content ID

```bash
npm run profile:partner
npm run push
```

Adds the `YouTubeContentId` advanced service and `youtubepartner` scope. This only works for accounts that YouTube has granted Content ID/content-owner access. Ordinary YouTube Partner Program membership does not necessarily include Content ID API access.

Set the Content Owner ID in the Settings tab before partner calls.

### Workspace Admin SDK

```bash
npm run profile:workspace-admin
npm run push
```

Adds read-only Admin Directory access. This only applies to an administrator in a Google Workspace domain. It is not usable for a normal consumer account such as `anticaptrad@gmail.com`.

## Publishing safety

To change a video's privacy through the JSON API:

```json
{
  "action": "publishVideo",
  "apiKey": "acp_...",
  "videoId": "VIDEO_ID",
  "privacyStatus": "public",
  "confirmation": "PUBLISH VIDEO_ID AS PUBLIC"
}
```

Before that succeeds, **Enable unlisted/public transitions** must be checked in Settings. The phrase must match exactly.

## Gmail ingestion

Create a Gmail label named `anticaptrad-video-inbox`, apply it to messages containing small video attachments, then use the **Gmail / Drive** tab. Imported attachments are deduplicated and saved in `01 Inbox`.

For normal video production, upload source media directly to Drive rather than emailing it.

## Validation and clasp diagnostics

```bash
npm run check          # static checks plus 14 mocked clasp workflow tests
npm run preflight      # verifies target, active profile, and exact push file set
npm run status         # raw clasp JSON file status
npm run deployments    # list versioned deployments
npm run versions       # list immutable script versions
```

The workflow tests cover target binding, foreign-target refusal, profile drift, exact push-set checks, backup integrity manifests, backup failure blocking, guarded force-push ordering, explicit backup bypass, project-creation rollback and isolation, and deployment-ID/profile pinning.

## Security notes

- `.clasprc.json` is an OAuth credential and must never be shared or committed. `.clasp.json` is generated locally by `npm run bind`, kept mode `0600`, and ignored by Git to reduce accidental cross-project pushes.
- Never commit OAuth tokens, Google service-account keys, API keys, or Content Owner credentials.
- The web app executes as the deployer, so its deployment access setting is a security boundary.
- Do not use the Partner profile unless the account is actually authorized.
- Destructive video deletion is intentionally not implemented.
- `clasp push` replaces the entire remote project and is not atomic per file; use the guarded workflow rather than invoking force push directly.

See [CLASP-WORKFLOW.md](CLASP-WORKFLOW.md) for the operational contract and recovery steps.
