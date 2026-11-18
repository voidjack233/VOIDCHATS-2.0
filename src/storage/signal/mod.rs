mod adapters;
mod binding;
mod crypto;
pub(crate) mod peer;
mod publication;
mod recovery;
mod wire;
pub(crate) use wire::valid_content;
#[cfg(all(test, windows))]
mod crypto_tests;
#[cfg(all(test, windows))]
mod tests;

use super::{Database, Error, Owner, Result, verify_owner};
use adapters::{decode_identity_pair, decode_kyber_prekey, decode_signed_prekey};
use ed25519_dalek::SigningKey;
use libsignal_protocol::{
    GenericSignedPreKey, IdentityKeyPair, KeyPair, KyberPreKeyRecord, PreKeyRecord,
    SignedPreKeyRecord, Timestamp, kem,
};
use rand::{Rng, SeedableRng, rngs::StdRng};
use rusqlite::{OptionalExtension, Transaction, params};
use std::time::SystemTime;
use zeroize::Zeroizing;

const DEVICE_ID: u32 = 1;
const INITIAL_SIGNED_ID: u32 = 1;
const INITIAL_LAST_RESORT_ID: u32 = 2;
const INITIAL_ONE_TIME_COUNT: u32 = 20;
const NEXT_ID_AFTER_INITIAL: u32 = 3 + 2 * INITIAL_ONE_TIME_COUNT;

impl Database {
    pub(crate) fn initialize_signal(
        &mut self,
        void_id: &str,
        account_key: &SigningKey,
    ) -> Result<()> {
        let verifying_key = account_key.verifying_key();
        self.write(|tx| {
            verify_owner(tx, Owner { void_id, public_key: verifying_key.as_bytes() })?;
            let pristine: bool = tx.query_row(
                "SELECT key_pair IS NULL FROM signal_local_identity WHERE slot = 1",
                [], |row| row.get(0),
            ).optional()?.ok_or(Error::SignalState)?;

            if pristine {
                let has_records = tx.prepare(
                    "SELECT 1 FROM signal_prekeys UNION ALL SELECT 1 FROM signal_sessions
                     UNION ALL SELECT 1 FROM signal_identities UNION ALL SELECT 1 FROM signal_kyber_uses LIMIT 1",
                )?.exists([])?;
                // Publication receipts were added in schema v5.
                let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
                let has_publication = version >= 5
                    && tx.prepare("SELECT 1 FROM signal_publication LIMIT 1")?.exists([])?;
                if has_records || has_publication {
                    return Err(Error::SignalState);
                }
                generate_initial_state(tx, void_id, account_key)?;
            }
            verify_local_state(tx, void_id, &verifying_key)
        })
    }
}

fn generate_initial_state(
    tx: &Transaction<'_>,
    void_id: &str,
    account_key: &SigningKey,
) -> Result<()> {
    let mut rng = StdRng::try_from_os_rng().map_err(|_| Error::SignalState)?;
    let timestamp = Timestamp::try_from(SystemTime::now()).map_err(|_| Error::SignalState)?;
    let pair = IdentityKeyPair::generate(&mut rng);
    let registration_id: u32 = rng.random_range(1..=16_380);
    let signature = binding::sign_binding(
        account_key,
        void_id,
        DEVICE_ID,
        registration_id,
        &pair.identity_key().serialize(),
    )
    .ok_or(Error::SignalState)?;

    let signed = KeyPair::generate(&mut rng);
    let signed_signature = pair
        .private_key()
        .calculate_signature(&signed.public_key.serialize(), &mut rng)
        .map_err(|_| Error::SignalState)?;
    let signed = SignedPreKeyRecord::new(
        INITIAL_SIGNED_ID.into(),
        timestamp,
        &signed,
        &signed_signature,
    );
    insert_prekey(tx, "signed", INITIAL_SIGNED_ID, signed.serialize(), false)?;

    let insert_kyber = |rng: &mut StdRng, id: u32, last_resort| {
        let key = kem::KeyPair::generate(kem::KeyType::Kyber1024, rng);
        let signature = pair
            .private_key()
            .calculate_signature(&key.public_key.serialize(), rng)
            .map_err(|_| Error::SignalState)?;
        let record = KyberPreKeyRecord::new(id.into(), timestamp, &key, &signature);
        insert_prekey(tx, "kyber", id, record.serialize(), last_resort)
    };
    insert_kyber(&mut rng, INITIAL_LAST_RESORT_ID, true)?;
    for index in 0..INITIAL_ONE_TIME_COUNT {
        let id = 3 + 2 * index;
        let ec = PreKeyRecord::new(id.into(), &KeyPair::generate(&mut rng));
        insert_prekey(tx, "ec", id, ec.serialize(), false)?;
        insert_kyber(&mut rng, id + 1, false)?;
    }

    let encoded = Zeroizing::new(pair.serialize().into_vec());
    tx.execute(
        "UPDATE signal_local_identity SET key_pair = ?1, registration_id = ?2, binding_signature = ?3,
            next_prekey_id = ?4, signed_prekey_id = ?5, kyber_last_resort_id = ?6 WHERE slot = 1",
        params![encoded.as_slice(), registration_id, signature.as_slice(), NEXT_ID_AFTER_INITIAL,
            INITIAL_SIGNED_ID, INITIAL_LAST_RESORT_ID],
    )?;
    Ok(())
}

