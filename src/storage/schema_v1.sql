CREATE TABLE local_account (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    void_id TEXT NOT NULL,
    public_key BLOB NOT NULL CHECK (length(public_key) = 32)
) STRICT;

CREATE TABLE conversations (
    id INTEGER PRIMARY KEY,
    peer_void_id TEXT NOT NULL UNIQUE,
    display_name TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    last_activity_at INTEGER NOT NULL CHECK (last_activity_at >= created_at)
) STRICT;

CREATE INDEX conversations_activity ON conversations(last_activity_at DESC, id DESC);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    direction INTEGER NOT NULL CHECK (direction IN (0, 1)),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    delivery_state INTEGER NOT NULL CHECK (delivery_state IN (0, 1, 2, 3, 4, 5))
) STRICT;

CREATE INDEX messages_conversation ON messages(conversation_id, created_at, id);

CREATE TABLE contacts (
    void_id TEXT PRIMARY KEY,
    display_name TEXT,
    public_identity BLOB
) STRICT;

CREATE TABLE drafts (
    conversation_id INTEGER PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    body TEXT NOT NULL
) STRICT;

CREATE TABLE requests (
    conversation_id INTEGER PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE
) STRICT;
