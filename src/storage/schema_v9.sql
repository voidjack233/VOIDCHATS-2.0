ALTER TABLE outbox ADD COLUMN receipt_requested INTEGER NOT NULL DEFAULT 0 CHECK (receipt_requested IN (0, 1));

CREATE TABLE receipt_outbox (
    event_id TEXT PRIMARY KEY CHECK (length(event_id) = 36),
    peer_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL,
    payload BLOB CHECK (length(payload) > 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at INTEGER NOT NULL CHECK (next_attempt_at >= 0),
    deadline_at INTEGER NOT NULL CHECK (deadline_at > 0),
    UNIQUE (peer_void_id, original_event_id),
    FOREIGN KEY (peer_void_id, original_event_id) REFERENCES inbox(sender_void_id, event_id),
    CHECK (event_id <> original_event_id)
) STRICT;

CREATE INDEX receipt_upload_due ON receipt_outbox(next_attempt_at);

CREATE TABLE receipt_inbox (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL CHECK (length(event_id) = 36),
    payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
    original_event_id TEXT NOT NULL REFERENCES outbox(event_id),
    PRIMARY KEY (sender_void_id, event_id),
    CHECK (event_id <> original_event_id)
) STRICT;

CREATE INDEX receipt_verified_original ON receipt_inbox(original_event_id);
