use super::{Database, Error, Result};
use rusqlite::{OptionalExtension, Row, Transaction, params};

// Durable evidence, including hidden/deleted history, establishes a relationship.
// A draft or an empty conversation created by entering an ID is not evidence.
const TWO_WAY: &str =
    "EXISTS (SELECT 1 FROM inbox i WHERE i.sender_void_id=c.peer_void_id)
    AND (EXISTS (SELECT 1 FROM outgoing_operations o WHERE o.conversation_id=c.id)
         OR EXISTS (SELECT 1 FROM messages m JOIN outbox b ON b.message_id=m.id WHERE m.conversation_id=c.id))";

pub(super) fn establish_contacts(tx: &Transaction<'_>, conversation: Option<i64>) -> Result<()> {
    tx.execute(
        &format!(
            "INSERT INTO contacts (void_id,display_name)
        SELECT c.peer_void_id,c.display_name FROM conversations c
        WHERE (?1 IS NULL OR c.id=?1) AND ({TWO_WAY})
        ON CONFLICT(void_id) DO NOTHING"
        ),
        [conversation],
    )?;
    tx.execute(
        &format!(
            "DELETE FROM requests WHERE conversation_id IN (
        SELECT c.id FROM conversations c WHERE (?1 IS NULL OR c.id=?1) AND ({TWO_WAY}))"
        ),
        [conversation],
    )?;
    Ok(())
}

