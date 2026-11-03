-- Local direct sessions and opaque WebSocket outbox. The server stores neither
-- conversations nor KeyPackage inventory. Secret state stays in native_mls_state.
CREATE TABLE messaging_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    peer_void_id TEXT NOT NULL,
    peer_key_base64 TEXT NOT NULL,
    peer_credential_base64 TEXT,
    direction TEXT NOT NULL CHECK (direction IN ('incoming','outgoing')),
    status TEXT NOT NULL CHECK (status IN ('requested','offered','awaiting_welcome','awaiting_ready','awaiting_ack','active','declined','blocked')),
    group_id_base64 TEXT NOT NULL UNIQUE,
    package_id TEXT REFERENCES key_packages(id),
    package_base64 TEXT,
    welcome_base64 TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);
CREATE UNIQUE INDEX messaging_open_peer ON messaging_sessions(peer_void_id) WHERE status NOT IN ('declined','blocked');

CREATE TABLE messaging_operations (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES messaging_sessions(id),
    kind TEXT NOT NULL CHECK (kind IN ('relay','message')),
    peer_void_id TEXT NOT NULL,
    payload_base64 TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','failed','done')),
    error_code TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);
CREATE INDEX messaging_pending ON messaging_operations(status,created_at,id);

-- Deduplicate authenticated online envelopes and replay the SAME response bytes.
CREATE TABLE messaging_relays (
    sender_void_id TEXT NOT NULL,
    exchange_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    response_operation_id TEXT REFERENCES messaging_operations(id),
    PRIMARY KEY(sender_void_id,exchange_id)
);

-- A staged row and its crypto_events plaintext journal commit with the receiving
-- ratchet. Visible history is materialized separately and can recover after a crash.
CREATE TABLE messaging_inbox (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    session_id TEXT,
    journal_id TEXT REFERENCES crypto_events(id),
    outcome TEXT NOT NULL CHECK (outcome IN ('staged','applied','rejected')),
    processed_at INTEGER NOT NULL CHECK (processed_at >= 0),
    PRIMARY KEY(sender_void_id,event_id)
);

CREATE TABLE messaging_blocks (
    peer_void_id TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);
