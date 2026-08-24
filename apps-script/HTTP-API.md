# HTTP API reference

The safest deployment is the default owner-only dashboard. For unattended systems, apply the `http-api` profile and deploy a separate web-app version. That profile disables the browser RPC dashboard and accepts privileged operations only through JSON `POST` requests containing the API key.

## Response envelope

Success:

```json
{"ok": true, "data": {}}
```

Failure:

```json
{
  "ok": false,
  "error": {
    "message": "Human-readable message",
    "code": "MACHINE_CODE",
    "details": null
  }
}
```

## Health

`GET <WEB_APP_URL>?action=health` is the only supported GET action and requires no key.

## POST template

```bash
curl --fail-with-body \
  -H 'Content-Type: application/json' \
  -d '{"action":"channel","apiKey":"acp_REDACTED"}' \
  '<WEB_APP_URL>'
```

Never put the API key in the URL.

## Core actions

### `channel`

Returns the pinned channel identity and aggregate statistics.

### `videos`

```json
{"action":"videos","apiKey":"...","maxResults":25}
```

### `analytics`

```json
{
  "action":"analytics",
  "apiKey":"...",
  "startDate":"2026-07-01",
  "endDate":"2026-07-27",
  "dimensions":["day"],
  "metrics":["views","estimatedMinutesWatched","subscribersGained"],
  "maxResults":200
}
```

Use `exportAnalytics` with the same payload to save CSV and JSON copies in Drive.

### `ingestVideo`

Stores a small authenticated video payload in the Drive Inbox so a server can
exercise the complete bridge without a second set of Google Drive OAuth
credentials. The decoded file is limited to 8 MiB and must be MP4, WebM, or
QuickTime media. Standard base64 and the exact SHA-256 digest are required.

```json
{
  "action":"ingestVideo",
  "apiKey":"...",
  "controlRequestId":"youtube-e2e-ingest-SHA256",
  "fileName":"anticaptrad-e2e-preview.mp4",
  "mimeType":"video/mp4",
  "sha256":"64-lowercase-hex-characters",
  "base64":"BASE64_VIDEO_BYTES"
}
```

Retries must reuse the same `controlRequestId`. An exact replay returns the
existing Drive file; reuse with different bytes fails with
`IDEMPOTENCY_CONFLICT`. Request bodies and base64 content must never be logged,
placed in URLs, or included in CI artifacts.

### `startUpload`

```json
{
  "action":"startUpload",
  "apiKey":"...",
  "idempotencyKey":"production-run-2026-07-27-video-001",
  "driveFileId":"DRIVE_FILE_ID",
  "title":"Video title",
  "description":"Video description",
  "tags":["anticaptrad","economics"],
  "categoryId":"22",
  "defaultLanguage":"en",
  "madeForKids":false,
  "rightsConfirmed":true,
  "thumbnailDriveFileId":"OPTIONAL_DRIVE_FILE_ID",
  "playlistId":"OPTIONAL_PLAYLIST_ID",
  "repository":"https://github.com/anticaptrad/REPOSITORY",
  "commit":"GIT_COMMIT_SHA"
}
```

Reuse the same `idempotencyKey` when retrying an uncertain client request. The server returns the existing job instead of making another Drive backup or YouTube upload.

### `jobs`

Lists up to 50 upload jobs. Resumable session URLs are redacted.

### `processUpload`

```json
{"action":"processUpload","apiKey":"...","jobId":"JOB_ID","maxChunks":6}
```

### `processAllUploads`

Processes a bounded batch of pending jobs. A five-minute Apps Script trigger calls the same operation.

### `publishVideo`

```json
{
  "action":"publishVideo",
  "apiKey":"...",
  "videoId":"VIDEO_ID",
  "privacyStatus":"public",
  "confirmation":"PUBLISH VIDEO_ID AS PUBLIC"
}
```

Unlisted/public transitions must first be enabled in Settings. For a scheduled publication, keep `privacyStatus` as `private`, add a future ISO-8601 `publishAt`, and confirm `PUBLISH VIDEO_ID AS PRIVATE`.

### `updateVideo`

Accepts `videoId` plus any of `title`, `description`, `tags`, `categoryId`, `defaultLanguage`, and `madeForKids`.

### `createPlaylist`

Accepts `title`, optional `description`, `defaultLanguage`, and `privacyStatus`. Non-private playlists use the same publication safety switch.

### `addToPlaylist`

Accepts `playlistId`, `videoId`, and optional numeric `position`.

### `ingestGmail`

Accepts an optional Gmail query and `maxMessages`. Only video attachments up to 20 MB are copied to the Drive Inbox.

### `sendDigest`

Emails a channel and upload-queue digest to the configured notification address.

## Optional YouTube Partner profile

These actions exist only when the account has YouTube Content ID access and the `partner` manifest profile is deployed:

- `partnerStatus`
- `partnerOwners`
- `partnerClaims` with optional `id`, `videoId`, `assetId`, `q`, or `pageToken`

The project intentionally provides read-only discovery calls rather than automatic claim creation or release.

## Optional Workspace Admin profile

These actions exist only for an authorized Google Workspace administrator using the `workspace-admin` profile:

- `adminStatus`
- `workspaceUsers` with optional `maxResults`

A consumer `@gmail.com` account cannot administer a Workspace directory.


## HTTP security boundary (v1.1.0)

The HTTP surface is an explicit allowlist. `bootstrap`, `setup`, `saveConfig`, and
`rotateApiKey` are owner-dashboard-only and return `ACTION_NOT_AVAILABLE` over
HTTP, even with a valid API key. Every mutating HTTP request must include a
printable `controlRequestId` (the Rust service supplies its idempotency key), and
the Apps Script audit log records requested/completed/failed lifecycle events.

Anonymous callers can never render the dashboard or invoke `google.script.run`.
This is enforced by comparing the non-empty active user identity with the
effective deploying user, not only by trusting the selected manifest profile.
Returned error envelopes omit stack traces and redact body, token, authorization,
API-key, secret, content, and resumable-upload URL fields.
