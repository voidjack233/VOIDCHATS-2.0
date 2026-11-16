CREATE TABLE unread_messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX unread_messages_conversation ON unread_messages(conversation_id, sequence);

CREATE TRIGGER mark_incoming_message_unread
AFTER INSERT ON messages
WHEN NEW.direction = 0
BEGIN
    INSERT INTO unread_messages (message_id, conversation_id) VALUES (NEW.id, NEW.conversation_id);
END;
