//! Serialization stays inside Rust. There is deliberately no disk/export FFI method.
use crate::{
    MlsError, OPENMLS_VERSION,
    engine::{ClientState, SUITE},
};
use openmls::prelude::{GroupId, MlsGroup};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeroize::{Zeroize, Zeroizing};

const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    format: u32,
    openmls_version: String,
    identity: Vec<u8>,
    signature_public_key: Vec<u8>,
    group_ids: Vec<Vec<u8>>,
    key_package_count: u32,
    // Byte-vector map keys need this representation in self-describing JSON.
    storage: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for (_, value) in &mut self.storage {
            value.zeroize();
        }
    }
}

pub(crate) fn serialize(client: &ClientState) -> Result<Zeroizing<Vec<u8>>, MlsError> {
    let storage = client
        .provider
        .storage()
        .values
        .read()
        .map_err(|_| MlsError::StorageFailure)?;
    let snapshot = Snapshot {
        format: 1,
        openmls_version: OPENMLS_VERSION.into(),
        identity: client.identity.clone(),
        signature_public_key: client.signer.to_public_vec(),
        group_ids: client.groups.keys().cloned().collect(),
        key_package_count: client.key_package_count,
        storage: storage
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    };
    let bytes =
        Zeroizing::new(serde_json::to_vec(&snapshot).map_err(|_| MlsError::StorageFailure)?);
    if bytes.len() > MAX_SNAPSHOT {
        return Err(MlsError::ResourceLimit);
    }
    Ok(bytes)
}

pub(crate) fn restore(bytes: &[u8]) -> Result<ClientState, MlsError> {
    if bytes.len() > MAX_SNAPSHOT {
        return Err(MlsError::InvalidSnapshot);
    }
    let snapshot: Snapshot =
        serde_json::from_slice(bytes).map_err(|_| MlsError::InvalidSnapshot)?;
    if snapshot.format != 1 || snapshot.openmls_version != OPENMLS_VERSION {
        return Err(MlsError::InvalidSnapshot);
    }
    let provider = OpenMlsRustCrypto::default();
    {
        let mut values = provider
            .storage()
            .values
            .write()
            .map_err(|_| MlsError::StorageFailure)?;
        for (key, value) in &snapshot.storage {
            values.insert(key.clone(), value.clone());
        }
    }
    let signer = match SignatureKeyPair::read(
        provider.storage(),
        &snapshot.signature_public_key,
        SUITE.signature_algorithm(),
    ) {
        Some(signer) => signer,
        None => {
            if let Ok(mut values) = provider.storage().values.write() {
                for value in values.values_mut() {
                    value.zeroize();
                }
                values.clear();
            }
            return Err(MlsError::InvalidSnapshot);
        }
    };
    // Construct the wiping owner before loading groups so error paths also clear storage.
    let mut client = ClientState {
        provider,
        signer,
        identity: snapshot.identity.clone(),
        groups: HashMap::new(),
        key_package_count: snapshot.key_package_count,
    };
    for id in &snapshot.group_ids {
        let group = MlsGroup::load(client.provider.storage(), &GroupId::from_slice(id))
            .map_err(|_| MlsError::InvalidSnapshot)?
            .ok_or(MlsError::InvalidSnapshot)?;
        if group.ciphersuite() != SUITE {
            return Err(MlsError::InvalidSnapshot);
        }
        client.groups.insert(id.clone(), group);
    }
    Ok(client)
}
