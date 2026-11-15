CREATE TABLE outbox (
    event_id TEXT PRIMARY KEY CHECK (length(event_id) > 0),
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id),
    payload BLOB NOT NULL CHECK (length(payload) > 0)
) STRICT;

CREATE TABLE inbox (
    sender_void_id TEXT NOT NULL CHECK (length(sender_void_id) > 0),
    event_id TEXT NOT NULL CHECK (length(event_id) > 0),
    payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id),
    PRIMARY KEY (sender_void_id, event_id)
) STRICT;
