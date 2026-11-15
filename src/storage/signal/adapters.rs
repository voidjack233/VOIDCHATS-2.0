//! Transaction-bound stores for the next phase's libsignal operations.

#![allow(dead_code)] // Phase 5.1 exercises these traits in tests without starting a messaging worker.

use async_trait::async_trait;
use libsignal_protocol::{
    CiphertextMessageType, Direction, GenericSignedPreKey, IdentityChange, IdentityKey,
    IdentityKeyPair, IdentityKeyStore, KeyPair, KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore,
    PreKeyId, PreKeyRecord, PreKeyStore, ProtocolAddress, ProtocolStore, PublicKey, SessionRecord,
    SessionStore, SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore, kem,
};
use rusqlite::{OptionalExtension, Transaction, params};
use zeroize::Zeroizing;

use crate::account::valid_void_id;

type Result<T> = libsignal_protocol::error::Result<T>;

const MAX_PREKEY_ID: u32 = 16_777_214;
const SIGNATURE_LENGTH: usize = 64;

/// Needs a live transaction and never commits or caches a record. Separate adapters can
/// borrow the same transaction for libsignal operations that take multiple mutable stores.
pub(in crate::storage) struct SignalStore<'tx, 'conn> {
    tx: &'tx Transaction<'conn>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreKeyKind {
    Ec,
    Signed,
    Kyber,
}

impl PreKeyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ec => "ec",
            Self::Signed => "signed",
            Self::Kyber => "kyber",
        }
    }

    fn invalid_id(self) -> SignalProtocolError {
        match self {
            Self::Ec => SignalProtocolError::InvalidPreKeyId,
            Self::Signed => SignalProtocolError::InvalidSignedPreKeyId,
            Self::Kyber => SignalProtocolError::InvalidKyberPreKeyId,
        }
    }

    fn check_id(self, id: u32) -> Result<()> {
        if (1..=MAX_PREKEY_ID).contains(&id) {
            Ok(())
        } else {
            Err(self.invalid_id())
        }
    }
}

fn storage_error(_: rusqlite::Error) -> SignalProtocolError {
    // Avoid forwarding SQL, paths, or parameter values to libsignal or its caller.
    SignalProtocolError::InvalidState("Signal store", "database operation failed".into())
}

fn missing_identity() -> SignalProtocolError {
    SignalProtocolError::InvalidState("Signal store", "local identity is unavailable".into())
}

fn corrupt_record() -> SignalProtocolError {
    SignalProtocolError::InvalidProtobufEncoding
}

fn canonical(bytes: &[u8], encoded: Zeroizing<Vec<u8>>) -> Result<()> {
    if bytes.is_empty() || bytes != encoded.as_slice() {
        return Err(corrupt_record());
    }
    Ok(())
}

fn check_ec_pair(pair: &KeyPair) -> Result<()> {
    let derived = pair.private_key.public_key().map_err(|_| corrupt_record())?;
    if derived != pair.public_key {
        return Err(corrupt_record());
    }
    Ok(())
}

pub(super) fn decode_identity_pair(bytes: &[u8]) -> Result<IdentityKeyPair> {
    let pair = IdentityKeyPair::try_from(bytes).map_err(|_| corrupt_record())?;
    check_ec_pair(&pair.into())?;
    let encoded = Zeroizing::new(pair.serialize().into_vec());
    canonical(bytes, encoded)?;
    Ok(pair)
}

pub(super) fn decode_ec_prekey(id: u32, bytes: &[u8]) -> Result<PreKeyRecord> {
    let record = PreKeyRecord::deserialize(bytes)?;
    if record.id()? != id.into() {
        return Err(corrupt_record());
    }
    let pair = record.key_pair().map_err(|_| corrupt_record())?;
    check_ec_pair(&pair)?;
    let normalized = PreKeyRecord::new(id.into(), &pair);
    canonical(bytes, Zeroizing::new(normalized.serialize()?))?;
    Ok(normalized)
}

