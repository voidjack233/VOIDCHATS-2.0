-- Additive only. Existing account metadata, MLS checkpoints and history survive.
CREATE TABLE account_signing_key (
    slot INTEGER PRIMARY KEY CHECK (slot=1),
    seed BLOB NOT NULL CHECK (length(seed)=32),
    public_key BLOB NOT NULL CHECK (length(public_key)=32)
);
CREATE TABLE signal_threads (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id),
    peer_void_id TEXT NOT NULL UNIQUE,
    account_key_base64 TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('incoming','outgoing')),
    is_request INTEGER NOT NULL CHECK (is_request IN (0,1)),
    claim_id TEXT NOT NULL,
    last_error TEXT
);
CREATE TABLE signal_outbox (
    event_id TEXT PRIMARY KEY REFERENCES messages(id),
    peer_void_id TEXT NOT NULL,
    payload_base64 TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','failed','done')),
    error_code TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE signal_inbox (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id),
    PRIMARY KEY(sender_void_id,event_id)
);
CREATE TABLE signal_drafts (
    message_id TEXT PRIMARY KEY REFERENCES messages(id),
    conversation_id TEXT NOT NULL REFERENCES signal_threads(conversation_id)
);
CREATE TABLE signal_publication (
    slot INTEGER PRIMARY KEY CHECK (slot=1),
    publication_id TEXT NOT NULL,
    payload_base64 TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0 CHECK (published IN (0,1))
);
