-- Original references deliberately do not reference messages: authenticated
-- controls can arrive before their targets, or after local history removal.
CREATE TABLE message_reply_refs (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    original_author_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL CHECK (length(original_event_id) = 36)
) STRICT;

CREATE TABLE message_reactions (
    peer_void_id TEXT NOT NULL,
    original_author_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL CHECK (length(original_event_id) = 36),
    reactor_void_id TEXT NOT NULL,
    emoji TEXT NOT NULL CHECK (length(emoji) BETWEEN 1 AND 16 AND length(CAST(emoji AS BLOB)) <= 64),
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    revision INTEGER NOT NULL CHECK (revision > 0),
    source_event_id TEXT NOT NULL CHECK (length(source_event_id) = 36),
    PRIMARY KEY (peer_void_id, original_author_void_id, original_event_id, reactor_void_id, emoji)
) STRICT;

CREATE TABLE message_tombstones (
    peer_void_id TEXT NOT NULL,
    original_author_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL CHECK (length(original_event_id) = 36),
    source_event_id TEXT NOT NULL CHECK (length(source_event_id) = 36),
    PRIMARY KEY (peer_void_id, original_author_void_id, original_event_id)
) STRICT;

CREATE TABLE control_event_inbox (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL CHECK (length(event_id) = 36),
    payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
    kind TEXT NOT NULL CHECK (kind IN ('reaction', 'delete_for_everyone', 'deferred')),
    PRIMARY KEY (sender_void_id, event_id)
) STRICT;

-- Decrypted content remains encrypted at rest by the existing SQLCipher key.
-- Keep exact bytes for a future compatible receiver; never substitute a bubble.
CREATE TABLE deferred_signal_events (
    sender_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    plaintext BLOB NOT NULL CHECK (length(plaintext) > 0),
    reason TEXT NOT NULL,
    min_protocol_version TEXT NOT NULL,
    received_at INTEGER NOT NULL CHECK (received_at >= 0),
    PRIMARY KEY (sender_void_id, event_id),
    FOREIGN KEY (sender_void_id, event_id)
        REFERENCES control_event_inbox(sender_void_id, event_id)
) STRICT;
