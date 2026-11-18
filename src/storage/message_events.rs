//! Transactional receiver metadata. References are scoped to one authenticated
//! peer and the original author's stable event UUID, not a timeline row index.

use super::{Database, Error, Result};
use crate::{account::valid_void_id, relay::ApplicationProtocolVersion};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[cfg(all(test, windows))]
#[path = "message_events_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageReference {
    pub(crate) author_void_id: String,
    pub(crate) event_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReactionEvent {
    pub(crate) target: MessageReference,
    pub(crate) emoji: String,
    pub(crate) active: bool,
    pub(crate) revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteEvent {
    pub(crate) target: MessageReference,
}

// One policy shared by authenticated wire validation and transactional storage.
// Emoji presentation itself belongs to the future picker; these bounded tokens
// are stored as exact text and are never interpreted as markup.
pub(super) fn valid_reaction_token(emoji: &str) -> bool {
    !emoji.is_empty()
        && emoji.len() <= 64
        && emoji.chars().count() <= 16
        && !emoji
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && emoji.chars().any(|ch| !ch.is_ascii())
}

pub(super) fn reference_valid(
    reference: &MessageReference,
    sender: &str,
    recipient: &str,
    event_id: &str,
) -> bool {
    valid_void_id(&reference.author_void_id)
        && matches_author(&reference.author_void_id, sender, recipient)
        && valid_event_id(&reference.event_id)
        && !(reference.author_void_id == sender && reference.event_id == event_id)
}

fn matches_author(author: &str, sender: &str, recipient: &str) -> bool {
    author == sender || author == recipient
}

fn valid_event_id(id: &str) -> bool {
    id.len() == 36
        && id != "00000000-0000-0000-0000-000000000000"
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn validate_reference(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    target: &MessageReference,
) -> Result<()> {
    let local_id: String =
        tx.query_row("SELECT void_id FROM local_account", [], |row| row.get(0))?;
    if !reference_valid(target, sender, &local_id, event_id) {
        return Err(Error::SignalEnvelope);
    }
    // An unavailable original is valid. A known original in another peer's
    // conversation cannot be addressed through this peer's control event.
    let belongs_to_other_peer: bool = tx.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE c.peer_void_id <> ?4 AND m.id IN (
                 SELECT message_id FROM inbox WHERE sender_void_id = ?1 AND event_id = ?2
                 UNION ALL
                 SELECT message_id FROM outbox WHERE event_id = ?2 AND ?1 = ?3
             )
             UNION ALL
             SELECT 1 FROM outgoing_operations o JOIN conversations c ON c.id = o.conversation_id
             WHERE o.event_id = ?2 AND ?1 = ?3 AND c.peer_void_id <> ?4
         )",
        params![target.author_void_id, target.event_id, local_id, sender],
        |row| row.get(0),
    )?;
    if belongs_to_other_peer {
        return Err(Error::SignalEnvelope);
    }
    Ok(())
}

pub(super) fn store_reply(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    message_id: &str,
    target: &MessageReference,
) -> Result<()> {
    validate_reference(tx, sender, event_id, target)?;
    tx.execute(
        "INSERT INTO message_reply_refs (message_id, original_author_void_id, original_event_id)
         VALUES (?1, ?2, ?3)",
        params![message_id, target.author_void_id, target.event_id],
    )?;
    Ok(())
}

pub(super) fn tombstoned(
    tx: &Transaction<'_>,
    peer: &str,
    target: &MessageReference,
) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM message_tombstones WHERE peer_void_id = ?1
         AND original_author_void_id = ?2 AND original_event_id = ?3)",
        params![peer, target.author_void_id, target.event_id],
        |row| row.get(0),
    )?)
}

