# Security policy

- Default deployment: execute as the deploying user; access only by that user.
- All uploads start private.
- Public transitions require a configuration switch and exact per-video confirmation phrase.
- HTTP API authentication uses a random key whose SHA-256 hash is stored in Script Properties.
- Source media is backed up to Drive before upload.
- The HTTP media-ingest action is API-key protected, mutation-correlated,
  SHA-256 verified, MIME allowlisted, idempotent, and capped at 8 MiB decoded.
- Request bodies and base64 media are redacted from returned errors and must
  not be written to logs, URLs, snapshots, traces, or CI artifacts.
- The authorized channel is pinned by channel ID after initial handle verification.
- Do not publish `.clasp.json`, `.clasprc.json`, OAuth tokens, Google service-account JSON, or API keys.
- The optional anonymous HTTP profile should only be used for a trusted server-to-server client and must be redeployed back to the default profile when no longer needed.
