-- Applied only by the serialized migration job. The API runtime must not run DDL.
-- The runtime role receives SELECT/INSERT/UPDATE on these three tables only.

CREATE TABLE IF NOT EXISTS act_operation_inbox (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) BETWEEN 1 AND 128),
    subject TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 256),
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL CHECK (status IN ('processing', 'completed')),
    result_json TEXT CHECK (result_json IS NULL OR octet_length(result_json) <= 65536),
    received_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    CHECK (
        (status = 'processing' AND completed_at IS NULL)
        OR (status = 'completed' AND completed_at IS NOT NULL)
    )
);

CREATE TABLE IF NOT EXISTS act_operation_status (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) BETWEEN 1 AND 128),
    subject TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 256),
    status TEXT NOT NULL CHECK (status IN ('processing', 'completed')),
    result_json TEXT CHECK (result_json IS NULL OR octet_length(result_json) <= 65536),
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS act_operation_outbox (
    event_id TEXT PRIMARY KEY CHECK (length(event_id) BETWEEN 1 AND 128),
    operation_id TEXT NOT NULL REFERENCES act_operation_inbox(operation_id),
    subject TEXT NOT NULL CHECK (subject = 'act.results.api.v1'),
    payload_json TEXT NOT NULL CHECK (octet_length(payload_json) <= 65536),
    created_at TIMESTAMPTZ NOT NULL,
    delivered_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS act_operation_outbox_pending_idx
    ON act_operation_outbox (created_at)
    WHERE delivered_at IS NULL;
