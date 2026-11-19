//! Local 1:1 Signal operations. Every protocol callback uses transaction-bound stores.

use super::{
    DEVICE_ID,
    adapters::{SignalStore, decode_identity_pair},
    binding,
    peer::{self, TrustedPeer},
    wire::{self, Routing, SignalBinding},
};
use crate::{
    relay::ApplicationProtocolVersion,
    storage::{
        Database, DeliveryState, Direction, Error, Message, Result,
        events::{Incoming, IncomingOutcome},
        receipts::Upload,
    },
};
use ed25519_dalek::VerifyingKey;
use futures_util::FutureExt;
use libsignal_protocol::{
    CiphertextMessage, CiphertextMessageType, DeviceId, PreKeySignalMessage, ProtocolAddress,
    SessionStore, SignalMessage, initiate_session, message_decrypt, message_encrypt,
};
use rand::{SeedableRng, rngs::StdRng};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use std::{future::Future, time::SystemTime};
use zeroize::Zeroizing;

impl Database {
    /// Encrypt each durable control once, committing the ratchet with its payload.
    /// A retry reads these exact bytes; no network operation runs in this write.
    #[allow(dead_code)]
    pub(crate) fn encrypt_signal_control(
        &mut self,
        peer: &TrustedPeer,
        id: &str,
    ) -> Result<Vec<u8>> {
        let operation = self.control(id)?;
        if peer.void_id() != operation.peer {
            return Err(Error::SignalPeer);
        }
        peer::require_pinned(&self.connection, peer)?;
        if let Some(payload) = operation.payload {
            return Ok(payload);
        }
        self.write(|tx| {
            let (own, _) = local_account(tx)?;
            let routing = Routing {
                event_id: id,
                sender: &own,
                recipient: peer.void_id(),
            };
            let mut value = serde_json::json!({"v":2,"type":operation.kind,"client_message_id":id,
                "from_void_id":own,"to_void_id":peer.void_id(),"target":operation.target});
            if operation.kind == "reaction" {
                value["emoji"] = serde_json::json!(operation.emoji);
                value["active"] = serde_json::json!(operation.active);
                value["revision"] = serde_json::json!(operation.revision);
            }
            let bytes =
                Zeroizing::new(serde_json::to_vec(&value).map_err(|_| Error::SignalEnvelope)?);
            // Use the existing receiver contract as the final structural validator.
            if !matches!(
                wire::decode_content(routing, &bytes)?,
                wire::Content::Reaction(_) | wire::Content::Delete(_)
            ) {
                return Err(Error::SignalEnvelope);
            }
            let payload = encrypt_content(tx, peer, routing, &bytes)?;
            if tx.execute(
                "UPDATE outgoing_controls SET payload=?2 WHERE event_id=?1 AND payload IS NULL",
                params![id, payload],
            )? != 1
            {
                return Err(Error::EventConflict);
            }
            Ok(payload)
        })
    }
    pub(crate) fn has_signal_session(&mut self, peer: &TrustedPeer) -> Result<bool> {
        self.write(|tx| {
            let remote = address(peer.void_id());
            let store = SignalStore::new(tx);
            match run_signal(store.load_session(&remote))? {
                Some(session) => {
                    let expected = peer::require_pinned(tx, peer)?;
                    verify_session_peer(&session, &expected)?;
                    Ok(true)
                }
                None => {
                    peer::check_account_pin(tx, peer)?;
                    Ok(false)
                }
            }
        })
    }

