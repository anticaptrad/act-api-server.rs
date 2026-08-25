# act-api-server.rs

HTTP API, NATS bridge, and guarded YouTube control plane for the AntiCapTrad platform.

## What it does

- Serves Kubernetes liveness and readiness probes.
- Connects to NATS without making broker availability a hard startup dependency.
- Exports traces through OTLP when configured.
- Emits Ores structured records through the existing tracing/OTLP pipeline.
- Introspects delegated product tokens through the official Shared Auth client.
- Calls the Anticaptrad Google Apps Script YouTube web app through a server-side API key.
- Keeps uploads private until a separate, exact-phrase publication request is approved.
- Emits redacted YouTube action lifecycle events to NATS.

## Configuration

Configuration comes only from the process environment. Do not add `dotenv` or commit credentials.

| Variable | Required | Default | Purpose |
| --- | --- | --- | --- |
| `PORT` | No | `8080` | HTTP listen port |
| `NATS_URL` | No | `nats://localhost:4222` | NATS endpoint |
| `OTEL_SERVICE_NAME` | No | `act-api-server` | OpenTelemetry service name |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | No | unset | OTLP collector endpoint |
| `SHARED_AUTH_URL` | For protected routes | unset/closed | Shared Auth base URL; public cleartext HTTP is rejected |
| `SHARED_AUTH_SERVICE_CREDENTIAL` | With Shared Auth URL | unset/closed | Independent service bearer used only for protected introspection |
| `YOUTUBE_GAS_URL` | For YouTube routes | unset | Deployed `https://script.google.com/macros/s/.../exec` URL |
| `YOUTUBE_GAS_API_KEY` | With GAS URL | unset | API key generated inside the GAS dashboard; minimum 32 characters |
| `YOUTUBE_EXPECTED_CHANNEL_HANDLE` | No | `@anticaptrad` | Channel identity expected by operators |
| `YOUTUBE_GAS_TIMEOUT_SECS` | No | `30` | Outbound timeout, bounded to 1–120 seconds |
| `YOUTUBE_GAS_MAX_RESPONSE_BYTES` | No | `4194304` | Response cap, bounded to 1 KiB–16 MiB |
| `YOUTUBE_ALLOW_PUBLIC_ACTIONS` | No | `false` | Defense-in-depth switch for public/unlisted publication |

When one of `YOUTUBE_GAS_URL` or `YOUTUBE_GAS_API_KEY` is set, both must be set and Shared Auth must also be configured. Invalid or non-Google URLs fail startup instead of silently weakening security.

Protected routes require a delegated Shared Auth token for audience `act-api`
and scope `youtube:admin`. The service credential authenticates the API server
to the introspection endpoint; it is not the user's token and is never
forwarded to Apps Script.

## GAS deployment requirement

The owner-only Apps Script dashboard cannot be called by a headless Rust pod because the pod has no Google browser session. For server-to-server calls:

1. Apply the GAS package's `http-api` manifest profile.
2. Rotate an API key in the owner-only dashboard and store it immediately in Kubernetes secrets.
3. Deploy a separate web-app version that permits external HTTP access.
4. Keep the browser RPC surface disabled in that profile.
5. Set the resulting `/exec` URL as `YOUTUBE_GAS_URL`.

Google ContentService redirects responses to a one-time `script.googleusercontent.com` URL. The Rust client follows only `script.google.com` and `script.googleusercontent.com`; a redirect to `accounts.google.com` is reported as `YOUTUBE_GAS_OWNER_ONLY`.

## HTTP API

### Process probes

```bash
curl --fail http://localhost:8080/health
curl --fail http://localhost:8080/ready
```

### End-to-end GAS health

This route exercises DNS, TLS, the Apps Script deployment, Google's redirect, and JSON parsing. It does not send the GAS API key.

```bash
curl --fail-with-body http://localhost:8080/v1/youtube/health
```

