//! Local recovery of previously authenticated plaintext. This module deliberately
//! does not import libsignal operations or ciphertext decoding.

use super::{
    DEVICE_ID,
    crypto::{incoming_plaintext, local_account},
    peer::{self, TrustedPeer},
    wire::{self, Routing},
};
use crate::{
    account::valid_void_id,
    relay::ApplicationProtocolVersion,
    storage::{
        Database, Error, Result,
        events::{Incoming, apply_incoming},
    },
};
use rusqlite::{OptionalExtension, Transaction, params};
use zeroize::Zeroizing;

const PAGE_SIZE: i64 = 64;

pub(super) type PlaintextDecoder = fn(
    &Transaction<'_>,
    Routing<'_>,
    Zeroizing<Vec<u8>>,
    ApplicationProtocolVersion,
    i64,
    bool,
) -> Result<Incoming>;

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct RecoverySummary {
    pub(crate) recovered: usize,
    pub(crate) deferred: usize,
    pub(crate) quarantined: usize,
}

enum Outcome {
    Recovered,
    Deferred,
    Quarantined,
}

impl Database {
    /// Scan bounded key pages so an unsupported head cannot starve later records.
    /// Each record is an independent atomic application; a crash can resume safely.
    pub(crate) fn recover_deferred_events(&mut self) -> Result<RecoverySummary> {
        self.recover_deferred_with(incoming_plaintext)
    }

    #[cfg(test)]
    pub(super) fn recover_deferred_with_decoder(
        &mut self,
        decode: PlaintextDecoder,
    ) -> Result<RecoverySummary> {
        self.recover_deferred_with(decode)
    }

