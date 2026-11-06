-- An original send must explicitly request a receipt before one can update it.
ALTER TABLE signal_outbox ADD COLUMN receipt_requested INTEGER NOT NULL DEFAULT 0 CHECK (receipt_requested IN (0,1));

-- A ready obligation has its transport ID but no ciphertext yet. Pending
-- uploads retain the exact encrypted payload across attempts and restarts.
CREATE TABLE signal_control_outbox (
    event_id TEXT PRIMARY KEY NOT NULL CHECK (length(event_id)=36),
    peer_void_id TEXT NOT NULL,
    original_message_id TEXT NOT NULL CHECK (length(original_message_id)=36),
    payload_base64 TEXT CHECK (length(payload_base64) BETWEEN 1 AND 1398104),
    status TEXT NOT NULL DEFAULT 'ready' CHECK (status IN ('ready','pending','accepted','stopped')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts>=0),
    created_at INTEGER NOT NULL CHECK (created_at>=0),
    next_attempt_at INTEGER NOT NULL CHECK (next_attempt_at>=0),
    deadline_at INTEGER NOT NULL CHECK (deadline_at>created_at),
    UNIQUE (peer_void_id,original_message_id),
    FOREIGN KEY (peer_void_id,original_message_id) REFERENCES signal_inbox(sender_void_id,event_id),
    CHECK (event_id<>original_message_id),
    CHECK ((status='ready' AND payload_base64 IS NULL) OR
        (status='pending' AND payload_base64 IS NOT NULL) OR status IN ('accepted','stopped'))
);

-- Applied rows are the private proof of delivery. Ignored rows deduplicate
-- authenticated receipts with no eligible target, without creating messages.
CREATE TABLE signal_control_inbox (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL CHECK (length(event_id)=36),
    original_message_id TEXT NOT NULL CHECK (length(original_message_id)=36),
    payload_hash TEXT NOT NULL CHECK (length(payload_hash)=64),
    disposition TEXT NOT NULL CHECK (disposition IN ('applied','ignored')),
    recorded_at INTEGER NOT NULL CHECK (recorded_at>=0),
    PRIMARY KEY (sender_void_id,event_id),
    CHECK (event_id<>original_message_id)
);

CREATE UNIQUE INDEX signal_verified_delivery
ON signal_control_inbox(original_message_id) WHERE disposition='applied';
