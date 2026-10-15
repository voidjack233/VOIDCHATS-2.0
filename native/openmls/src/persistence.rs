//! Complete provider serialization stays in Rust and the trusted native storage
//! callback. Public engine methods never export secret snapshots.
use crate::{
    MlsError, OPENMLS_VERSION,
    engine::{
        ClientState, EngineState, MAX_CLIENTS, MAX_GROUPS, MAX_KEY_PACKAGES, MAX_MEMBERS, SUITE,
    },
};
use openmls::prelude::{GroupId, MlsGroup};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{OpenMlsProvider, storage::CURRENT_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeroize::{Zeroize, Zeroizing};

const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;
pub(crate) const MAX_ENGINE_SNAPSHOT: usize = 64 * 1024 * 1024;
const ENGINE_FORMAT: u32 = 1;
const PROVIDER_VERSION: &str = "openmls_rust_crypto=0.6.0;openmls_memory_storage=0.6.0";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        for (key, value) in &mut self.storage {
            key.zeroize();
            value.zeroize();
        }
    }
}

pub(crate) fn serialize(client: &ClientState) -> Result<Zeroizing<Vec<u8>>, MlsError> {
    let snapshot = client_snapshot(client)?;
    let mut writer = BoundedWriter {
        bytes: Zeroizing::new(Vec::new()),
        limit: MAX_SNAPSHOT,
    };
    serde_json::to_writer(&mut writer, &snapshot).map_err(|_| MlsError::ResourceLimit)?;
    Ok(writer.bytes)
}

fn client_snapshot(client: &ClientState) -> Result<Snapshot, MlsError> {
    let storage = client
        .provider
        .storage()
        .values
        .read()
        .map_err(|_| MlsError::StorageFailure)?;
    Ok(Snapshot {
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
    })
}

pub(crate) fn restore(bytes: &[u8]) -> Result<ClientState, MlsError> {
    if bytes.len() > MAX_SNAPSHOT {
        return Err(MlsError::InvalidSnapshot);
    }
    let snapshot: Snapshot =
        serde_json::from_slice(bytes).map_err(|_| MlsError::InvalidSnapshot)?;
    restore_snapshot(&snapshot)
}

fn restore_snapshot(snapshot: &Snapshot) -> Result<ClientState, MlsError> {
    if snapshot.format != 1
        || snapshot.openmls_version != OPENMLS_VERSION
        || snapshot.identity.is_empty()
        || snapshot.identity.len() > 1024
        || snapshot.signature_public_key.len() != 32
        || snapshot.group_ids.len() > MAX_GROUPS
        || snapshot.key_package_count > MAX_KEY_PACKAGES
    {
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
            if values.insert(key.clone(), value.clone()).is_some() {
                for value in values.values_mut() {
                    value.zeroize();
                }
                values.clear();
                return Err(MlsError::InvalidSnapshot);
            }
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
        if id.is_empty() || id.len() > 256 || client.groups.contains_key(id) {
            return Err(MlsError::InvalidSnapshot);
        }
        let group = MlsGroup::load(client.provider.storage(), &GroupId::from_slice(id))
            .map_err(|_| MlsError::InvalidSnapshot)?
            .ok_or(MlsError::InvalidSnapshot)?;
        if group.ciphersuite() != SUITE || group.members().count() > MAX_MEMBERS {
            return Err(MlsError::InvalidSnapshot);
        }
        client.groups.insert(id.clone(), group);
    }
    Ok(client)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineSnapshot {
    format: u32,
    revision: u64,
    openmls_version: String,
    provider_version: String,
    storage_version: u16,
    clients: Vec<(String, Snapshot)>,
    snapshots: Vec<(String, Vec<u8>)>,
}

impl Drop for EngineSnapshot {
    fn drop(&mut self) {
        for (_, bytes) in &mut self.snapshots {
            bytes.zeroize();
        }
    }
}

pub(crate) fn serialize_engine(
    state: &EngineState,
    revision: u64,
) -> Result<Zeroizing<Vec<u8>>, MlsError> {
    // Bound copied provider bytes before cloning any secret records. The JSON
    // writers below additionally cap the actual encoded client/engine sizes.
    let mut raw_size = state
        .snapshots
        .values()
        .try_fold(0usize, |size, bytes| size.checked_add(bytes.len()))
        .ok_or(MlsError::ResourceLimit)?;
    for client in state.clients.values() {
        let storage = client
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| MlsError::StorageFailure)?;
        for (key, value) in storage.iter() {
            raw_size = raw_size
                .checked_add(key.len())
                .and_then(|size| size.checked_add(value.len()))
                .ok_or(MlsError::ResourceLimit)?;
            if raw_size > MAX_ENGINE_SNAPSHOT {
                return Err(MlsError::ResourceLimit);
            }
        }
    }
    if raw_size > MAX_ENGINE_SNAPSHOT {
        return Err(MlsError::ResourceLimit);
    }
    let mut snapshot = EngineSnapshot {
        format: ENGINE_FORMAT,
        revision,
        openmls_version: OPENMLS_VERSION.into(),
        provider_version: PROVIDER_VERSION.into(),
        storage_version: CURRENT_VERSION,
        clients: Vec::new(),
        snapshots: Vec::new(),
    };
    // Apply a bounded writer while serializing instead of checking only after
    // a potentially huge allocation. The total serialized engine is capped.
    for (handle, client) in &state.clients {
        let client = client_snapshot(client)?;
        let mut counter = BoundedCounter { length: 0 };
        serde_json::to_writer(&mut counter, &client).map_err(|_| MlsError::ResourceLimit)?;
        snapshot.clients.push((handle.clone(), client));
    }
    for (handle, bytes) in &state.snapshots {
        snapshot.snapshots.push((handle.clone(), bytes.to_vec()));
    }
    let mut writer = BoundedWriter {
        bytes: Zeroizing::new(Vec::new()),
        limit: MAX_ENGINE_SNAPSHOT,
    };
    serde_json::to_writer(&mut writer, &snapshot).map_err(|_| MlsError::ResourceLimit)?;
    Ok(writer.bytes)
}

