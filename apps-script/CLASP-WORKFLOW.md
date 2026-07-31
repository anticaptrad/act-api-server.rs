# Clasp workflow and safety contract

## Assessment

The original package used the correct `@google/clasp` 3.x command names, pinned version 3.3.0, used `rootDir: src`, created standalone projects, and correctly recognized that Apps Script web-app behavior is selected at deployment time. Those choices were sound.

The unsafe part was operational rather than syntactic: `npm run push` directly executed `clasp push --force`. Apps Script replaces the complete remote project rather than updating files atomically, so an incorrect target, profile drift, ignore-rule error, or failed manual backup could overwrite the project.

## Approved workflow

```bash
npm ci
npm run check
npm run login
npm run auth:status
npm run bind
npm run preflight
npm run push
```

The default push performs these gates in order:

1. Validate the exact Script ID, `rootDir`, and deterministic `filePushOrder`.
2. Verify that `src/appsscript.json` exactly matches the active profile and that `PUBLIC_HTTP` is consistent.
3. Parse `clasp show-file-status --json` and compare its `filesToPush` with the accepted files under `src/`.
4. Clone the remote project into a timestamped ignored backup directory.
5. Write `backup-manifest.json` with clasp version, byte sizes, and SHA-256 for every backed-up file.
6. Run the full-project force push.
7. Write an ignored `.clasp-last-push.json` receipt with profile, backup path, and source hashes.

A failure in steps 1–5 prevents step 6.

## Public HTTP deployment

```bash
npm run profile:http-api
npm run deploy:http-api
```

This command requires the active `http-api` profile, performs the guarded push, and updates only deployment:

```text
AKfycbwXNUnFogkqg_aeobBMLCas21CHJ8eIR8W1AnmEBNx7pPgfio8eARW5J4A-lu_V5gY
```

It records `.clasp-last-deployment.json`. The file is local operational evidence and is ignored by Git.

## Commands that require extra care

- `npm run push:unsafe` invokes raw `clasp push --force`. It exists for recovery only.
- `node scripts/push-safe.mjs --skip-backup --acknowledge-no-backup` bypasses backup with an explicit acknowledgement.
- `npm run create:new` creates a separate standalone project and refuses to run while `.clasp.json` exists.
- `clasp pull` is intentionally not wrapped because it mutates local source. Use `npm run backup:remote` for inspection and recovery instead.

## Authentication

Use the pinned local binary through npm scripts:

```bash
npm run login
npm run auth:status
```

For terminals that cannot receive a localhost OAuth callback:

```bash
npm run login:no-localhost
```

The OAuth credential in `~/.clasprc.json` is sensitive. Never copy it into the repository, Linear, logs, chat, or build artifacts. Named `--user` profiles or a custom internal OAuth client can be added later if multiple Google operators require isolation.

## Recovery

1. Stop deployments and mutations.
2. Inspect the latest `backups/remote-before-push-*/backup-manifest.json`.
3. Verify file hashes before restoring.
4. Bind to the approved Script ID and run preflight.
5. Restore from the selected backup only after comparing it with reviewed source.
6. Create a new immutable deployment version rather than silently editing the Apps Script editor.
