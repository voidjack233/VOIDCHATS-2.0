use super::{
    DEVICE_ID, Error, Result, binding,
    wire::{SignalBinding, decode_base64, decode_exact_base64},
};
use crate::account::{timestamp_ms, valid_void_id};
use ed25519_dalek::VerifyingKey;
use libsignal_protocol::{DeviceId, IdentityKey, PreKeyBundle, PublicKey, kem};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CLAIM_BYTES: usize = 32 * 1024;
const MAX_PREKEY_ID: u32 = 16_777_214;

/// The registered Ed25519 public key for a peer, authenticated independently of its
/// prekey claim or message envelope. This is distinct from manual user verification.
pub(crate) struct TrustedPeer {
    void_id: String,
    account_key: VerifyingKey,
}

impl TrustedPeer {
    /// Use only with an authenticated lookup result or an isolated test fixture.
    #[allow(dead_code)] // Production lookup is outside the local Signal phase.
    pub(crate) fn from_trusted_lookup(
        void_id: &str,
        account_public_key: &[u8; 32],
    ) -> Result<Self> {
        let account_key =
            VerifyingKey::from_bytes(account_public_key).map_err(|_| Error::SignalPeer)?;
        if !valid_void_id(void_id) || account_key.is_weak() {
            return Err(Error::SignalPeer);
        }
        Ok(Self {
            void_id: void_id.to_owned(),
            account_key,
        })
    }

    pub(crate) fn void_id(&self) -> &str {
        &self.void_id
    }
}

pub(super) struct VerifiedPeer<'a> {
    peer: &'a TrustedPeer,
    binding: SignalBinding,
    identity_key: IdentityKey,
}

impl VerifiedPeer<'_> {
    pub(super) fn identity_key(&self) -> &IdentityKey {
        &self.identity_key
    }
}

pub(super) fn verify_binding<'a>(
    peer: &'a TrustedPeer,
    binding: &SignalBinding,
) -> Result<VerifiedPeer<'a>> {
    if !binding::verify_binding(
        &peer.account_key,
        peer.void_id(),
        binding.device_id,
        binding.registration_id,
        &binding.identity_key,
        &binding.account_signature,
    ) {
        return Err(Error::SignalPeer);
    }

    let identity_key = IdentityKey::decode(&binding.identity_key).map_err(|_| Error::SignalPeer)?;
    if !identity_key.public_key().is_canonical() {
        return Err(Error::SignalPeer);
    }
    Ok(VerifiedPeer {
        peer,
        binding: *binding,
        identity_key,
    })
}

fn load_pin(connection: &Connection, peer: &TrustedPeer) -> Result<Option<SignalBinding>> {
    let stored = connection
        .query_row(
            "SELECT b.account_public_key, b.registration_id, i.identity_key, b.account_signature
             FROM signal_identities i LEFT JOIN signal_peer_bindings b USING (peer_void_id, device_id)
             WHERE i.peer_void_id = ?1 AND i.device_id = ?2",
            params![peer.void_id(), DEVICE_ID],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, Option<u32>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((account_key, registration, identity_key, signature)) = stored else {
        return Ok(None);
    };
    // Older Signal-only rows carry no proof of the account binding.
    if account_key.as_deref() != Some(peer.account_key.as_bytes().as_slice()) {
        return Err(Error::SignalPeer);
    }
    let binding = SignalBinding {
        device_id: DEVICE_ID,
        registration_id: registration.ok_or(Error::SignalPeer)?,
        identity_key: identity_key.try_into().map_err(|_| Error::SignalPeer)?,
        account_signature: signature
            .ok_or(Error::SignalPeer)?
            .try_into()
            .map_err(|_| Error::SignalPeer)?,
    };
    verify_binding(peer, &binding)?;
    Ok(Some(binding))
}

pub(super) fn check_pin(connection: &Connection, verified: &VerifiedPeer<'_>) -> Result<bool> {
    let Some(stored) = load_pin(connection, verified.peer)? else {
        return Ok(false);
    };
    if stored.identity_key != verified.binding.identity_key
        || stored.registration_id != verified.binding.registration_id
    {
        return Err(Error::SignalPeer);
    }
    Ok(true)
}

pub(super) fn pin(tx: &Transaction<'_>, verified: &VerifiedPeer<'_>) -> Result<()> {
    if check_pin(tx, verified)? {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO signal_identities (peer_void_id, device_id, identity_key) VALUES (?1, ?2, ?3)",
        params![
            verified.peer.void_id(),
            DEVICE_ID,
            &verified.binding.identity_key[..]
        ],
    )?;
    tx.execute(
        "INSERT INTO signal_peer_bindings
            (peer_void_id, device_id, account_public_key, registration_id, account_signature)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            verified.peer.void_id(),
            DEVICE_ID,
            &verified.peer.account_key.as_bytes()[..],
            verified.binding.registration_id,
            &verified.binding.account_signature[..]
        ],
    )?;
    Ok(())
}