fn insert_prekey(
    tx: &Transaction<'_>,
    kind: &str,
    id: u32,
    record: std::result::Result<Vec<u8>, libsignal_protocol::SignalProtocolError>,
    last_resort: bool,
) -> Result<()> {
    let record = Zeroizing::new(record.map_err(|_| Error::SignalState)?);
    tx.execute(
        "INSERT INTO signal_prekeys (kind, id, record, last_resort) VALUES (?1, ?2, ?3, ?4)",
        params![kind, id, record.as_slice(), last_resort],
    )?;
    Ok(())
}

fn verify_local_state(
    tx: &Transaction<'_>,
    void_id: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
) -> Result<()> {
    let (device, encoded, registration, signature, signed_id, last_resort_id) = tx.query_row(
        "SELECT device_id, key_pair, registration_id, binding_signature, signed_prekey_id, kyber_last_resort_id
         FROM signal_local_identity WHERE slot = 1",
        [], |row| Ok((row.get::<_, u32>(0)?, Zeroizing::new(row.get::<_, Vec<u8>>(1)?),
            row.get::<_, u32>(2)?, row.get::<_, Vec<u8>>(3)?, row.get::<_, u32>(4)?, row.get::<_, u32>(5)?)),
    ).map_err(|_| Error::SignalState)?;
    let pair = decode_identity_pair(&encoded).map_err(|_| Error::SignalState)?;
    if !binding::verify_binding(
        verifying_key,
        void_id,
        device,
        registration,
        &pair.identity_key().serialize(),
        &signature,
    ) {
        return Err(Error::SignalState);
    }

    let load =
        |kind, id, last_resort| {
            tx.query_row(
            "SELECT record FROM signal_prekeys WHERE kind = ?1 AND id = ?2 AND last_resort = ?3",
            params![kind, id, last_resort], |row| row.get::<_, Vec<u8>>(0),
        ).map(Zeroizing::new).map_err(|_| Error::SignalState)
        };
    let signed = decode_signed_prekey(signed_id, &load("signed", signed_id, false)?)
        .map_err(|_| Error::SignalState)?;
    let last_resort = decode_kyber_prekey(last_resort_id, &load("kyber", last_resort_id, true)?)
        .map_err(|_| Error::SignalState)?;
    let keys_signed_by_identity =
        || -> std::result::Result<bool, libsignal_protocol::SignalProtocolError> {
            Ok(pair
                .public_key()
                .verify_signature(&signed.public_key()?.serialize(), &signed.signature()?)
                && pair.public_key().verify_signature(
                    &last_resort.public_key()?.serialize(),
                    &last_resort.signature()?,
                ))
        };
    if !keys_signed_by_identity().map_err(|_| Error::SignalState)? {
        return Err(Error::SignalState);
    }
    Ok(())
}