### Configuration status

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $PRODUCT_ACCESS_TOKEN" \
  http://localhost:8080/v1/youtube/status
```

The response confirms that keys are present but never returns either key or the full deployment URL.

### Read channel data

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $PRODUCT_ACCESS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{}' \
  http://localhost:8080/v1/youtube/actions/channel
```

Other read actions include `videos`, `analytics`, `jobs`, `partnerStatus`, `partnerOwners`, `partnerClaims`, `adminStatus`, and `workspaceUsers` when the corresponding GAS profile is authorized.

### Start a private upload

Every mutating action requires an `Idempotency-Key`. The Rust server injects the same value as the GAS upload idempotency key.

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $PRODUCT_ACCESS_TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: production-2026-07-27-video-001' \
  -d '{
    "driveFileId": "DRIVE_FILE_ID",
    "title": "Video title",
    "description": "Video description",
    "tags": ["anticaptrad", "economics"],
    "madeForKids": false,
    "rightsConfirmed": true,
    "repository": "https://github.com/anticaptrad/REPOSITORY",
    "commit": "GIT_COMMIT_SHA"
  }' \
  http://localhost:8080/v1/youtube/actions/startUpload
```

The control plane rejects attempts to set an upload public directly. Publication is always a separate operation.

### Publish after review

Both the Rust switch and the GAS dashboard switch must permit public actions. The confirmation phrase must match exactly.

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $PRODUCT_ACCESS_TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: publish-VIDEO_ID-public-v1' \
  -d '{
    "videoId": "VIDEO_ID",
    "privacyStatus": "public",
    "confirmation": "PUBLISH VIDEO_ID AS PUBLIC"
  }' \
  http://localhost:8080/v1/youtube/actions/publishVideo
```

## Local validation

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release

/path/to/zed validate

grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
```

## Security boundaries

- The user token, Shared Auth service credential, and `YOUTUBE_GAS_API_KEY` are distinct credentials.
- No credential belongs in Git, logs, NATS, URLs, browser storage, or error responses.
- The Rust API does not proxy GAS setup, API-key rotation, or configuration mutation actions.
- NATS events include only action metadata and selected identifiers; they omit descriptions, email content, tokens, resumable session URLs, and API keys.
- Public and unlisted actions remain disabled unless `YOUTUBE_ALLOW_PUBLIC_ACTIONS=true` and the GAS dashboard independently permits them.

## Web-to-API interaction modes

The public Rust contract in `src/web_data_plane.rs` supports four explicit
paths without treating them as interchangeable:

1. `direct_read_only_database` builds only actor-scoped, parameterized SeaORM
   `SELECT` statements and requires a separately provisioned `_web_ro` role, a
   read-only transaction, a statement deadline, and a row cap. It rejects every
   write operation.
2. `stateless_http` requires HTTPS except for an explicit in-cluster service
   address, disables redirects, keeps a separate service-credential reference,
   and bounds connection time, request time, and response bytes.
3. `stateful_mtls_tcp` requires certificate/key references and mutual TLS. Each
   operation is a strict four-byte length-prefixed frame with a size and
   deadline cap; the API must authenticate every operation rather than trusting
   connection age.
4. `jet_stream_async` requires a stable operation ID/deduplication key, durable
   consumer, explicit acknowledgements, bounded redelivery, acknowledgement
   wait, and publish deadline. Broker unavailability remains fail-soft for the
   ordinary HTTP control plane.

## Environment secrets

Secrets live in this repo **encrypted** with [sops](https://github.com/getsops/sops) + [age](https://github.com/FiloSottile/age):
`env/enc/<dev|prod>.env.enc` is committed; `just env-use <name>` decrypts it to
`env/dec/<name>.env` (gitignored, mode 0600) and symlinks `./.env` to it. The
Nix dev shell provides the tooling, `just env-audit` runs keyless in CI, and
containers decrypt at `docker run` — never at build. See [`env/README.md`](env/README.md).