    fn recover_deferred_with(&mut self, decode: PlaintextDecoder) -> Result<RecoverySummary> {
        let mut summary = RecoverySummary::default();
        let mut cursor: Option<(i64, String, String)> = None;
        loop {
            let keys = {
                let mut query = self.connection.prepare(
                    "SELECT received_at, sender_void_id, event_id FROM deferred_signal_events
                     WHERE recovery_state = 'pending' AND (?1 IS NULL OR
                         (received_at, sender_void_id, event_id) > (?1, ?2, ?3))
                     ORDER BY received_at, sender_void_id, event_id LIMIT ?4",
                )?;
                let rows = query
                    .query_map(
                        params![
                            cursor.as_ref().map(|v| v.0),
                            cursor.as_ref().map(|v| &v.1),
                            cursor.as_ref().map(|v| &v.2),
                            PAGE_SIZE
                        ],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            if keys.is_empty() {
                return Ok(summary);
            }
            for key in keys {
                let result = self.write(|tx| recover_one(tx, &key.1, &key.2, decode));
                let outcome = match result {
                    Ok(outcome) => outcome,
                    Err(
                        error @ (Error::SignalEnvelope | Error::SignalPeer | Error::EventConflict),
                    ) => {
                        // The failed application transaction has already rolled back.
                        // A crash before quarantine merely revalidates the same record.
                        let reason = match error {
                            Error::SignalPeer => "invalid_authenticated_provenance",
                            Error::EventConflict => "conflicting_event_identity",
                            _ => "invalid_content_or_authorization",
                        };
                        self.write(|tx| {
                            tx.execute("UPDATE deferred_signal_events SET recovery_state = 'quarantined', recovery_error = ?3
                                WHERE sender_void_id = ?1 AND event_id = ?2 AND recovery_state = 'pending'",
                                params![key.1,key.2,reason])?;
                            Ok(())
                        })?;
                        Outcome::Quarantined
                    }
                    Err(error) => return Err(error),
                };
                match outcome {
                    Outcome::Recovered => summary.recovered += 1,
                    Outcome::Deferred => summary.deferred += 1,
                    Outcome::Quarantined => summary.quarantined += 1,
                }
                cursor = Some(key);
            }
        }
    }
}

fn recover_one(
    tx: &Transaction<'_>,
    sender: &str,
    event_id: &str,
    decode: PlaintextDecoder,
) -> Result<Outcome> {
    let record = tx.query_row(
        "SELECT d.plaintext, d.min_protocol_version, d.received_at, c.payload_digest, c.kind
         FROM deferred_signal_events d LEFT JOIN control_event_inbox c USING (sender_void_id,event_id)
         WHERE d.sender_void_id = ?1 AND d.event_id = ?2 AND d.recovery_state = 'pending'",
        params![sender,event_id], |row|Ok((Zeroizing::new(row.get::<_,Vec<u8>>(0)?),row.get::<_,String>(1)?,
            row.get::<_,i64>(2)?,row.get::<_,Option<Vec<u8>>>(3)?,row.get::<_,Option<String>>(4)?)),
    ).optional()?;
    let Some((plaintext, minimum, received_at, digest, kind)) = record else {
        return Ok(Outcome::Deferred);
    };
    if !valid_void_id(sender)
        || !wire::valid_event_id(event_id)
        || kind.as_deref() != Some("deferred")
    {
        return Err(Error::SignalEnvelope);
    }
    let digest: [u8; 32] = digest
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::SignalEnvelope)?;
    if received_at < 0 || received_at.checked_add(30 * 24 * 60 * 60 * 1000).is_none() {
        return Err(Error::SignalEnvelope);
    }
    let min_protocol_version =
        ApplicationProtocolVersion::parse(&minimum).ok_or(Error::SignalEnvelope)?;
    if min_protocol_version > wire::RECEIVER_APPLICATION_PROTOCOL {
        return Ok(Outcome::Deferred);
    }
    let (local_id, _) = local_account(tx)?;
    if sender == local_id {
        return Err(Error::SignalPeer);
    }
    let key: Option<Vec<u8>> = tx.query_row(
        "SELECT account_public_key FROM signal_peer_bindings WHERE peer_void_id = ?1 AND device_id = ?2",
        params![sender,DEVICE_ID], |row|row.get(0),
    ).optional()?;
    let key: [u8; 32] = key
        .and_then(|key| key.try_into().ok())
        .ok_or(Error::SignalPeer)?;
    let peer = TrustedPeer::from_trusted_lookup(sender, &key)?;
    // The binding signature and pinned identity are immutable account provenance.
    // Read-only verification does not inspect or modify Signal ratchets.
    peer::require_pinned(tx, &peer)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM inbox WHERE sender_void_id = ?1 AND event_id = ?2
         UNION ALL SELECT 1 FROM receipt_inbox WHERE sender_void_id = ?1 AND event_id = ?2)",
        params![sender, event_id],
        |row| row.get(0),
    )?;
    if exists {
        return Err(Error::EventConflict);
    }
    let incoming = decode(
        tx,
        Routing {
            event_id,
            sender,
            recipient: &local_id,
        },
        plaintext,
        min_protocol_version,
        received_at,
        true,
    )?;
    if let Incoming::Deferred { reason, .. } = incoming {
        tx.execute("UPDATE deferred_signal_events SET reason = ?3 WHERE sender_void_id = ?1 AND event_id = ?2",
            params![sender,event_id,reason])?;
        return Ok(Outcome::Deferred);
    }
    // Remove the old placeholder ledger only within the replacement transaction.
    // Any failed effect/COMMIT restores both the saved plaintext and its digest.
    tx.execute(
        "DELETE FROM deferred_signal_events WHERE sender_void_id = ?1 AND event_id = ?2",
        params![sender, event_id],
    )?;
    tx.execute(
        "DELETE FROM control_event_inbox WHERE sender_void_id = ?1 AND event_id = ?2",
        params![sender, event_id],
    )?;
    apply_incoming(tx, sender, event_id, &digest, incoming)?;
    let still_deferred: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM deferred_signal_events WHERE sender_void_id = ?1 AND event_id = ?2)",
        params![sender,event_id],|row|row.get(0),
    )?;
    Ok(if still_deferred {
        Outcome::Deferred
    } else {
        Outcome::Recovered
    })
}