pub(super) fn require_pinned(connection: &Connection, peer: &TrustedPeer) -> Result<()> {
    load_pin(connection, peer)?.ok_or(Error::SignalPeer)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    v: u32,
    device_id: u32,
    registration_id: u32,
    identity_key: String,
    account_signature: String,
    signed_prekey: SignedPreKey,
    one_time_prekey: Option<EcPreKey>,
    kyber_prekey: KyberPreKey,
    claim_expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EcPreKey {
    key_id: u32,
    public_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedPreKey {
    key_id: u32,
    public_key: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KyberPreKey {
    key_id: u32,
    public_key: String,
    signature: String,
    // libsignal processes either selected key the same way on the sender. The
    // receiver's own prekey record determines fallback handling and replay checks.
    #[serde(rename = "last_resort")]
    _last_resort: bool,
}

fn valid_prekey_id(id: u32) -> Result<u32> {
    (1..=MAX_PREKEY_ID)
        .contains(&id)
        .then_some(id)
        .ok_or(Error::SignalClaim)
}

fn ec_public_key(encoded: &str) -> Result<PublicKey> {
    let bytes = decode_exact_base64::<33>(encoded).map_err(|_| Error::SignalClaim)?;
    let key = PublicKey::deserialize(&bytes).map_err(|_| Error::SignalClaim)?;
    if !key.is_canonical() {
        return Err(Error::SignalClaim);
    }
    Ok(key)
}

pub(super) fn verify_claim<'a>(
    peer: &'a TrustedPeer,
    claim_target_void_id: &str,
    encoded: &str,
    now: SystemTime,
) -> Result<(VerifiedPeer<'a>, PreKeyBundle)> {
    // The target comes from the authenticated prekey_bundle frame; its decoded
    // payload has no VOID-ID field. The account signature binds it independently.
    if claim_target_void_id != peer.void_id() {
        return Err(Error::SignalClaim);
    }
    let raw = decode_base64(encoded, MAX_CLAIM_BYTES).map_err(|_| Error::SignalClaim)?;
    let claim: Claim = serde_json::from_slice(&raw).map_err(|_| Error::SignalClaim)?;
    let expires_ms = timestamp_ms(&claim.claim_expires_at).ok_or(Error::SignalClaim)?;
    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .ok_or(Error::SignalClaim)?;
    if claim.v != 1 || expires_ms <= now_ms {
        return Err(Error::SignalClaim);
    }

    let binding = SignalBinding {
        device_id: claim.device_id,
        registration_id: claim.registration_id,
        identity_key: decode_exact_base64(&claim.identity_key).map_err(|_| Error::SignalClaim)?,
        account_signature: decode_exact_base64(&claim.account_signature)
            .map_err(|_| Error::SignalClaim)?,
    };
    let verified = verify_binding(peer, &binding)?;

    let signed_id = valid_prekey_id(claim.signed_prekey.key_id)?;
    let signed_key = ec_public_key(&claim.signed_prekey.public_key)?;
    let signed_signature = decode_exact_base64::<64>(&claim.signed_prekey.signature)
        .map_err(|_| Error::SignalClaim)?;
    let one_time = claim
        .one_time_prekey
        .map(|prekey| -> Result<_> {
            Ok((
                valid_prekey_id(prekey.key_id)?.into(),
                ec_public_key(&prekey.public_key)?,
            ))
        })
        .transpose()?;
    let kyber_id = valid_prekey_id(claim.kyber_prekey.key_id)?;
    let kyber_bytes =
        decode_base64(&claim.kyber_prekey.public_key, 3072).map_err(|_| Error::SignalClaim)?;
    let kyber_key = kem::PublicKey::deserialize(&kyber_bytes).map_err(|_| Error::SignalClaim)?;
    if kyber_key.key_type() != kem::KeyType::Kyber1024 {
        return Err(Error::SignalClaim);
    }
    let kyber_signature =
        decode_exact_base64::<64>(&claim.kyber_prekey.signature).map_err(|_| Error::SignalClaim)?;
    let signer = verified.identity_key().public_key();
    if !signer.verify_signature(&signed_key.serialize(), &signed_signature)
        || !signer.verify_signature(&kyber_bytes, &kyber_signature)
    {
        return Err(Error::SignalClaim);
    }

    let device_id = DeviceId::new(DEVICE_ID as u8).map_err(|_| Error::SignalClaim)?;
    let bundle = PreKeyBundle::new(
        claim.registration_id,
        device_id,
        one_time,
        signed_id.into(),
        signed_key,
        signed_signature.to_vec(),
        kyber_id.into(),
        kyber_key,
        kyber_signature.to_vec(),
        *verified.identity_key(),
    )
    .map_err(|_| Error::SignalClaim)?;
    Ok((verified, bundle))
}
