use super::{adapters::SignalStore, crypto::local_signal_binding};
use crate::storage::{Database, Error, Result, messaging::new_event_id};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::FutureExt;
use libsignal_protocol::{GenericSignedPreKey, KyberPreKeyStore, PreKeyStore, SignedPreKeyStore};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) struct Publication {
    pub(crate) id: String,
    pub(crate) payload_base64: String,
}

#[derive(Deserialize, Serialize)]
struct PublicKey {
    key_id: u32,
    public_key: String,
}

#[derive(Deserialize, Serialize)]
struct SignedKey {
    key_id: u32,
    public_key: String,
    signature: String,
}

#[derive(Deserialize, Serialize)]
struct Upload {
    v: u8,
    device_id: u32,
    registration_id: u32,
    identity_key: String,
    account_signature: String,
    signed_prekey: SignedKey,
    kyber_last_resort: SignedKey,
    one_time_prekeys: Vec<PublicKey>,
    one_time_kyber_prekeys: Vec<SignedKey>,
}

impl Database {
    pub(crate) fn prepare_publication(&mut self) -> Result<Option<Publication>> {
        self.write(|tx| {
            let saved = tx.query_row(
                "SELECT publication_id, payload_base64, confirmed FROM signal_publication WHERE slot = 1",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, bool>(2)?)),
            ).optional()?;
            if let Some((id, payload_base64, confirmed)) = saved {
                return Ok((!confirmed).then_some(Publication { id, payload_base64 }));
            }
            let owner = tx.query_row("SELECT void_id FROM local_account WHERE singleton = 1", [],
                |row| row.get::<_, String>(0))?;
            let binding = local_signal_binding(tx, &owner)?;
            let (signed_id, fallback_id) = tx.query_row(
                "SELECT signed_prekey_id, kyber_last_resort_id FROM signal_local_identity WHERE slot = 1",
                [], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
            )?;
            let store = SignalStore::new(tx);
            let signed = store.get_signed_pre_key(signed_id.into()).now_or_never()
                .ok_or(Error::SignalState)?.map_err(|_| Error::SignalState)?;
            let kyber = |id: u32| -> Result<SignedKey> {
                let key = store.get_kyber_pre_key(id.into()).now_or_never()
                    .ok_or(Error::SignalState)?.map_err(|_| Error::SignalState)?;
                Ok(SignedKey { key_id: id,
                    public_key: STANDARD.encode(key.public_key().map_err(|_| Error::SignalState)?.serialize()),
                    signature: STANDARD.encode(key.signature().map_err(|_| Error::SignalState)?),
                })
            };
            let mut upload = Upload {
                v: 1, device_id: binding.device_id, registration_id: binding.registration_id,
                identity_key: STANDARD.encode(binding.identity_key),
                account_signature: STANDARD.encode(binding.account_signature),
                signed_prekey: SignedKey { key_id: signed_id,
                    public_key: STANDARD.encode(signed.public_key().map_err(|_| Error::SignalState)?.serialize()),
                    signature: STANDARD.encode(signed.signature().map_err(|_| Error::SignalState)?),
                },
                kyber_last_resort: kyber(fallback_id)?,
                one_time_prekeys: Vec::new(), one_time_kyber_prekeys: Vec::new(),
            };
            let mut query = tx.prepare(
                "SELECT kind, id FROM signal_prekeys WHERE kind IN ('ec', 'kyber')
                 AND last_resort = 0 AND published = 0 ORDER BY id",
            )?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let id: u32 = row.get(1)?;
                if row.get::<_, String>(0)? == "ec" {
                    let key = store.get_pre_key(id.into()).now_or_never().ok_or(Error::SignalState)?
                        .map_err(|_| Error::SignalState)?;
                    upload.one_time_prekeys.push(PublicKey { key_id: id,
                        public_key: STANDARD.encode(key.public_key().map_err(|_| Error::SignalState)?.serialize()),
                    });
                } else {
                    upload.one_time_kyber_prekeys.push(kyber(id)?);
                }
            }
            if upload.one_time_prekeys.len() > 64 || upload.one_time_kyber_prekeys.len() > 64 {
                return Err(Error::SignalState);
            }
            let id = new_event_id()?;
            let bytes = serde_json::to_vec(&upload).map_err(|_| Error::SignalState)?;
            if bytes.len() > 256 * 1024 { return Err(Error::SignalState); }
            let payload_base64 = STANDARD.encode(bytes);
            tx.execute("INSERT INTO signal_publication (slot, publication_id, payload_base64) VALUES (1, ?1, ?2)",
                params![id, payload_base64])?;
            Ok(Some(Publication { id, payload_base64 }))
        })
    }

    pub(crate) fn confirm_publication(&mut self, id: &str) -> Result<()> {
        self.write(|tx| {
            let encoded: String = tx.query_row(
                "SELECT payload_base64 FROM signal_publication WHERE slot = 1 AND publication_id = ?1", [id],
                |row| row.get(0),
            )?;
            let bytes = STANDARD.decode(encoded).map_err(|_| Error::SignalState)?;
            let upload: Upload = serde_json::from_slice(&bytes).map_err(|_| Error::SignalState)?;
            let mark = |kind, key_id| tx.execute(
                "UPDATE signal_prekeys SET published = 1 WHERE kind = ?1 AND id = ?2", params![kind, key_id],
            );
            mark("signed", upload.signed_prekey.key_id)?;
            mark("kyber", upload.kyber_last_resort.key_id)?;
            for key in upload.one_time_prekeys { mark("ec", key.key_id)?; }
            for key in upload.one_time_kyber_prekeys { mark("kyber", key.key_id)?; }
            tx.execute("UPDATE signal_publication SET confirmed = 1 WHERE slot = 1", [])?;
            Ok(())
        })
    }
}