    pub(crate) fn establish_signal_session(
        &mut self,
        peer: &TrustedPeer,
        claim_target_void_id: &str,
        claim_base64: &str,
        now: SystemTime,
    ) -> Result<()> {
        let (verified, bundle) = peer::verify_claim(peer, claim_target_void_id, claim_base64, now)?;
        self.write(|tx| {
            let (local_id, _) = local_account(tx)?;
            let local_binding = local_signal_binding(tx, &local_id)?;
            if peer.void_id() == local_id
                || verified.identity_key().serialize().as_ref()
                    == local_binding.identity_key.as_slice()
            {
                return Err(Error::SignalPeer);
            }
            let remote = address(peer.void_id());
            let local = address(&local_id);
            let mut sessions = SignalStore::new(tx);
            let mut identities = SignalStore::new(tx);
            let pinned = peer::check_pin(tx, &verified)?;
            if let Some(existing) = run_signal(sessions.load_session(&remote))? {
                if !pinned {
                    return Err(Error::SignalPeer);
                }
                verify_session_peer(&existing, verified.identity_key())?;
                return Ok(());
            }
            peer::pin(tx, &verified)?;
            let mut rng = signal_rng()?;
            run_signal(initiate_session(
                &remote,
                &local,
                &mut sessions,
                &mut identities,
                &bundle,
                now,
                &mut rng,
            ))
        })
    }

    /// Returns the exact VOID relay payload once the outbox transaction commits.
    pub(crate) fn encrypt_signal_message(
        &mut self,
        peer: &TrustedPeer,
        conversation_id: i64,
        event_id: &str,
        body: &str,
        created_at: i64,
    ) -> Result<Vec<u8>> {
        if !wire::valid_event_id(event_id) {
            return Err(Error::SignalEnvelope);
        }
        if self.outgoing_message_blocked(event_id)? {
            return Err(Error::EventConflict);
        }
        let (local_id, _) = local_account(&self.connection)?;
        let conversation = self.conversation(conversation_id)?;
        if conversation.peer_void_id != peer.void_id() || peer.void_id() == local_id {
            return Err(Error::SignalPeer);
        }
        // Check trust even when the event is already committed and the callback is skipped.
        peer::require_pinned(&self.connection, peer)?;
        if let Some(stored) = self.committed_outgoing(event_id)? {
            if stored.conversation_id != conversation_id
                || stored.body != body
                || stored.created_at != created_at
            {
                return Err(Error::EventConflict);
            }
            return Ok(stored.payload);
        }
        let routing = Routing {
            event_id,
            sender: &local_id,
            recipient: peer.void_id(),
        };
        let reply = self.outgoing_reply(event_id)?;
        self.commit_outgoing(conversation_id, event_id, true, |tx| {
            let plaintext = match reply {
                Some(reference) => {
                    wire::encode_content_with_reply(routing, body, created_at, Some(reference))?
                }
                None => wire::encode_content(routing, body, created_at)?,
            };
            let payload = encrypt_content(tx, peer, routing, &plaintext)?;
            Ok((
                Message {
                    id: message_id(&local_id, event_id),
                    conversation_id,
                    direction: Direction::Outgoing,
                    body: body.into(),
                    created_at,
                    delivery_state: DeliveryState::Pending,
                },
                payload,
            ))
        })
    }

    pub(crate) fn encrypt_signal_receipt(&mut self, upload: Upload) -> Result<Vec<u8>> {
        if let Some(payload) = upload.payload {
            return Ok(payload);
        }
        self.write(|tx| {
            let (local_id, _) = local_account(tx)?;
            let account_key: Vec<u8> = tx.query_row(
                "SELECT account_public_key FROM signal_peer_bindings WHERE peer_void_id = ?1 AND device_id = ?2",
                params![upload.peer, DEVICE_ID],
                |row| row.get(0),
            )?;
            let peer = TrustedPeer::from_trusted_lookup(
                &upload.peer, &account_key.try_into().map_err(|_| Error::SignalPeer)?,
            )?;
            let routing = Routing {
                event_id: &upload.event_id,
                sender: &local_id,
                recipient: &upload.peer,
            };
            let plaintext = wire::encode_receipt(routing, &upload.original_event_id)?;
            let payload = encrypt_content(tx, &peer, routing, &plaintext)?;
            if tx.execute(
                "UPDATE receipt_outbox SET payload = ?2 WHERE event_id = ?1 AND payload IS NULL",
                params![upload.event_id, payload],
            )? != 1 {
                return Err(Error::EventConflict);
            }
            Ok(payload)
        })
    }

