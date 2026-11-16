use super::{
    Database, DeliveryState, Error, Result,
    account_status::{self, AccountStatus},
};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

fn outgoing_body(body: &str) -> Option<&str> {
    let first = body.find(|c: char| !c.is_whitespace())?;
    let last = body.rfind(|c: char| !c.is_whitespace())?;
    // Keep indentation and internal spacing. Drop only blank lines around the message.
    let start = body[..first]
        .rfind(['\r', '\n'])
        .map_or(0, |offset| offset + 1);
    let end = body[last..]
        .find(['\r', '\n'])
        .map_or(body.len(), |offset| last + offset);
    Some(&body[start..end])
}

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

pub(crate) struct MessagePreview {
    pub(crate) body: String,
    pub(crate) created_at: i64,
}

// A committed outgoing message and its queue entry represent the same item.
const STANDALONE_MESSAGE: &str =
    "NOT EXISTS (SELECT 1 FROM outbox b JOIN outgoing_operations o ON o.event_id = b.event_id
                 WHERE b.message_id = m.id)";

impl Database {
    pub(crate) fn recover_interrupted_sends(&self) -> Result<()> {
        self.connection.execute(
            "UPDATE outgoing_operations SET state = CASE
               WHEN EXISTS (SELECT 1 FROM outbox WHERE outbox.event_id = outgoing_operations.event_id)
               THEN 'unconfirmed' ELSE 'failed' END WHERE state = 'pending'
               AND NOT EXISTS (
                 SELECT 1 FROM conversations c JOIN peer_account_status a ON a.void_id = c.peer_void_id
                 WHERE c.id = outgoing_operations.conversation_id AND a.status = 'deleted'
               )", [],
        )?;
        Ok(())
    }

    /// Move the submitted draft into a durable operation and report whether it promoted a request.
    pub(crate) fn queue_outgoing(
        &mut self,
        conversation_id: i64,
        submitted: &str,
        created_at: i64,
    ) -> Result<(String, bool)> {
        let body = outgoing_body(submitted).ok_or(Error::SignalEnvelope)?;
        if !super::signal::valid_content(body, created_at) {
            return Err(Error::SignalEnvelope);
        }
        let event_id = new_event_id()?;
        let promoted = self.write(|tx| {
            if account_status::conversation_status(tx, conversation_id)? == AccountStatus::Deleted {
                return Err(Error::PeerDeleted);
            }
            tx.execute("INSERT INTO outgoing_operations (event_id, conversation_id, body, created_at, state)
                VALUES (?1, ?2, ?3, ?4, 'pending')", params![event_id, conversation_id, body, created_at])?;
            tx.execute("DELETE FROM drafts WHERE conversation_id = ?1 AND body = ?2", params![conversation_id, submitted])?;
            tx.execute("UPDATE conversations SET last_activity_at = max(last_activity_at, ?2) WHERE id = ?1",
                params![conversation_id, created_at])?;
            Ok(tx.execute("DELETE FROM requests WHERE conversation_id = ?1", [conversation_id])? != 0)
        })?;
        Ok((event_id, promoted))
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
        let mut query = self.connection.prepare(&format!(
            "SELECT body, created_at, outgoing, state, event_id FROM (
               SELECT m.body, m.created_at, m.direction AS outgoing, NULL AS state, NULL AS event_id, m.id AS sort_id
               FROM messages m WHERE m.conversation_id = ?1 AND {STANDALONE_MESSAGE}
               UNION ALL
               SELECT body, created_at, 1, state, event_id, event_id
               FROM outgoing_operations WHERE conversation_id = ?1
             ) ORDER BY created_at, sort_id"
        ))?;
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

    pub(crate) fn last_message_previews(
        &self,
        conversation_ids: impl IntoIterator<Item = i64>,
    ) -> Result<HashMap<i64, MessagePreview>> {
        let mut query = self.connection.prepare(&format!(
            "WITH last_saved AS (
               SELECT m.body, m.created_at, m.id AS sort_id
               FROM messages m WHERE m.conversation_id = ?1 AND {STANDALONE_MESSAGE}
               ORDER BY m.created_at DESC, m.id DESC LIMIT 1
             ), last_outgoing AS (
               SELECT body, created_at, event_id AS sort_id
               FROM outgoing_operations WHERE conversation_id = ?1
               ORDER BY created_at DESC, event_id DESC LIMIT 1
             )
             SELECT * FROM last_saved UNION ALL SELECT * FROM last_outgoing
             ORDER BY created_at DESC, sort_id DESC LIMIT 1"
        ))?;
        let mut previews = HashMap::new();
        for id in conversation_ids {
            let preview = query
                .query_row([id], |row| {
                    Ok(MessagePreview {
                        body: row.get(0)?,
                        created_at: row.get(1)?,
                    })
                })
                .optional()?;
            if let Some(preview) = preview {
                previews.insert(id, preview);
            }
        }
        Ok(previews)
    }
}
