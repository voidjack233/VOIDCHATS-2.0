-- VOID local storage migration 1. Executed only after a native SQLCipher key.
-- Native connection must enable foreign_keys before this migration.
-- Epoch millisecond timestamps; application records contain no private keys.
CREATE TABLE storage_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at INTEGER NOT NULL CHECK (applied_at >= 0)
);

CREATE TABLE local_identity (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    local_device_id TEXT NOT NULL UNIQUE CHECK (length(local_device_id) > 0),
    client_handle TEXT NOT NULL UNIQUE CHECK (length(client_handle) > 0),
    credential_identity_base64 TEXT NOT NULL CHECK (length(credential_identity_base64) > 0),
    signature_public_key_base64 TEXT NOT NULL CHECK (length(signature_public_key_base64) > 0),
    account_void_id TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

-- Serialized signers, providers, group secrets and replay state never cross JS.
CREATE TABLE native_mls_state (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    snapshot BLOB NOT NULL CHECK (length(snapshot) > 0)
);

-- Native cryptographic transaction journal; inaccessible to execute().
CREATE TABLE crypto_events (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    local_id TEXT,
    operation TEXT NOT NULL CHECK (length(operation) > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('ok', 'rejected')),
    group_id BLOB,
    plaintext BLOB,
    ciphertext BLOB,
    sender_identity BLOB,
    sender_signature_public_key BLOB,
    epoch TEXT CHECK (epoch IS NULL OR (length(epoch) BETWEEN 1 AND 20
        AND epoch NOT GLOB '*[^0-9]*'
        AND (length(epoch) < 20 OR epoch <= '18446744073709551615'))),
    checkpoint_revision INTEGER CHECK (checkpoint_revision IS NULL OR checkpoint_revision >= 1),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

-- Public KeyPackage bookkeeping only; private package state lives in the engine.
CREATE TABLE key_packages (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    client_handle TEXT NOT NULL REFERENCES local_identity(client_handle) ON DELETE CASCADE,
    package_base64 TEXT NOT NULL UNIQUE CHECK (length(package_base64) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER CHECK (expires_at IS NULL OR expires_at >= created_at),
    consumed_at INTEGER CHECK (consumed_at IS NULL OR consumed_at >= created_at)
);

CREATE TABLE conversations (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    kind TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    peer_void_id TEXT,
    mls_group_id TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    local_read_at INTEGER,
    local_read_message_id TEXT,
    CHECK ((kind = 'direct' AND peer_void_id IS NOT NULL AND length(peer_void_id) > 0)
        OR (kind = 'group' AND peer_void_id IS NULL)),
    CHECK ((local_read_at IS NULL AND local_read_message_id IS NULL)
        OR (local_read_at IS NOT NULL AND local_read_at >= 0 AND local_read_message_id IS NOT NULL))
);
CREATE UNIQUE INDEX conversations_direct_peer ON conversations(peer_void_id) WHERE kind = 'direct';
CREATE INDEX conversations_activity ON conversations(updated_at DESC, id DESC);

CREATE TABLE conversation_members (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    void_id TEXT NOT NULL CHECK (length(void_id) > 0),
    display_name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('member', 'admin')),
    joined_at INTEGER NOT NULL CHECK (joined_at >= 0),
    left_at INTEGER CHECK (left_at IS NULL OR left_at >= joined_at),
    PRIMARY KEY (conversation_id, void_id)
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_void_id TEXT NOT NULL CHECK (length(sender_void_id) > 0),
    body TEXT,
    direction TEXT NOT NULL CHECK (direction IN ('incoming', 'outgoing')),
    delivery_state TEXT NOT NULL CHECK (delivery_state IN ('received', 'pending', 'sent', 'delivered', 'read', 'failed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    edited_at INTEGER CHECK (edited_at IS NULL OR edited_at >= created_at),
    deleted_at INTEGER CHECK (deleted_at IS NULL OR deleted_at >= created_at),
    read_at INTEGER CHECK (read_at IS NULL OR read_at >= 0),
    CHECK ((deleted_at IS NULL AND body IS NOT NULL) OR (deleted_at IS NOT NULL AND body IS NULL)),
    CHECK ((direction = 'incoming' AND delivery_state = 'received')
        OR (direction = 'outgoing' AND delivery_state <> 'received'))
);
CREATE INDEX messages_page ON messages(conversation_id, created_at DESC, id DESC);
CREATE INDEX messages_unread ON messages(conversation_id, read_at) WHERE direction = 'incoming' AND deleted_at IS NULL;

CREATE TABLE message_reactions (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    reactor_void_id TEXT NOT NULL CHECK (length(reactor_void_id) > 0),
    emoji TEXT NOT NULL CHECK (length(emoji) > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (message_id, reactor_void_id, emoji)
);

CREATE TABLE message_receipts (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    recipient_void_id TEXT NOT NULL CHECK (length(recipient_void_id) > 0),
    state TEXT NOT NULL CHECK (state IN ('delivered', 'read')),
    recorded_at INTEGER NOT NULL CHECK (recorded_at >= 0),
    PRIMARY KEY (message_id, recipient_void_id)
);

CREATE TABLE contacts (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    void_id TEXT NOT NULL UNIQUE CHECK (length(void_id) > 0),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    identity_public_key TEXT NOT NULL CHECK (length(identity_public_key) > 0),
    trust_state TEXT NOT NULL DEFAULT 'unverified' CHECK (trust_state IN ('unverified', 'changed')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
);
CREATE INDEX contacts_name ON contacts(display_name COLLATE NOCASE, id);

CREATE TABLE message_requests (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    from_void_id TEXT NOT NULL CHECK (length(from_void_id) > 0),
    display_name TEXT NOT NULL,
    identity_public_key TEXT NOT NULL CHECK (length(identity_public_key) > 0),
    preview TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'declined', 'blocked')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
);
CREATE INDEX message_requests_status ON message_requests(status, created_at DESC, id DESC);

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY NOT NULL CHECK (key IN ('theme', 'notifications_enabled', 'read_receipts_enabled')),
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

-- A stable message identifier cannot be repointed to another sender/conversation.
CREATE TRIGGER messages_identity_immutable
BEFORE UPDATE OF id, conversation_id, sender_void_id, direction, created_at ON messages
WHEN NEW.id <> OLD.id OR NEW.conversation_id <> OLD.conversation_id
    OR NEW.sender_void_id <> OLD.sender_void_id OR NEW.direction <> OLD.direction
    OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'message_identity_immutable');
END;

CREATE TRIGGER messages_no_resurrection
BEFORE UPDATE ON messages
WHEN OLD.deleted_at IS NOT NULL AND (NEW.deleted_at IS NULL OR NEW.body IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'message_deleted');
END;

CREATE TRIGGER messages_delivery_transition
BEFORE UPDATE OF delivery_state ON messages
WHEN NEW.delivery_state <> OLD.delivery_state AND NOT (
    (OLD.delivery_state = 'pending' AND NEW.delivery_state IN ('sent', 'failed'))
    OR (OLD.delivery_state = 'failed' AND NEW.delivery_state IN ('pending', 'sent'))
    OR (OLD.delivery_state = 'sent' AND NEW.delivery_state IN ('delivered', 'read', 'failed'))
    OR (OLD.delivery_state = 'delivered' AND NEW.delivery_state = 'read')
)
BEGIN
    SELECT RAISE(ABORT, 'invalid_delivery_transition');
END;

CREATE TRIGGER messages_clear_deleted_reactions
AFTER UPDATE OF deleted_at ON messages
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM message_reactions WHERE message_id = NEW.id;
END;

CREATE TRIGGER reactions_require_visible_message
BEFORE INSERT ON message_reactions
WHEN EXISTS (SELECT 1 FROM messages WHERE id = NEW.message_id AND deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'message_deleted');
END;

CREATE TRIGGER receipts_no_regression
BEFORE UPDATE OF state ON message_receipts
WHEN OLD.state = 'read' AND NEW.state <> 'read'
BEGIN
    SELECT RAISE(ABORT, 'receipt_regression');
END;

CREATE TRIGGER contacts_key_change_unverified
BEFORE UPDATE OF identity_public_key ON contacts
WHEN NEW.identity_public_key <> OLD.identity_public_key AND NEW.trust_state <> 'changed'
BEGIN
    SELECT RAISE(ABORT, 'contact_key_changed');
END;

CREATE TRIGGER requests_status_transition
BEFORE UPDATE OF status ON message_requests
WHEN NEW.status <> OLD.status AND NOT (
    (OLD.status = 'pending' AND NEW.status IN ('accepted', 'declined', 'blocked'))
    OR (OLD.status IN ('accepted', 'declined') AND NEW.status = 'blocked')
)
BEGIN
    SELECT RAISE(ABORT, 'invalid_request_transition');
END;