    /// A matching committed duplicate returns without invoking libsignal or consuming keys.
    #[cfg(test)]
    pub(crate) fn decrypt_signal_event(
        &mut self,
        peer: &TrustedPeer,
        event_json: &[u8],
        now: SystemTime,
    ) -> Result<IncomingOutcome> {
        self.decrypt_signal_delivery(peer, event_json, now, None)
    }

    /// Simulate a previous receiver's classifier while using real Signal and
    /// the same durable incoming transaction. Recovery fixtures retain exact
    /// authenticated plaintext without replaying its ciphertext.
    #[cfg(test)]
    pub(super) fn decrypt_signal_event_with_decoder(
        &mut self,
        peer: &TrustedPeer,
        event_json: &[u8],
        now: SystemTime,
        decode: super::recovery::PlaintextDecoder,
    ) -> Result<IncomingOutcome> {
        self.decrypt_delivery_with_decoder(peer, event_json, now, None, decode)
    }

    pub(crate) fn decrypt_signal_delivery(
        &mut self,
        peer: &TrustedPeer,
        event_json: &[u8],
        now: SystemTime,
        expected_event_id: Option<&str>,
    ) -> Result<IncomingOutcome> {
        self.decrypt_delivery_with_decoder(
            peer,
            event_json,
            now,
            expected_event_id,
            incoming_plaintext,
        )
    }

    fn decrypt_delivery_with_decoder(
        &mut self,
        peer: &TrustedPeer,
        event_json: &[u8],
        now: SystemTime,
        expected_event_id: Option<&str>,
        decode: super::recovery::PlaintextDecoder,
    ) -> Result<IncomingOutcome> {
        let (local_id, _) = local_account(&self.connection)?;
        let event = wire::decode_event(event_json, &local_id)?;
        // Gate before duplicate lookup, identity pinning, and any Signal ratchet changes.
        // Recognizing a transport envelope does not imply support for its encrypted events.
        if event.min_protocol_version > wire::RECEIVER_APPLICATION_PROTOCOL {
            return Err(Error::SignalEnvelope);
        }
        if expected_event_id.is_some_and(|id| id != event.event_id) {
            return Err(Error::SignalEnvelope);
        }
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|age| i64::try_from(age.as_millis()).ok())
            .ok_or(Error::SignalEnvelope)?;
        if now_ms >= event.expires_at_ms {
            return Err(Error::SignalEnvelope);
        }
        if event.sender != peer.void_id() || event.sender == local_id {
            return Err(Error::SignalPeer);
        }
        let verified = peer::verify_binding(peer, &event.binding)?;
        // Reject a changed binding even when commit_incoming takes its duplicate path.
        let pinned = peer::check_pin(&self.connection, &verified)?;
        let digest = Sha256::digest(event.payload_base64.as_bytes()).into();
        let outcome =
            self.commit_signal_incoming(&event.sender, &event.event_id, &digest, |tx| {
                let local_binding = local_signal_binding(tx, &local_id)?;
                if local_binding.identity_key == event.binding.identity_key {
                    return Err(Error::SignalPeer);
                }
                let ciphertext = match event.message_type {
                    CiphertextMessageType::PreKey => {
                        let message = PreKeySignalMessage::try_from(event.ciphertext.as_slice())
                            .map_err(|_| Error::SignalProtocol)?;
                        if message.identity_key() != verified.identity_key()
                            || message.registration_id() != event.binding.registration_id
                        {
                            return Err(Error::SignalPeer);
                        }
                        CiphertextMessage::PreKeySignalMessage(message)
                    }
                    CiphertextMessageType::Whisper if pinned => CiphertextMessage::SignalMessage(
                        SignalMessage::try_from(event.ciphertext.as_slice())
                            .map_err(|_| Error::SignalProtocol)?,
                    ),
                    _ => return Err(Error::SignalPeer),
                };
                peer::pin(tx, &verified)?;
                let local = address(&local_id);
                let remote = address(&event.sender);
                let mut sessions = SignalStore::new(tx);
                let mut identities = SignalStore::new(tx);
                let mut prekeys = SignalStore::new(tx);
                let signed_prekeys = SignalStore::new(tx);
                let mut kyber_prekeys = SignalStore::new(tx);
                let mut rng = signal_rng()?;
                let plaintext = Zeroizing::new(run_signal(message_decrypt(
                    &ciphertext,
                    &remote,
                    &local,
                    &mut sessions,
                    &mut identities,
                    &mut prekeys,
                    &signed_prekeys,
                    &mut kyber_prekeys,
                    &mut rng,
                ))?);
                let session =
                    run_signal(sessions.load_session(&remote))?.ok_or(Error::SignalProtocol)?;
                verify_session_peer(&session, verified.identity_key())?;

                decode(
                    tx,
                    Routing {
                        event_id: &event.event_id,
                        sender: &event.sender,
                        recipient: &local_id,
                    },
                    plaintext,
                    event.min_protocol_version,
                    now_ms,
                    pinned,
                )
            })?;
        if outcome == IncomingOutcome::AlreadyStored && !pinned {
            return Err(Error::SignalPeer);
        }
        Ok(outcome)
    }
}