pub(super) fn decode_signed_prekey(id: u32, bytes: &[u8]) -> Result<SignedPreKeyRecord> {
    let record = SignedPreKeyRecord::deserialize(bytes)?;
    let signature = record.signature()?;
    if record.id()? != id.into() || signature.len() != SIGNATURE_LENGTH {
        return Err(corrupt_record());
    }
    let pair = record.key_pair().map_err(|_| corrupt_record())?;
    check_ec_pair(&pair)?;
    let normalized = SignedPreKeyRecord::new(id.into(), record.timestamp()?, &pair, &signature);
    canonical(bytes, Zeroizing::new(normalized.serialize()?))?;
    Ok(normalized)
}

pub(super) fn decode_kyber_prekey(id: u32, bytes: &[u8]) -> Result<KyberPreKeyRecord> {
    let record = KyberPreKeyRecord::deserialize(bytes)?;
    let signature = record.signature()?;
    if record.id()? != id.into() || signature.len() != SIGNATURE_LENGTH {
        return Err(corrupt_record());
    }
    let pair = record.key_pair().map_err(|_| corrupt_record())?;
    if pair.public_key.key_type() != kem::KeyType::Kyber1024 {
        return Err(corrupt_record());
    }
    let normalized = KyberPreKeyRecord::new(id.into(), record.timestamp()?, &pair, &signature);
    canonical(bytes, Zeroizing::new(normalized.serialize()?))?;
    Ok(normalized)
}

fn address_parts(address: &ProtocolAddress) -> Result<(&str, u32)> {
    let device_id = u32::from(address.device_id());
    let peer = address.name();
    if device_id != 1 || !valid_void_id(peer) {
        return Err(SignalProtocolError::InvalidArgument("invalid Signal address".into()));
    }
    Ok((peer, device_id))
}

impl<'tx, 'conn> SignalStore<'tx, 'conn> {
    pub(in crate::storage) fn new(tx: &'tx Transaction<'conn>) -> Self {
        Self { tx }
    }

    fn prekey(&self, kind: PreKeyKind, id: u32) -> Result<Zeroizing<Vec<u8>>> {
        kind.check_id(id)?;
        self.tx
            .query_row("SELECT record FROM signal_prekeys WHERE kind = ?1 AND id = ?2",
                params![kind.as_str(), id], |row| row.get(0))
            .optional()
            .map_err(storage_error)?
            .map(Zeroizing::new)
            .ok_or_else(|| kind.invalid_id())
    }

    fn save_prekey(&self, kind: PreKeyKind, id: u32, bytes: &[u8]) -> Result<()> {
        let old: Option<Vec<u8>> = self.tx
            .query_row("SELECT record FROM signal_prekeys WHERE kind = ?1 AND id = ?2",
                params![kind.as_str(), id], |row| row.get(0))
            .optional().map_err(storage_error)?;
        if let Some(old) = old.map(Zeroizing::new) {
            // Only identical retries are allowed, regardless of current publication state.
            return if bytes == old.as_slice() { Ok(()) } else { Err(kind.invalid_id()) };
        }
        let next_id = self.tx
            .query_row(
                "SELECT next_prekey_id FROM signal_local_identity WHERE slot = 1 AND key_pair IS NOT NULL",
                [], |row| row.get::<_, u32>(0),
            )
            .optional().map_err(storage_error)?.ok_or_else(missing_identity)?;
        if next_id == 0 || id < next_id {
            return Err(kind.invalid_id());
        }
        self.tx.execute(
            "INSERT INTO signal_prekeys (kind, id, record) VALUES (?1, ?2, ?3)",
            params![kind.as_str(), id, bytes],
        ).map_err(storage_error)?;
        self.tx.execute(
            "UPDATE signal_local_identity SET next_prekey_id = ?1 WHERE slot = 1",
            [id + 1],
        ).map_err(storage_error)?;
        Ok(())
    }

    fn remove_prekey(&self, kind: PreKeyKind, id: u32) -> Result<()> {
        kind.check_id(id)?;
        self.tx.execute("DELETE FROM signal_prekeys WHERE kind = ?1 AND id = ?2",
            params![kind.as_str(), id]).map_err(storage_error)?;
        Ok(())
    }

