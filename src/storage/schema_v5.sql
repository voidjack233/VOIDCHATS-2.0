CREATE TABLE signal_publication (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    publication_id TEXT NOT NULL UNIQUE,
    payload_base64 TEXT NOT NULL,
    confirmed INTEGER NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1))
) STRICT;

CREATE TABLE outgoing_operations (
    event_id TEXT PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    claim_id TEXT UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('pending', 'failed', 'unconfirmed', 'sent'))
) STRICT;

CREATE INDEX outgoing_operations_conversation ON outgoing_operations(conversation_id, created_at, event_id);

-- Preserve ciphertext already committed by older builds, including its local message metadata.
INSERT INTO outgoing_operations (event_id, conversation_id, body, created_at, state)
SELECT o.event_id, m.conversation_id, m.body, m.created_at,
       CASE WHEN m.delivery_state IN (2, 3, 4) THEN 'sent' ELSE 'unconfirmed' END
FROM outbox o JOIN messages m ON m.id = o.message_id;