pub(crate) fn restore_engine(bytes: &[u8], revision: u64) -> Result<EngineState, MlsError> {
    if bytes.is_empty()
        || bytes.len() > MAX_ENGINE_SNAPSHOT
        || revision == 0
        || revision > i64::MAX as u64
    {
        return Err(MlsError::InvalidSnapshot);
    }
    let snapshot: EngineSnapshot =
        serde_json::from_slice(bytes).map_err(|_| MlsError::InvalidSnapshot)?;
    if snapshot.format != ENGINE_FORMAT
        || snapshot.revision != revision
        || snapshot.openmls_version != OPENMLS_VERSION
        || snapshot.provider_version != PROVIDER_VERSION
        || snapshot.storage_version != CURRENT_VERSION
        || snapshot.clients.len() + snapshot.snapshots.len() > MAX_CLIENTS
    {
        return Err(MlsError::InvalidSnapshot);
    }
    let mut state = EngineState {
        revision,
        ..EngineState::default()
    };
    for (handle, client) in &snapshot.clients {
        validate_handle(handle)?;
        if state.clients.contains_key(handle) {
            return Err(MlsError::InvalidSnapshot);
        }
        state
            .clients
            .insert(handle.clone(), restore_snapshot(client)?);
    }
    for (handle, bytes) in &snapshot.snapshots {
        validate_handle(handle)?;
        if state.clients.contains_key(handle) || state.snapshots.contains_key(handle) {
            return Err(MlsError::InvalidSnapshot);
        }
        // Validate suspended identity, groups, and provider records now, rather
        // than admitting corrupt state until a later restore operation.
        let _validated = restore(bytes)?;
        state
            .snapshots
            .insert(handle.clone(), Zeroizing::new(bytes.clone()));
    }
    Ok(state)
}

fn validate_handle(handle: &str) -> Result<(), MlsError> {
    if handle.len() == 64
        && handle
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(MlsError::InvalidSnapshot)
    }
}

struct BoundedWriter {
    bytes: Zeroizing<Vec<u8>>,
    limit: usize,
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("native snapshot size limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BoundedCounter {
    length: usize,
}

impl std::io::Write for BoundedCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > MAX_SNAPSHOT.saturating_sub(self.length) {
            return Err(std::io::Error::other("native client snapshot size limit"));
        }
        self.length += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