    fn verify_prekey_signature(&self, public_key: &[u8], signature: &[u8]) -> Result<()> {
        let identity = self.local_identity()?;
        if !identity.public_key().verify_signature(public_key, signature) {
            return Err(SignalProtocolError::SignatureValidationFailed);
        }
        Ok(())
    }

    fn local_identity(&self) -> Result<IdentityKeyPair> {
        let bytes = self.tx.query_row(
            "SELECT key_pair FROM signal_local_identity WHERE slot = 1 AND key_pair IS NOT NULL",
            [], |row| row.get::<_, Vec<u8>>(0),
        ).optional().map_err(storage_error)?.ok_or_else(missing_identity)?;
        decode_identity_pair(&Zeroizing::new(bytes))
    }

    fn trusted_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>> {
        let (peer, device_id) = address_parts(address)?;
        self.tx.query_row(
            "SELECT identity_key FROM signal_identities WHERE peer_void_id = ?1 AND device_id = ?2",
            params![peer, device_id], |row| row.get::<_, Vec<u8>>(0),
        ).optional().map_err(storage_error)?.map(|bytes| {
            let key = IdentityKey::decode(&bytes).map_err(|_| corrupt_record())?;
            if key.serialize().as_ref() != bytes.as_slice() {
                return Err(corrupt_record());
            }
            Ok(key)
        }).transpose()
    }
}

#[async_trait(?Send)]
impl IdentityKeyStore for SignalStore<'_, '_> {
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair> {
        self.local_identity()
    }

