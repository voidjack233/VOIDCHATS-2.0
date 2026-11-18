CREATE TABLE draft_reply_refs (
    conversation_id INTEGER PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    author_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL
) STRICT;
CREATE TABLE outgoing_reply_refs (
    event_id TEXT PRIMARY KEY REFERENCES outgoing_operations(event_id),
    author_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL
) STRICT;
CREATE TABLE outgoing_controls (
    event_id TEXT PRIMARY KEY CHECK(length(event_id)=36),
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    kind TEXT NOT NULL CHECK(kind IN ('reaction','delete_for_everyone')),
    author_void_id TEXT NOT NULL,
    original_event_id TEXT NOT NULL,
    emoji TEXT,
    active INTEGER CHECK(active IN (0,1)),
    revision INTEGER CHECK(revision>0),
    created_at INTEGER NOT NULL CHECK(created_at>=0),
    claim_id TEXT NOT NULL,
    payload BLOB CHECK(length(payload)>0),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','unconfirmed','failed','accepted')),
    CHECK((kind='reaction' AND emoji IS NOT NULL AND active IS NOT NULL AND revision IS NOT NULL)
       OR (kind='delete_for_everyone' AND emoji IS NULL AND active IS NULL AND revision IS NULL))
) STRICT;
CREATE INDEX outgoing_controls_pending ON outgoing_controls(state,created_at,event_id);
CREATE TABLE local_message_hides (
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    author_void_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    message_id TEXT,
    PRIMARY KEY(conversation_id,author_void_id,event_id)
) STRICT;
CREATE INDEX local_hidden_row ON local_message_hides(message_id);
