//! Durable Signal control events. A receipt is evidence of local commit by the peer.

use super::{Database, DeliveryState, Error, Result, messaging::new_event_id};
use rusqlite::{OptionalExtension, Transaction, params};

const MAX_ATTEMPTS: i64 = 8;
const DEADLINE_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const INITIAL_DELAY_MS: i64 = 30_000;
const MAX_DELAY_MS: i64 = 3_600_000;

pub(crate) struct Upload {
    pub(crate) event_id: String,
    pub(crate) peer: String,
    pub(crate) original_event_id: String,
    pub(crate) payload: Option<Vec<u8>>,
}

pub(super) fn queue(tx: &Transaction<'_>, sender: &str, original: &str, now: i64) -> Result<()> {
    let event_id = new_event_id()?;
    tx.execute(
        "INSERT INTO receipt_outbox (event_id, peer_void_id, original_event_id, next_attempt_at, deadline_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![event_id, sender, original, now, now + DEADLINE_MS],
    )?;
    Ok(())
}

pub(super) fn verify(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    digest: &[u8; 32],
    original: &str,
) -> Result<()> {
    let inserted = tx.execute(
        "INSERT INTO receipt_inbox (sender_void_id, event_id, payload_digest, original_event_id)
         SELECT ?1, ?2, ?3, o.event_id FROM outbox o
         JOIN messages m ON m.id = o.message_id
         JOIN conversations c ON c.id = m.conversation_id
         WHERE o.event_id = ?4 AND o.receipt_requested = 1 AND m.direction = 1
         AND c.peer_void_id = ?1",
        params![sender, event_id, digest.as_slice(), original],
    )?;
    if inserted != 1 {
        return Err(Error::SignalEnvelope);
    }
    tx.execute(
        "UPDATE messages SET delivery_state = ?2
         WHERE id = (SELECT message_id FROM outbox WHERE event_id = ?1) AND delivery_state <> ?3",
        params![
            original,
            DeliveryState::Delivered.code(),
            DeliveryState::Read.code()
        ],
    )?;
    Ok(())
}

impl Database {
    pub(crate) fn has_verified_delivery(&self, event_id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM receipt_inbox WHERE original_event_id = ?1)",
            [event_id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn next_receipt_attempt(&self, now: i64) -> Result<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT min(next_attempt_at) FROM receipt_outbox
             WHERE attempts < ?1 AND deadline_at > ?2 AND next_attempt_at < deadline_at",
            params![MAX_ATTEMPTS, now],
            |row| row.get(0),
        )?)
    }

    /// Reserve the attempt before encryption or upload, so crashes respect backoff.
    pub(crate) fn claim_receipt(&mut self, now: i64) -> Result<Option<Upload>> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM receipt_outbox WHERE deadline_at <= ?1
                 OR (attempts >= ?2 AND next_attempt_at <= ?1)",
                params![now, MAX_ATTEMPTS],
            )?;
            let due = tx.query_row(
                "SELECT event_id, peer_void_id, original_event_id, payload, attempts
                 FROM receipt_outbox WHERE attempts < ?1 AND next_attempt_at <= ?2 AND deadline_at > ?2
                 ORDER BY next_attempt_at, event_id LIMIT 1",
                params![MAX_ATTEMPTS, now],
                |row| Ok((Upload {
                    event_id: row.get(0)?,
                    peer: row.get(1)?,
                    original_event_id: row.get(2)?,
                    payload: row.get(3)?,
                }, row.get::<_, u32>(4)?)),
            ).optional()?;
            let Some((upload, attempts)) = due else { return Ok(None) };
            let delay = (INITIAL_DELAY_MS << attempts).min(MAX_DELAY_MS);
            tx.execute(
                "UPDATE receipt_outbox SET attempts = attempts + 1, next_attempt_at = ?2 WHERE event_id = ?1",
                params![upload.event_id, now + delay],
            )?;
            Ok(Some(upload))
        })
    }

    pub(crate) fn finish_receipt(&self, event_id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM receipt_outbox WHERE event_id = ?1", [event_id])?;
        Ok(())
    }
}
