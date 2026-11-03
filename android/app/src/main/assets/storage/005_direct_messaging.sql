-- A direct message can be saved before the peer comes online. This migration
-- preserves all account/MLS bytes and upgrades the local transport bookkeeping.
ALTER TABLE messaging_sessions ADD COLUMN thread_id TEXT;
UPDATE messaging_sessions SET thread_id=id;
CREATE UNIQUE INDEX messaging_thread ON messaging_sessions(thread_id) WHERE thread_id IS NOT NULL;
ALTER TABLE messaging_sessions ADD COLUMN is_request INTEGER NOT NULL DEFAULT 0 CHECK (is_request IN (0,1));
UPDATE messaging_sessions SET is_request=1 WHERE direction='incoming' AND NOT EXISTS (
    SELECT 1 FROM messages WHERE conversation_id=messaging_sessions.id AND direction='outgoing'
);
ALTER TABLE messaging_sessions ADD COLUMN last_error TEXT;
ALTER TABLE messaging_operations ADD COLUMN purpose TEXT NOT NULL DEFAULT 'application' CHECK (purpose IN ('setup','application'));
UPDATE messaging_operations SET purpose='setup',kind='message' WHERE kind='relay';
-- Previous online forwarding could have completed before the recipient saved
-- the envelope. Queue the last unfinished stage once through durable delivery.
UPDATE messaging_operations SET status='pending',error_code=NULL WHERE id IN (
    SELECT (SELECT id FROM messaging_operations WHERE session_id=s.id AND purpose='setup' ORDER BY rowid DESC LIMIT 1)
    FROM messaging_sessions s WHERE s.status IN ('requested','offered','awaiting_welcome','awaiting_ready','awaiting_ack')
);
CREATE TABLE messaging_drafts (
    message_id TEXT PRIMARY KEY NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES messaging_sessions(id)
);