/// False means the same revision tried to assert a different state. The caller
/// must durably defer that authenticated event instead of overwriting history.
pub(super) fn apply_reaction(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    event: &ReactionEvent,
) -> Result<bool> {
    validate_reference(tx, sender, event_id, &event.target)?;
    if event.revision <= 0 || !valid_reaction_token(&event.emoji) {
        return Err(Error::SignalEnvelope);
    }
    if tombstoned(tx, sender, &event.target)? {
        return Ok(true);
    }
    let current = tx
        .query_row(
            "SELECT active, revision FROM message_reactions WHERE peer_void_id = ?1
             AND original_author_void_id = ?2 AND original_event_id = ?3
             AND reactor_void_id = ?1 AND emoji = ?4",
            params![
                sender,
                event.target.author_void_id,
                event.target.event_id,
                event.emoji
            ],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((active, revision)) = current {
        if revision > event.revision || (revision == event.revision && active == event.active) {
            return Ok(true);
        }
        if revision == event.revision {
            return Ok(false);
        }
    }
    tx.execute(
        "INSERT INTO message_reactions (peer_void_id, original_author_void_id, original_event_id,
         reactor_void_id, emoji, active, revision, source_event_id) VALUES (?1, ?2, ?3, ?1, ?4, ?5, ?6, ?7)
         ON CONFLICT (peer_void_id, original_author_void_id, original_event_id, reactor_void_id, emoji)
         DO UPDATE SET active = excluded.active, revision = excluded.revision,
                       source_event_id = excluded.source_event_id",
        params![sender, event.target.author_void_id, event.target.event_id,
            event.emoji, event.active, event.revision, event_id],
    )?;
    Ok(true)
}

pub(super) fn apply_delete(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    event: &DeleteEvent,
) -> Result<()> {
    validate_reference(tx, sender, event_id, &event.target)?;
    if event.target.author_void_id != sender {
        return Err(Error::SignalEnvelope);
    }
    tx.execute(
        "INSERT INTO message_tombstones (peer_void_id, original_author_void_id, original_event_id, source_event_id)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![sender, event.target.author_void_id, event.target.event_id, event_id],
    )?;
    // Preserve row identity, cursor ordering, receipts, and original inbox digest.
    // A deletion never deletes another participant's outgoing message.
    tx.execute(
        "UPDATE messages SET body = '' WHERE direction = 0 AND id IN (
            SELECT i.message_id FROM inbox i JOIN conversations c ON c.peer_void_id = i.sender_void_id
            JOIN messages m ON m.id = i.message_id AND m.conversation_id = c.id
            WHERE i.sender_void_id = ?1 AND i.event_id = ?2)",
        params![sender, event.target.event_id],
    )?;
    tx.execute(
        "DELETE FROM unread_messages WHERE message_id IN (
            SELECT message_id FROM inbox WHERE sender_void_id = ?1 AND event_id = ?2)",
        params![sender, event.target.event_id],
    )?;
    tx.execute(
        "UPDATE message_reactions SET active = 0 WHERE peer_void_id = ?1
         AND original_author_void_id = ?2 AND original_event_id = ?3",
        params![sender, event.target.author_void_id, event.target.event_id],
    )?;
    Ok(())
}

pub(super) fn store_control(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    digest: &[u8; 32],
    kind: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO control_event_inbox (sender_void_id, event_id, payload_digest, kind)
         VALUES (?1, ?2, ?3, ?4)",
        params![sender, event_id, digest.as_slice(), kind],
    )?;
    Ok(())
}

pub(super) fn store_deferred(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    plaintext: &[u8],
    reason: &str,
    min_protocol_version: ApplicationProtocolVersion,
    received_at: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO deferred_signal_events (sender_void_id, event_id, plaintext, reason,
         min_protocol_version, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            sender,
            event_id,
            plaintext,
            reason,
            String::from(min_protocol_version),
            received_at
        ],
    )?;
    Ok(())
}

// Receiver persistence API; rendering and sending these features is a later phase.
#[allow(dead_code)]
impl Database {
    pub(crate) fn reply_reference(&self, message_id: &str) -> Result<Option<MessageReference>> {
        Ok(self.connection.query_row(
            "SELECT original_author_void_id, original_event_id FROM message_reply_refs WHERE message_id = ?1",
            [message_id],
            |row| Ok(MessageReference { author_void_id: row.get(0)?, event_id: row.get(1)? }),
        ).optional()?)
    }

    pub(crate) fn message_deleted(&self, peer: &str, target: &MessageReference) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM message_tombstones WHERE peer_void_id = ?1
             AND original_author_void_id = ?2 AND original_event_id = ?3)",
            params![peer, target.author_void_id, target.event_id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn reaction_states(
        &self,
        peer: &str,
        target: &MessageReference,
    ) -> Result<Vec<(String, String, bool, i64)>> {
        Ok(self.connection.prepare(
            "SELECT reactor_void_id, emoji, active, revision FROM message_reactions WHERE peer_void_id = ?1
             AND original_author_void_id = ?2 AND original_event_id = ?3 ORDER BY reactor_void_id, emoji",
        )?.query_map(params![peer, target.author_void_id, target.event_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
