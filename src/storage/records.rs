use super::{Database, Result};
use rusqlite::{OptionalExtension, Row, params};

pub(crate) struct Conversation {
    pub id: i64,
    pub peer_void_id: String,
    pub display_name: Option<String>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub is_request: bool,
}

pub(crate) struct Contact {
    pub void_id: String,
    pub display_name: Option<String>,
    pub public_identity: Option<Vec<u8>>,
}

pub(crate) struct Message {
    pub id: String,
    pub conversation_id: i64,
    pub direction: Direction,
    pub body: String,
    pub created_at: i64,
    pub delivery_state: DeliveryState,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub(crate) enum Direction { Incoming, Outgoing }

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub(crate) enum DeliveryState { Received, Pending, Sent, Delivered, Read, Failed }

impl Direction {
    fn code(self) -> i64 {
        match self { Self::Incoming => 0, Self::Outgoing => 1 }
    }

    fn from_code(code: i64) -> rusqlite::Result<Self> {
        match code {
            0 => Ok(Self::Incoming),
            1 => Ok(Self::Outgoing),
            other => Err(rusqlite::Error::IntegralValueOutOfRange(2, other)),
        }
    }
}

impl DeliveryState {
    fn code(self) -> i64 {
        match self {
            Self::Received => 0, Self::Pending => 1, Self::Sent => 2,
            Self::Delivered => 3, Self::Read => 4, Self::Failed => 5,
        }
    }

    fn from_code(code: i64) -> rusqlite::Result<Self> {
        match code {
            0 => Ok(Self::Received),
            1 => Ok(Self::Pending),
            2 => Ok(Self::Sent),
            3 => Ok(Self::Delivered),
            4 => Ok(Self::Read),
            5 => Ok(Self::Failed),
            other => Err(rusqlite::Error::IntegralValueOutOfRange(5, other)),
        }
    }
}

// Message ingestion and delivery updates will be called by a later messaging phase.
#[allow(dead_code)]
impl Database {
    pub(crate) fn create_conversation(&self, peer: &str, created_at: i64) -> Result<i64> {
        let id = self.connection.query_row(
            "INSERT INTO conversations (peer_void_id, created_at, last_activity_at) VALUES (?1, ?2, ?2)
             ON CONFLICT (peer_void_id) DO UPDATE SET peer_void_id = excluded.peer_void_id
             RETURNING id", params![peer, created_at], |row| row.get(0))?;
        Ok(id)
    }

    pub(crate) fn update_conversation(&self, id: i64, name: Option<&str>, activity: i64) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE conversations SET display_name = ?2, last_activity_at = max(last_activity_at, ?3)
             WHERE id = ?1", params![id, name, activity])? == 1)
    }

    pub(crate) fn conversations(&self, requests: bool) -> Result<Vec<Conversation>> {
        let mut query = self.connection.prepare(
            "SELECT c.id, c.peer_void_id, coalesce(c.display_name, contact.display_name),
                    c.created_at, c.last_activity_at, r.conversation_id IS NOT NULL
             FROM conversations c
             LEFT JOIN contacts contact ON contact.void_id = c.peer_void_id
             LEFT JOIN requests r ON r.conversation_id = c.id
             WHERE (r.conversation_id IS NOT NULL) = ?1
             ORDER BY c.last_activity_at DESC, c.id DESC")?;
        Ok(query.query_map([requests], conversation_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn conversation(&self, id: i64) -> Result<Conversation> {
        Ok(self.connection.query_row(
            "SELECT c.id, c.peer_void_id, coalesce(c.display_name, contact.display_name),
                    c.created_at, c.last_activity_at, r.conversation_id IS NOT NULL
             FROM conversations c
             LEFT JOIN contacts contact ON contact.void_id = c.peer_void_id
             LEFT JOIN requests r ON r.conversation_id = c.id
             WHERE c.id = ?1", [id], conversation_row)?)
    }

    pub(crate) fn set_request(&self, conversation_id: i64, is_request: bool) -> Result<()> {
        if is_request {
            self.connection.execute(
                "INSERT INTO requests (conversation_id) VALUES (?1) ON CONFLICT (conversation_id) DO NOTHING",
                [conversation_id])?;
        } else {
            self.connection.execute("DELETE FROM requests WHERE conversation_id = ?1", [conversation_id])?;
        }
        Ok(())
    }

    pub(crate) fn save_message(&mut self, message: &Message) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO messages (id, conversation_id, direction, body, created_at, delivery_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![message.id, message.conversation_id, message.direction.code(),
                message.body, message.created_at, message.delivery_state.code()])?;
        tx.execute(
            "UPDATE conversations SET last_activity_at = max(last_activity_at, ?2) WHERE id = ?1",
            params![message.conversation_id, message.created_at])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn messages(&self, conversation_id: i64) -> Result<Vec<Message>> {
        let mut query = self.connection.prepare(
            "SELECT id, conversation_id, direction, body, created_at, delivery_state
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at, id")?;
        Ok(query.query_map([conversation_id], |row| Ok(Message {
            id: row.get(0)?, conversation_id: row.get(1)?, direction: Direction::from_code(row.get(2)?)?,
            body: row.get(3)?, created_at: row.get(4)?, delivery_state: DeliveryState::from_code(row.get(5)?)?,
        }))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn update_delivery(&self, message_id: &str, state: DeliveryState) -> Result<bool> {
        Ok(self.connection.execute("UPDATE messages SET delivery_state = ?2 WHERE id = ?1",
            params![message_id, state.code()])? == 1)
    }

    pub(crate) fn save_contact(&self, contact: &Contact) -> Result<()> {
        self.connection.execute(
            "INSERT INTO contacts (void_id, display_name, public_identity) VALUES (?1, ?2, ?3)
             ON CONFLICT (void_id) DO UPDATE
             SET display_name = excluded.display_name, public_identity = excluded.public_identity",
            params![contact.void_id, contact.display_name, contact.public_identity])?;
        Ok(())
    }

    pub(crate) fn contacts(&self) -> Result<Vec<Contact>> {
        let mut query = self.connection.prepare(
            "SELECT void_id, display_name, public_identity FROM contacts ORDER BY display_name COLLATE NOCASE, void_id")?;
        Ok(query.query_map([], |row| Ok(Contact {
            void_id: row.get(0)?, display_name: row.get(1)?, public_identity: row.get(2)?,
        }))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn save_draft(&self, conversation_id: i64, body: &str) -> Result<()> {
        if body.is_empty() {
            self.connection.execute("DELETE FROM drafts WHERE conversation_id = ?1", [conversation_id])?;
        } else {
            self.connection.execute(
                "INSERT INTO drafts (conversation_id, body) VALUES (?1, ?2)
                 ON CONFLICT (conversation_id) DO UPDATE SET body = excluded.body",
                params![conversation_id, body])?;
        }
        Ok(())
    }

    pub(crate) fn draft(&self, conversation_id: i64) -> Result<Option<String>> {
        Ok(self.connection.query_row("SELECT body FROM drafts WHERE conversation_id = ?1",
            [conversation_id], |row| row.get(0)).optional()?)
    }
}

fn conversation_row(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?, peer_void_id: row.get(1)?, display_name: row.get(2)?,
        created_at: row.get(3)?, last_activity_at: row.get(4)?, is_request: row.get(5)?,
    })
}
