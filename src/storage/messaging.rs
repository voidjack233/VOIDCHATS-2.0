use super::{Database, DeliveryState, Error, Result};
use rusqlite::{OptionalExtension, params};

pub(crate) fn new_event_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Unavailable)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub(crate) struct OutgoingOperation {
    pub(crate) conversation_id: i64,
    pub(crate) peer: String,
    pub(crate) body: String,
    pub(crate) created_at: i64,
    pub(crate) state: String,
}

pub(crate) struct CommittedOutgoing {
    pub(crate) conversation_id: i64,
    pub(crate) body: String,
    pub(crate) created_at: i64,
    pub(crate) payload: Vec<u8>,
}

#[derive(Clone, Copy)]
pub(crate) enum SendState {
    Failed,
    Unconfirmed,
    Sent,
}

impl SendState {
    fn code(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Unconfirmed => "unconfirmed",
            Self::Sent => "sent",
        }
    }
}

pub(crate) struct TimelineMessage {
    pub(crate) body: String,
    pub(crate) created_at: i64,
    pub(crate) outgoing: bool,
    pub(crate) status: &'static str,
    pub(crate) retry_event: String,
}

impl Database {
    pub(crate) fn recover_interrupted_sends(&self) -> Result<()> {
        self.connection.execute(
            "UPDATE outgoing_operations SET state = CASE
               WHEN EXISTS (SELECT 1 FROM outbox WHERE outbox.event_id = outgoing_operations.event_id)
               THEN 'unconfirmed' ELSE 'failed' END WHERE state = 'pending'", [],
        )?;
        Ok(())
    }

    /// Move the submitted draft into a durable operation before lookup or network transmission.
    pub(crate) fn queue_outgoing(
        &mut self,
        conversation_id: i64,
        body: &str,
        created_at: i64,
    ) -> Result<String> {
        if !super::signal::valid_content(body, created_at) {
            return Err(Error::SignalEnvelope);
        }
        let event_id = new_event_id()?;
        self.write(|tx| {
            tx.execute("INSERT INTO outgoing_operations (event_id, conversation_id, body, created_at, state)
                VALUES (?1, ?2, ?3, ?4, 'pending')", params![event_id, conversation_id, body, created_at])?;
            tx.execute("DELETE FROM drafts WHERE conversation_id = ?1 AND body = ?2", params![conversation_id, body])?;
            tx.execute("UPDATE conversations SET last_activity_at = max(last_activity_at, ?2) WHERE id = ?1",
                params![conversation_id, created_at])?;
            Ok(())
        })?;
        Ok(event_id)
    }

    pub(crate) fn outgoing_operation(&self, id: &str) -> Result<OutgoingOperation> {
        Ok(self.connection.query_row(
            "SELECT o.conversation_id, c.peer_void_id, o.body, o.created_at, o.state
             FROM outgoing_operations o JOIN conversations c ON c.id = o.conversation_id WHERE o.event_id = ?1", [id],
            |row| Ok(OutgoingOperation { conversation_id: row.get(0)?, peer: row.get(1)?, body: row.get(2)?,
                created_at: row.get(3)?, state: row.get(4)? }),
        )?)
    }

    pub(crate) fn committed_outgoing(&self, id: &str) -> Result<Option<CommittedOutgoing>> {
        Ok(self
            .connection
            .query_row(
                "SELECT m.conversation_id, m.body, m.created_at, o.payload FROM outbox o
             JOIN messages m ON m.id = o.message_id WHERE o.event_id = ?1 AND m.direction = 1",
                [id],
                |row| {
                    Ok(CommittedOutgoing {
                        conversation_id: row.get(0)?,
                        body: row.get(1)?,
                        created_at: row.get(2)?,
                        payload: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub(crate) fn outgoing_claim(&self, event_id: &str) -> Result<String> {
        let id: Option<String> = self.connection.query_row(
            "SELECT claim_id FROM outgoing_operations WHERE event_id = ?1",
            [event_id],
            |row| row.get(0),
        )?;
        if let Some(id) = id {
            return Ok(id);
        }
        let id = new_event_id()?;
        self.connection.execute(
            "UPDATE outgoing_operations SET claim_id = ?2 WHERE event_id = ?1",
            params![event_id, id],
        )?;
        Ok(id)
    }

    pub(crate) fn clear_expired_claim(&self, event_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE outgoing_operations SET claim_id = NULL WHERE event_id = ?1",
            [event_id],
        )?;
        Ok(())
    }

    pub(crate) fn set_send_state(&mut self, event_id: &str, state: SendState) -> Result<()> {
        self.write(|tx| {
            let code = state.code();
            let changed = tx.execute(
                "UPDATE outgoing_operations SET state = ?2 WHERE event_id = ?1 AND state <> 'sent'",
                params![event_id, code],
            )?;
            if changed != 1 {
                return Err(Error::EventConflict);
            }
            // Unconfirmed writes stay Pending at the record level. This is an uploader status,
            // expressed separately from delivery state and only rendered in outgoing bubbles.
            let delivery = match state {
                SendState::Sent => DeliveryState::Sent,
                SendState::Failed => DeliveryState::Failed,
                SendState::Unconfirmed => DeliveryState::Pending,
            };
            let messages = tx.execute(
                "UPDATE messages SET delivery_state = ?2 WHERE id =
                (SELECT message_id FROM outbox WHERE event_id = ?1) AND direction = 1",
                params![event_id, delivery.code()],
            )?;
            if code == "sent" && messages != 1 {
                return Err(Error::EventConflict);
            }
            Ok(())
        })
    }

    pub(crate) fn conversation_timeline(
        &self,
        conversation_id: i64,
    ) -> Result<Vec<TimelineMessage>> {
        let mut query = self.connection.prepare(
            "SELECT body, created_at, outgoing, state, event_id FROM (
               SELECT m.body, m.created_at, m.direction AS outgoing, NULL AS state, NULL AS event_id, m.id AS sort_id
               FROM messages m WHERE m.conversation_id = ?1 AND NOT EXISTS (
                 SELECT 1 FROM outbox b JOIN outgoing_operations o ON o.event_id = b.event_id WHERE b.message_id = m.id)
               UNION ALL
               SELECT body, created_at, 1, state, event_id, event_id
               FROM outgoing_operations WHERE conversation_id = ?1
             ) ORDER BY created_at, sort_id",
        )?;
        let rows = query.query_map([conversation_id], |row| {
            let state: Option<String> = row.get(3)?;
            let status = match state.as_deref() {
                Some("pending") => "Pending",
                Some("sent") => "Sent to relay",
                Some("unconfirmed") => "Acceptance unconfirmed",
                Some("failed") => "Send failed",
                _ => "",
            };
            let retry_event = if matches!(state.as_deref(), Some("unconfirmed" | "failed")) {
                row.get(4)?
            } else {
                String::new()
            };
            Ok(TimelineMessage {
                body: row.get(0)?,
                created_at: row.get(1)?,
                outgoing: row.get(2)?,
                status,
                retry_event,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