pub(crate) struct Conversation {
    pub id: i64,
    pub peer_void_id: String,
    pub display_name: Option<String>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub is_request: bool,
    pub unread_through: Option<i64>,
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
pub(crate) enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub(crate) enum DeliveryState {
    Received,
    Pending,
    Sent,
    Delivered,
    Read,
    Failed,
}

impl Direction {
    fn code(self) -> i64 {
        match self {
            Self::Incoming => 0,
            Self::Outgoing => 1,
        }
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
    pub(super) fn code(self) -> i64 {
        match self {
            Self::Received => 0,
            Self::Pending => 1,
            Self::Sent => 2,
            Self::Delivered => 3,
            Self::Read => 4,
            Self::Failed => 5,
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

    pub(crate) fn update_conversation(
        &self,
        id: i64,
        name: Option<&str>,
        activity: i64,
    ) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE conversations SET display_name = ?2, last_activity_at = max(last_activity_at, ?3)
             WHERE id = ?1", params![id, name, activity])? == 1)
    }

    pub(crate) fn conversations(&self, requests: bool) -> Result<Vec<Conversation>> {
        let mut query = self.connection.prepare(
            "SELECT c.id, c.peer_void_id, coalesce(contact.display_name, c.display_name),
                    c.created_at, c.last_activity_at, r.conversation_id IS NOT NULL,
                    (SELECT max(sequence) FROM unread_messages WHERE conversation_id = c.id)
             FROM conversations c
             LEFT JOIN contacts contact ON contact.void_id = c.peer_void_id
             LEFT JOIN requests r ON r.conversation_id = c.id
             WHERE (r.conversation_id IS NOT NULL) = ?1
             ORDER BY c.last_activity_at DESC, c.id DESC",
        )?;
        Ok(query
            .query_map([requests], conversation_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn conversation(&self, id: i64) -> Result<Conversation> {
        Ok(self.connection.query_row(
            "SELECT c.id, c.peer_void_id, coalesce(contact.display_name, c.display_name),
                    c.created_at, c.last_activity_at, r.conversation_id IS NOT NULL,
                    (SELECT max(sequence) FROM unread_messages WHERE conversation_id = c.id)
             FROM conversations c
             LEFT JOIN contacts contact ON contact.void_id = c.peer_void_id
             LEFT JOIN requests r ON r.conversation_id = c.id
             WHERE c.id = ?1",
            [id],
            conversation_row,
        )?)
    }

    pub(crate) fn set_request(&self, conversation_id: i64, is_request: bool) -> Result<()> {
        if is_request {
            self.connection.execute(
                "INSERT INTO requests (conversation_id) VALUES (?1) ON CONFLICT (conversation_id) DO NOTHING",
                [conversation_id])?;
        } else {
            self.connection.execute(
                "DELETE FROM requests WHERE conversation_id = ?1",
                [conversation_id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn mark_read(&self, conversation_id: i64, through: i64) -> Result<bool> {
        Ok(self.connection.execute(
            "DELETE FROM unread_messages WHERE conversation_id = ?1 AND sequence <= ?2",
            params![conversation_id, through],
        )? != 0)
    }

    pub(crate) fn save_message(&mut self, message: &Message) -> Result<()> {
        self.write(|tx| insert_message(tx, message))
    }

    pub(crate) fn messages(&self, conversation_id: i64) -> Result<Vec<Message>> {
        let mut query = self.connection.prepare(
            "SELECT id, conversation_id, direction, body, created_at, delivery_state
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at, id",
        )?;
        Ok(query
            .query_map([conversation_id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    direction: Direction::from_code(row.get(2)?)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                    delivery_state: DeliveryState::from_code(row.get(5)?)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn update_delivery(&self, message_id: &str, state: DeliveryState) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE messages SET delivery_state = ?2 WHERE id = ?1",
            params![message_id, state.code()],
        )? == 1)
    }

    pub(crate) fn save_contact(&self, contact: &Contact) -> Result<()> {
        self.connection.execute(
            "INSERT INTO contacts (void_id, display_name, public_identity) VALUES (?1, ?2, ?3)
             ON CONFLICT (void_id) DO UPDATE
             SET display_name = excluded.display_name, public_identity = excluded.public_identity",
            params![
                contact.void_id,
                contact.display_name,
                contact.public_identity
            ],
        )?;
        Ok(())
    }

    pub(crate) fn contacts(&self) -> Result<Vec<Contact>> {
        let mut query = self.connection.prepare(
            "SELECT void_id, display_name, public_identity FROM contacts ORDER BY display_name COLLATE NOCASE, void_id")?;
        Ok(query
            .query_map([], |row| {
                Ok(Contact {
                    void_id: row.get(0)?,
                    display_name: row.get(1)?,
                    public_identity: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn contact(&self, peer: &str) -> Result<Option<Contact>> {
        Ok(self
            .connection
            .query_row(
                "SELECT void_id,display_name,public_identity FROM contacts WHERE void_id=?1",
                [peer],
                |row| {
                    Ok(Contact {
                        void_id: row.get(0)?,
                        display_name: row.get(1)?,
                        public_identity: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub(crate) fn rename_contact(&mut self, peer: &str, name: &str) -> Result<bool> {
        let name = name.trim();
        if name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(Error::InvalidContactName);
        }
        self.write(|tx| {
            let changed = tx.execute(
                "UPDATE contacts SET display_name=?2 WHERE void_id=?1",
                params![peer, (!name.is_empty()).then_some(name)],
            )? != 0;
            if changed {
                // Contacts own local names. Clearing a nickname must not revive
                // an older conversation alias through the fallback query.
                tx.execute(
                    "UPDATE conversations SET display_name=NULL WHERE peer_void_id=?1",
                    [peer],
                )?;
            }
            Ok(changed)
        })
    }

    pub(crate) fn save_draft(&self, conversation_id: i64, body: &str) -> Result<()> {
        if body.is_empty() {
            self.connection.execute(
                "DELETE FROM drafts WHERE conversation_id = ?1",
                [conversation_id],
            )?;
        } else {
            self.connection.execute(
                "INSERT INTO drafts (conversation_id, body) VALUES (?1, ?2)
                 ON CONFLICT (conversation_id) DO UPDATE SET body = excluded.body",
                params![conversation_id, body],
            )?;
        }
        Ok(())
    }

    pub(crate) fn draft(&self, conversation_id: i64) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT body FROM drafts WHERE conversation_id = ?1",
                [conversation_id],
                |row| row.get(0),
            )
            .optional()?)
    }
}

pub(super) fn insert_message(tx: &Transaction<'_>, message: &Message) -> Result<()> {
    tx.execute(
        "INSERT INTO messages (id, conversation_id, direction, body, created_at, delivery_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            message.id,
            message.conversation_id,
            message.direction.code(),
            message.body,
            message.created_at,
            message.delivery_state.code()
        ],
    )?;
    tx.execute(
        "UPDATE conversations SET last_activity_at = max(last_activity_at, ?2) WHERE id = ?1",
        params![message.conversation_id, message.created_at],
    )?;
    Ok(())
}

fn conversation_row(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        peer_void_id: row.get(1)?,
        display_name: row.get(2)?,
        created_at: row.get(3)?,
        last_activity_at: row.get(4)?,
        is_request: row.get(5)?,
        unread_through: row.get(6)?,
    })
}
