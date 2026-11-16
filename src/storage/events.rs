//! Durable event boundaries for the messaging worker.
//!
//! Official libsignal uses local async store traits. A worker can run its local future
//! to completion with separate adapters borrowing the same SQLCipher transaction.
//! Session and peer identity lookups use a protocol address, including the device ID.
//! The adapters must persist the separate Signal identity and registration ID, and
//! support updating sessions, consuming one-time prekeys, and recording Kyber prekey
//! use. The Ed25519 relay-authentication key is outside these stores.
//!
//! Persist a draft or a pending operation with its stable event ID before preparing
//! ciphertext. Keep any fetched prekey bundle in memory before entering the write.
//! Once committed, the exact payload can be retried using the same event ID. A failed
//! callback must discard any in-memory protocol state derived from the failed write.

use super::{Database, Direction, Error, Message, Result, records::insert_message};
use rusqlite::{OptionalExtension, Transaction, params};

pub(super) enum Incoming {
    Message {
        message: Message,
        receipt_at: Option<i64>,
    },
    Receipt {
        original_event_id: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IncomingOutcome {
    Stored,
    AlreadyStored,
}

// Callbacks borrow one transaction for the libsignal store adapters.
// Keep network operations outside the callback.
impl Database {
    /// Returns the exact payload for this event, only after the transaction commits.
    /// Reserve the event ID before calling this and use it again on retry. An already
    /// committed event skips the callback, so retry does not advance the session.
    pub(crate) fn commit_outgoing(
        &mut self,
        conversation_id: i64,
        event_id: &str,
        receipt_requested: bool,
        prepare: impl FnOnce(&Transaction<'_>) -> Result<(Message, Vec<u8>)>,
    ) -> Result<Vec<u8>> {
        if event_id.is_empty() {
            return Err(Error::EventConflict);
        }
        self.write(|tx| {
            let stored = tx.query_row(
                "SELECT m.conversation_id, o.payload FROM outbox o JOIN messages m ON m.id = o.message_id
                 WHERE o.event_id = ?1", [event_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            ).optional()?;
            if let Some((stored_conversation, payload)) = stored {
                return if stored_conversation == conversation_id { Ok(payload) } else { Err(Error::EventConflict) };
            }
            let (message, payload) = prepare(tx)?;
            if message.conversation_id != conversation_id || message.direction != Direction::Outgoing {
                return Err(Error::EventConflict);
            }
            insert_message(tx, &message)?;
            tx.execute("INSERT INTO outbox (event_id, message_id, payload, receipt_requested) VALUES (?1, ?2, ?3, ?4)",
                params![event_id, message.id, payload, receipt_requested])?;
            Ok(payload)
        })
    }

    /// Call with the canonical sender and a digest of the exact received payload bytes.
    /// Only an Ok result may authorize a later ACK for this event. A committed duplicate
    /// can be acknowledged again without running the callback or writing another message.
    #[cfg(test)]
    pub(crate) fn commit_incoming(
        &mut self,
        sender: &str,
        event_id: &str,
        digest: &[u8; 32],
        process: impl FnOnce(&Transaction<'_>) -> Result<Message>,
    ) -> Result<IncomingOutcome> {
        self.commit_signal_incoming(sender, event_id, digest, |tx| {
            Ok(Incoming::Message {
                message: process(tx)?,
                receipt_at: None,
            })
        })
    }

    pub(super) fn commit_signal_incoming(
        &mut self,
        sender: &str,
        event_id: &str,
        digest: &[u8; 32],
        process: impl FnOnce(&Transaction<'_>) -> Result<Incoming>,
    ) -> Result<IncomingOutcome> {
        if sender.is_empty() || event_id.is_empty() {
            return Err(Error::EventConflict);
        }
        self.write(|tx| {
            let stored: Option<Vec<u8>> = tx.query_row(
                "SELECT payload_digest FROM inbox WHERE sender_void_id = ?1 AND event_id = ?2
                 UNION ALL
                 SELECT payload_digest FROM receipt_inbox WHERE sender_void_id = ?1 AND event_id = ?2",
                params![sender, event_id], |row| row.get(0),
            ).optional()?;
            if let Some(stored) = stored {
                return if stored == digest { Ok(IncomingOutcome::AlreadyStored) } else { Err(Error::EventConflict) };
            }

            match process(tx)? {
                Incoming::Message { message, receipt_at } => {
                    let peer: String = tx.query_row("SELECT peer_void_id FROM conversations WHERE id = ?1",
                        [message.conversation_id], |row| row.get(0))?;
                    if peer != sender || message.direction != Direction::Incoming {
                        return Err(Error::EventConflict);
                    }
                    insert_message(tx, &message)?;
                    tx.execute("INSERT INTO inbox (sender_void_id, event_id, payload_digest, message_id) VALUES (?1, ?2, ?3, ?4)",
                        params![sender, event_id, digest.as_slice(), message.id])?;
                    if let Some(now) = receipt_at {
                        super::receipts::queue(tx, sender, event_id, now)?;
                    }
                }
                Incoming::Receipt { original_event_id } => {
                    super::receipts::verify(tx, sender, event_id, digest, &original_event_id)?;
                }
            }
            Ok(IncomingOutcome::Stored)
        })
    }
}