/// Classify authenticated plaintext without invoking Signal or changing its state.
/// Recovery uses the original routing and receive time, but a newer local decoder
/// may process content previously deferred by an older decoder.
pub(super) fn incoming_plaintext(
    tx: &Transaction<'_>,
    routing: Routing<'_>,
    plaintext: Zeroizing<Vec<u8>>,
    min_protocol_version: ApplicationProtocolVersion,
    received_at: i64,
    pinned: bool,
) -> Result<Incoming> {
    let content = wire::decode_content(routing, &plaintext)?;
    match content {
        wire::Content::Message(content) => {
            let conversation_id = incoming_conversation(tx, routing.sender, content.created_at)?;
            Ok(Incoming::Message {
                message: Message {
                    id: message_id(routing.sender, routing.event_id),
                    conversation_id,
                    direction: Direction::Incoming,
                    body: content.body,
                    created_at: content.created_at,
                    delivery_state: DeliveryState::Received,
                },
                receipt_at: content.requests_receipt.then_some(received_at),
                reply_to: content.reply_to,
            })
        }
        wire::Content::Receipt(original_event_id) if pinned => {
            Ok(Incoming::Receipt { original_event_id })
        }
        wire::Content::Receipt(_) => Err(Error::SignalPeer),
        wire::Content::Reaction(event) => Ok(Incoming::Reaction {
            event,
            plaintext,
            min_protocol_version,
            received_at,
        }),
        wire::Content::Delete(event) => Ok(Incoming::Delete(event)),
        wire::Content::Unsupported { reason } => Ok(Incoming::Deferred {
            plaintext,
            reason,
            min_protocol_version,
            received_at,
        }),
    }
}

pub(super) fn encrypt_content(
    tx: &Transaction<'_>,
    peer: &TrustedPeer,
    routing: Routing<'_>,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let expected = peer::require_pinned(tx, peer)?;
    let local_binding = local_signal_binding(tx, routing.sender)?;
    let local = address(routing.sender);
    let remote = address(routing.recipient);
    let mut sessions = SignalStore::new(tx);
    let mut identities = SignalStore::new(tx);
    let session = run_signal(sessions.load_session(&remote))?.ok_or(Error::SignalProtocol)?;
    verify_session_peer(&session, &expected)?;
    let ciphertext = run_signal(message_encrypt(
        plaintext,
        &remote,
        &local,
        &mut sessions,
        &mut identities,
        SystemTime::now(),
        &mut signal_rng()?,
    ))?;
    wire::encode_payload(routing, &local_binding, &ciphertext)
}