    async fn get_local_registration_id(&self) -> Result<u32> {
        let id = self.tx.query_row(
            "SELECT registration_id FROM signal_local_identity WHERE slot = 1 AND key_pair IS NOT NULL",
            [], |row| row.get::<_, u32>(0),
        ).optional().map_err(storage_error)?.ok_or_else(missing_identity)?;
        if !(1..=16_380).contains(&id) {
            return Err(corrupt_record());
        }
        Ok(id)
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<IdentityChange> {
        // As on Android, only a future coordinator with a verified account binding may pin.
        if self.trusted_identity(address)?.as_ref() != Some(identity) {
            return Err(SignalProtocolError::UntrustedIdentity(address.clone()));
        }
        Ok(IdentityChange::NewOrUnchanged)
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> Result<bool> {
        Ok(self.trusted_identity(address)?.as_ref() == Some(identity))
    }

    async fn get_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>> {
        self.trusted_identity(address)
    }
}

#[async_trait(?Send)]
impl SessionStore for SignalStore<'_, '_> {
    async fn load_session(&self, address: &ProtocolAddress) -> Result<Option<SessionRecord>> {
        let (peer, device_id) = address_parts(address)?;
        self.tx.query_row(
            "SELECT record FROM signal_sessions WHERE peer_void_id = ?1 AND device_id = ?2",
            params![peer, device_id], |row| row.get::<_, Vec<u8>>(0),
        ).optional().map_err(storage_error)?.map(|bytes| {
            let bytes = Zeroizing::new(bytes);
            let record = SessionRecord::deserialize(&bytes).map_err(|_| corrupt_record())?;
            canonical(&bytes, Zeroizing::new(record.serialize()?))?;
            Ok(record)
        }).transpose()
    }

    async fn store_session(&mut self, address: &ProtocolAddress, record: &SessionRecord) -> Result<()> {
        let (peer, device_id) = address_parts(address)?;
        let bytes = Zeroizing::new(record.serialize()?);
        if bytes.is_empty() {
            return Err(corrupt_record());
        }
        self.tx.execute(
            "INSERT INTO signal_sessions (peer_void_id, device_id, record) VALUES (?1, ?2, ?3)
             ON CONFLICT (peer_void_id, device_id) DO UPDATE SET record = excluded.record",
            params![peer, device_id, bytes.as_slice()],
        ).map_err(storage_error)?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl PreKeyStore for SignalStore<'_, '_> {
    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord> {
        let id = prekey_id.into();
        decode_ec_prekey(id, &self.prekey(PreKeyKind::Ec, id)?)
    }

    async fn save_pre_key(&mut self, prekey_id: PreKeyId, record: &PreKeyRecord) -> Result<()> {
        let id = prekey_id.into();
        PreKeyKind::Ec.check_id(id)?;
        if record.id()? != prekey_id {
            return Err(PreKeyKind::Ec.invalid_id());
        }
        let bytes = Zeroizing::new(record.serialize()?);
        decode_ec_prekey(id, &bytes)?;
        self.save_prekey(PreKeyKind::Ec, id, &bytes)
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<()> {
        self.remove_prekey(PreKeyKind::Ec, prekey_id.into())
    }
}

#[async_trait(?Send)]
impl SignedPreKeyStore for SignalStore<'_, '_> {
    async fn get_signed_pre_key(&self, signed_prekey_id: SignedPreKeyId) -> Result<SignedPreKeyRecord> {
        let id = signed_prekey_id.into();
        let record = decode_signed_prekey(id, &self.prekey(PreKeyKind::Signed, id)?)?;
        self.verify_prekey_signature(&record.public_key()?.serialize(), &record.signature()?)?;
        Ok(record)
    }

    async fn save_signed_pre_key(
        &mut self,
        signed_prekey_id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<()> {
        let id = signed_prekey_id.into();
        PreKeyKind::Signed.check_id(id)?;
        if record.id()? != signed_prekey_id {
            return Err(PreKeyKind::Signed.invalid_id());
        }
        let bytes = Zeroizing::new(record.serialize()?);
        let record = decode_signed_prekey(id, &bytes)?;
        self.verify_prekey_signature(&record.public_key()?.serialize(), &record.signature()?)?;
        self.save_prekey(PreKeyKind::Signed, id, &bytes)
    }
}

#[async_trait(?Send)]
impl KyberPreKeyStore for SignalStore<'_, '_> {
    async fn get_kyber_pre_key(&self, kyber_prekey_id: KyberPreKeyId) -> Result<KyberPreKeyRecord> {
        let id = kyber_prekey_id.into();
        let record = decode_kyber_prekey(id, &self.prekey(PreKeyKind::Kyber, id)?)?;
        self.verify_prekey_signature(&record.public_key()?.serialize(), &record.signature()?)?;
        Ok(record)
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<()> {
        let id = kyber_prekey_id.into();
        PreKeyKind::Kyber.check_id(id)?;
        if record.id()? != kyber_prekey_id {
            return Err(PreKeyKind::Kyber.invalid_id());
        }
        let bytes = Zeroizing::new(record.serialize()?);
        let record = decode_kyber_prekey(id, &bytes)?;
        self.verify_prekey_signature(&record.public_key()?.serialize(), &record.signature()?)?;
        self.save_prekey(PreKeyKind::Kyber, id, &bytes)
    }

    async fn mark_kyber_pre_key_used(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        signed_prekey_id: SignedPreKeyId,
        base_key: &PublicKey,
    ) -> Result<()> {
        let kyber_id = kyber_prekey_id.into();
        let signed_id = signed_prekey_id.into();
        PreKeyKind::Kyber.check_id(kyber_id)?;
        PreKeyKind::Signed.check_id(signed_id)?;
        let last_resort: bool = self.tx.query_row(
            "SELECT last_resort FROM signal_prekeys WHERE kind = 'kyber' AND id = ?1",
            [kyber_id], |row| row.get(0),
        ).optional().map_err(storage_error)?.ok_or(SignalProtocolError::InvalidKyberPreKeyId)?;
        if !last_resort {
            return self.remove_prekey(PreKeyKind::Kyber, kyber_id);
        }
        let encoded = base_key.serialize();
        let inserted = self.tx.execute(
            "INSERT INTO signal_kyber_uses (kyber_prekey_id, signed_prekey_id, base_key)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (kyber_prekey_id, signed_prekey_id, base_key) DO NOTHING",
            params![kyber_id, signed_id, encoded.as_ref()],
        ).map_err(storage_error)?;
        if inserted != 1 {
            return Err(SignalProtocolError::InvalidMessage(
                CiphertextMessageType::PreKey, "reused base key".into(),
            ));
        }
        Ok(())
    }
}

impl ProtocolStore for SignalStore<'_, '_> {}
