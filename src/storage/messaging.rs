use super::{
    Database, DeliveryState, Error, Result,
    account_status::{self, AccountStatus},
};
use rusqlite::{OptionalExtension, Row, params};
use std::collections::HashMap;

#[cfg(all(test, windows))]
mod timeline_tests;

pub(crate) fn without_edge_blank_lines(body: &str) -> Option<&str> {
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TimelineSource {
    Saved,
    Operation,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct TimelineCursor {
    created_at: i64,
    sort_id: String,
    source: TimelineSource,
}

impl TimelineCursor {
    pub(crate) fn key(&self) -> String {
        let source = match self.source {
            TimelineSource::Saved => 'm',
            TimelineSource::Operation => 'o',
        };
        format!("{}:{source}:{}", self.created_at, self.sort_id)
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        let mut parts = key.splitn(3, ':');
        let created_at = parts.next()?.parse().ok()?;
        let source = match parts.next()? {
            "m" => TimelineSource::Saved,
            "o" => TimelineSource::Operation,
            _ => return None,
        };
        Some(Self {
            created_at,
            sort_id: parts.next()?.into(),
            source,
        })
    }
}

enum TimelinePage<'a> {
    Recent,
    Before(&'a TimelineCursor),
    After(&'a TimelineCursor),
    From(&'a TimelineCursor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimelineMessage {
    pub(crate) reference: Option<super::message_events::MessageReference>,
    pub(crate) presentation: super::presentation::Presentation,
    pub(crate) cursor: TimelineCursor,
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
    fn visibility_filters(&self) -> Result<(&'static str, &'static str)> {
        if !super::outgoing::schema_ready(&self.connection)? {
            return Ok(("", ""));
        }
        Ok((
            "AND NOT EXISTS(SELECT 1 FROM local_message_hides h WHERE h.message_id=m.id)",
            "AND NOT EXISTS(SELECT 1 FROM local_message_hides h WHERE h.conversation_id=o.conversation_id AND h.event_id=o.event_id AND h.author_void_id=(SELECT void_id FROM local_account))",
        ))
    }
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
        let reply = if super::outgoing::schema_ready(&self.connection)?
            && self.draft(conversation_id)?.as_deref() == Some(submitted)
        {
            self.reply_draft(conversation_id)?
        } else {
            None
        };
        self.queue_outgoing_with_reply(conversation_id, submitted, created_at, reply.as_ref())
    }

    #[allow(dead_code)] // Connected to UI in the next phase.
    pub(crate) fn queue_outgoing_with_reply(
        &mut self,
        conversation_id: i64,
        submitted: &str,
        created_at: i64,
        reply: Option<&super::message_events::MessageReference>,
    ) -> Result<(String, bool)> {
        let body = without_edge_blank_lines(submitted).ok_or(Error::SignalEnvelope)?;
        if !super::signal::valid_content(body, created_at) {
            return Err(Error::SignalEnvelope);
        }
        let event_id = new_event_id()?;
        let promoted = self.write(|tx| {
            if let Some(reference) = reply { super::outgoing::validate_target(tx, conversation_id, reference, true)?; }
            if account_status::conversation_status(tx, conversation_id)? == AccountStatus::Deleted {
                return Err(Error::PeerDeleted);
            }
            tx.execute("INSERT INTO outgoing_operations (event_id, conversation_id, body, created_at, state)
                VALUES (?1, ?2, ?3, ?4, 'pending')", params![event_id, conversation_id, body, created_at])?;
            if let Some(reference) = reply {
                tx.execute("INSERT INTO outgoing_reply_refs VALUES (?1,?2,?3)",params![event_id, reference.author_void_id, reference.event_id])?;
            }
            tx.execute("DELETE FROM drafts WHERE conversation_id = ?1 AND body = ?2", params![conversation_id, submitted])?;
            if super::outgoing::schema_ready(tx)? {
                tx.execute("DELETE FROM draft_reply_refs WHERE conversation_id=?1 AND NOT EXISTS(SELECT 1 FROM drafts WHERE conversation_id=?1)",[conversation_id])?;
            }
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
            if code != "sent"
                && tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM receipt_inbox WHERE original_event_id = ?1)",
                    [event_id],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Ok(());
            }
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
                "UPDATE messages SET delivery_state = CASE
                    WHEN delivery_state IN (?3, ?4) THEN delivery_state ELSE ?2 END WHERE id =
                (SELECT message_id FROM outbox WHERE event_id = ?1) AND direction = 1",
                params![
                    event_id,
                    delivery.code(),
                    DeliveryState::Delivered.code(),
                    DeliveryState::Read.code()
                ],
            )?;
            if code == "sent" && messages != 1 {
                return Err(Error::EventConflict);
            }
            Ok(())
        })
    }

    pub(crate) fn timeline_before(
        &self,
        conversation_id: i64,
        cursor: Option<&TimelineCursor>,
        limit: usize,
    ) -> Result<Vec<TimelineMessage>> {
        self.timeline_page(
            conversation_id,
            cursor.map_or(TimelinePage::Recent, TimelinePage::Before),
            limit,
        )
    }

    pub(crate) fn timeline_after(
        &self,
        conversation_id: i64,
        cursor: &TimelineCursor,
        limit: usize,
    ) -> Result<Vec<TimelineMessage>> {
        self.timeline_page(conversation_id, TimelinePage::After(cursor), limit)
    }

    pub(crate) fn timeline_from(
        &self,
        conversation_id: i64,
        cursor: &TimelineCursor,
        limit: usize,
    ) -> Result<Vec<TimelineMessage>> {
        self.timeline_page(conversation_id, TimelinePage::From(cursor), limit)
    }

    #[cfg(test)]
    pub(crate) fn conversation_timeline(
        &self,
        conversation_id: i64,
    ) -> Result<Vec<TimelineMessage>> {
        self.timeline_before(conversation_id, None, usize::MAX)
    }

    fn timeline_page(
        &self,
        conversation_id: i64,
        page: TimelinePage<'_>,
        limit: usize,
    ) -> Result<Vec<TimelineMessage>> {
        // Both IDs are stored as text, so retain their original lexical order at equal times.
        // The source breaks ties if independent records happen to have exactly the same ID.
        let (cursor, order, saved_cmp, operation_cmp) = match page {
            TimelinePage::Recent => (None, "DESC", "", ""),
            TimelinePage::Before(cursor) => (
                Some(cursor),
                "DESC",
                if cursor.source == TimelineSource::Operation {
                    "<="
                } else {
                    "<"
                },
                "<",
            ),
            TimelinePage::After(cursor) => (
                Some(cursor),
                "ASC",
                ">",
                if cursor.source == TimelineSource::Saved {
                    ">="
                } else {
                    ">"
                },
            ),
            TimelinePage::From(cursor) => (
                Some(cursor),
                "ASC",
                if cursor.source == TimelineSource::Saved {
                    ">="
                } else {
                    ">"
                },
                ">=",
            ),
        };
        let (saved_bound, operation_bound) = if cursor.is_some() {
            (
                format!("AND (m.created_at, m.id) {saved_cmp} (?3, ?4)"),
                format!("AND (o.created_at, o.event_id) {operation_cmp} (?3, ?4)"),
            )
        } else {
            (String::new(), String::new())
        };
        let (saved_visible, operation_visible) = self.visibility_filters()?;
        // Seek and limit using each table's conversation index before merging the two pages.
        let mut query = self.connection.prepare(&format!(
            "WITH saved AS (
               SELECT m.body, m.created_at, m.direction AS outgoing, NULL AS state, m.id AS sort_id, 0 AS source
               FROM messages m WHERE m.conversation_id = ?1 {saved_bound} AND {STANDALONE_MESSAGE} {saved_visible}
               ORDER BY m.created_at {order}, m.id {order} LIMIT ?2
             ), operations AS (
               SELECT body, created_at, 1 AS outgoing,
                      CASE WHEN EXISTS (
                          SELECT 1 FROM receipt_inbox r WHERE r.original_event_id = o.event_id
                      ) THEN 'delivered' ELSE state END AS state, event_id AS sort_id, 1 AS source
               FROM outgoing_operations o WHERE o.conversation_id = ?1 {operation_bound} {operation_visible}
               ORDER BY o.created_at {order}, o.event_id {order} LIMIT ?2
             )
             SELECT * FROM saved UNION ALL SELECT * FROM operations
             ORDER BY created_at {order}, sort_id {order}, source {order} LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = match cursor {
            Some(cursor) => query.query_map(
                params![conversation_id, limit, cursor.created_at, cursor.sort_id],
                timeline_message,
            )?,
            None => query.query_map(params![conversation_id, limit], timeline_message)?,
        };
        let mut messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let presentation_ready: i64 =
            self.connection
                .pragma_query_value(None, "user_version", |r| r.get(0))?;
        for message in &mut messages {
            if presentation_ready < 10 {
                continue;
            }
            message.reference = self.message_reference(conversation_id, &message.cursor.key())?;
            message.presentation =
                self.message_presentation(conversation_id, &message.cursor.key())?;
            if message.presentation.deleted {
                message.body = "Message deleted".into();
                message.retry_event.clear();
            }
        }
        if order == "DESC" {
            messages.reverse();
        }
        Ok(messages)
    }

    pub(crate) fn last_message_previews(
        &self,
        conversation_ids: impl IntoIterator<Item = i64>,
    ) -> Result<HashMap<i64, MessagePreview>> {
        let (saved_visible, operation_visible) = self.visibility_filters()?;
        let mut query = self.connection.prepare(&format!(
            "WITH last_saved AS (
               SELECT m.body, m.created_at, m.id AS sort_id
               FROM messages m WHERE m.conversation_id = ?1 AND {STANDALONE_MESSAGE} {saved_visible}
               ORDER BY m.created_at DESC, m.id DESC LIMIT 1
             ), last_outgoing AS (
               SELECT body, created_at, event_id AS sort_id
               FROM outgoing_operations o WHERE conversation_id = ?1 {operation_visible}
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

fn timeline_message(row: &Row<'_>) -> rusqlite::Result<TimelineMessage> {
    let state: Option<String> = row.get(3)?;
    let status = match state.as_deref() {
        Some("pending") => "Pending",
        Some("sent") => "Sent to relay",
        Some("delivered") => "Delivered",
        Some("unconfirmed") => "Acceptance unconfirmed",
        Some("failed") => "Send failed",
        _ => "",
    };
    let sort_id: String = row.get(4)?;
    let retry_event = if matches!(state.as_deref(), Some("unconfirmed" | "failed")) {
        sort_id.clone()
    } else {
        String::new()
    };
    let created_at = row.get(1)?;
    Ok(TimelineMessage {
        reference: None,
        presentation: Default::default(),
        cursor: TimelineCursor {
            created_at,
            sort_id,
            source: if row.get::<_, bool>(5)? {
                TimelineSource::Operation
            } else {
                TimelineSource::Saved
            },
        },
        body: row.get(0)?,
        created_at,
        outgoing: row.get(2)?,
        status,
        retry_event,
    })
}