fn address(void_id: &str) -> ProtocolAddress {
    ProtocolAddress::new(
        void_id.into(),
        DeviceId::new(DEVICE_ID as u8).expect("fixed Signal device ID is valid"),
    )
}

fn signal_rng() -> Result<StdRng> {
    StdRng::try_from_os_rng().map_err(|_| Error::SignalProtocol)
}

fn run_signal<T>(
    operation: impl Future<Output = libsignal_protocol::error::Result<T>>,
) -> Result<T> {
    // The official async traits are backed by synchronous SQL in this worker. A future that
    // ever tries to suspend fails the transaction instead of holding it across an await.
    operation
        .now_or_never()
        .ok_or(Error::SignalProtocol)?
        .map_err(|_| Error::SignalProtocol)
}

fn verify_session_peer(
    session: &libsignal_protocol::SessionRecord,
    expected: &libsignal_protocol::IdentityKey,
) -> Result<()> {
    let key = session
        .remote_identity_key_bytes()
        .map_err(|_| Error::SignalProtocol)?;
    if key.as_deref() != Some(expected.serialize().as_ref()) {
        return Err(Error::SignalPeer);
    }
    Ok(())
}

pub(super) fn local_account(connection: &Connection) -> Result<(String, VerifyingKey)> {
    let (id, key) = connection.query_row(
        "SELECT void_id, public_key FROM local_account WHERE singleton = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let key = VerifyingKey::from_bytes(&key.try_into().map_err(|_| Error::SignalState)?)
        .map_err(|_| Error::SignalState)?;
    if !crate::account::valid_void_id(&id) || key.is_weak() {
        return Err(Error::SignalState);
    }
    Ok((id, key))
}

pub(super) fn local_signal_binding(
    connection: &Connection,
    expected_id: &str,
) -> Result<SignalBinding> {
    let (id, verifying_key) = local_account(connection)?;
    if id != expected_id {
        return Err(Error::WrongAccount);
    }
    let (device_id, registration_id, pair, signature) = connection.query_row(
        "SELECT device_id, registration_id, key_pair, binding_signature FROM signal_local_identity WHERE slot = 1",
        [], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?,
            Zeroizing::new(row.get::<_, Vec<u8>>(2)?), row.get::<_, Vec<u8>>(3)?)),
    ).map_err(|_| Error::SignalState)?;
    let pair = decode_identity_pair(&pair).map_err(|_| Error::SignalState)?;
    let public_key = pair.identity_key().serialize();
    if !binding::verify_binding(
        &verifying_key,
        &id,
        device_id,
        registration_id,
        &public_key,
        &signature,
    ) {
        return Err(Error::SignalState);
    }
    Ok(SignalBinding {
        device_id,
        registration_id,
        identity_key: public_key
            .as_ref()
            .try_into()
            .map_err(|_| Error::SignalState)?,
        account_signature: signature.try_into().map_err(|_| Error::SignalState)?,
    })
}

fn incoming_conversation(tx: &Transaction<'_>, sender: &str, created_at: i64) -> Result<i64> {
    let created = tx.query_row(
        "INSERT INTO conversations (peer_void_id, created_at, last_activity_at) VALUES (?1, ?2, ?2)
         ON CONFLICT (peer_void_id) DO NOTHING RETURNING id",
        params![sender, created_at], |row| row.get(0),
    ).optional()?;
    if let Some(id) = created {
        tx.execute("INSERT INTO requests (conversation_id) VALUES (?1)", [id])?;
        return Ok(id);
    }
    Ok(tx.query_row(
        "SELECT id FROM conversations WHERE peer_void_id = ?1",
        [sender],
        |row| row.get(0),
    )?)
}

fn message_id(sender: &str, event_id: &str) -> String {
    // Relay delivery is scoped by sender. The local primary key must also distinguish senders.
    format!("{sender}:{event_id}")
}
