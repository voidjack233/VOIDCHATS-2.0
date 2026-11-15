//! Local 1:1 Signal operations. Every protocol callback uses transaction-bound stores.

use super::{
    DEVICE_ID,
    adapters::{SignalStore, decode_identity_pair},
    binding,
    peer::{self, TrustedPeer},
    wire::{self, Routing, SignalBinding},
};
use crate::storage::{
    Database, DeliveryState, Direction, Error, Message, Result, events::IncomingOutcome,
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

// The caller in this phase is the isolated harness. No relay or Slint code invokes these APIs.
#[allow(dead_code)]
impl Database {
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
        let (local_id, _) = local_account(&self.connection)?;
        let conversation = self.conversation(conversation_id)?;
        if conversation.peer_void_id != peer.void_id() || peer.void_id() == local_id {
            return Err(Error::SignalPeer);
        }
        // Check trust even when the event is already committed and the callback is skipped.
        peer::require_pinned(&self.connection, peer)?;
        let routing = Routing {
            event_id,
            sender: &local_id,
            recipient: peer.void_id(),
        };
        self.commit_outgoing(conversation_id, event_id, |tx| {
            let plaintext = wire::encode_content(routing, body, created_at)?;
            let local_binding = local_signal_binding(tx, &local_id)?;
            let local = address(&local_id);
            let remote = address(peer.void_id());
            let mut sessions = SignalStore::new(tx);
            let mut identities = SignalStore::new(tx);
            let mut rng = signal_rng()?;
            let now = SystemTime::now();
            let ciphertext = run_signal(message_encrypt(
                &plaintext,
                &remote,
                &local,
                &mut sessions,
                &mut identities,
                now,
                &mut rng,
            ))?;
            let payload = wire::encode_payload(routing, &local_binding, &ciphertext)?;
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

    /// A matching committed duplicate returns without invoking libsignal or consuming keys.
    pub(crate) fn decrypt_signal_event(
        &mut self,
        peer: &TrustedPeer,
        event_json: &[u8],
        now: SystemTime,
    ) -> Result<IncomingOutcome> {
        let (local_id, _) = local_account(&self.connection)?;
        let event = wire::decode_event(event_json, &local_id)?;
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
        let outcome = self.commit_incoming(&event.sender, &event.event_id, &digest, |tx| {
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

            let routing = Routing {
                event_id: &event.event_id,
                sender: &event.sender,
                recipient: &local_id,
            };
            let content = wire::decode_content(routing, &plaintext)?;
            let conversation_id = incoming_conversation(tx, &event.sender, content.created_at)?;
            Ok(Message {
                id: message_id(&event.sender, &event.event_id),
                conversation_id,
                direction: Direction::Incoming,
                body: content.body,
                created_at: content.created_at,
                delivery_state: DeliveryState::Received,
            })
        })?;
        if outcome == IncomingOutcome::AlreadyStored && !pinned {
            return Err(Error::SignalPeer);
        }
        Ok(outcome)
    }
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

fn local_account(connection: &Connection) -> Result<(String, VerifyingKey)> {
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

fn local_signal_binding(connection: &Connection, expected_id: &str) -> Result<SignalBinding> {
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
