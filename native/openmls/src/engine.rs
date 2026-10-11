use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use openmls::prelude::{
    tls_codec::{Deserialize, Serialize},
    *,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{OpenMlsProvider, crypto::OpenMlsCrypto, random::OpenMlsRand};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AddMemberResult, CIPHERSUITE_NAME, ClientInfo, DecryptedMessage, EngineInfo, GroupInfo,
    MlsError, OPENMLS_VERSION, SelfTestReport, persistence,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(crate) const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
pub(crate) const MAX_WIRE: usize = 1024 * 1024;
const MAX_CLIENTS: usize = 64;
const MAX_GROUPS: usize = 32;
const MAX_MEMBERS: usize = 128;
const MAX_KEY_PACKAGES: u32 = 64;

pub(crate) struct ClientState {
    pub(crate) provider: OpenMlsRustCrypto,
    pub(crate) signer: SignatureKeyPair,
    pub(crate) identity: Vec<u8>,
    pub(crate) groups: HashMap<Vec<u8>, MlsGroup>,
    pub(crate) key_package_count: u32,
}

impl ClientState {
    fn new(identity: Vec<u8>) -> Result<Self, MlsError> {
        let provider = OpenMlsRustCrypto::default();
        provider
            .crypto()
            .supports(SUITE)
            .map_err(|_| MlsError::CryptoFailure)?;
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm())
            .map_err(|_| MlsError::CryptoFailure)?;
        signer
            .store(provider.storage())
            .map_err(|_| MlsError::StorageFailure)?;
        Ok(Self {
            provider,
            signer,
            identity,
            groups: HashMap::new(),
            key_package_count: 0,
        })
    }

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(self.identity.clone()).into(),
            signature_key: self.signer.public().into(),
        }
    }

    fn info(&self, handle: String) -> ClientInfo {
        ClientInfo {
            client_handle: handle,
            credential_identity: self.identity.clone(),
            signature_public_key: self.signer.to_public_vec(),
        }
    }
}

impl Drop for ClientState {
    fn drop(&mut self) {
        // OpenMLS secret types zeroize on drop. Also wipe the serialized storage copies.
        self.groups.clear();
        if let Ok(mut values) = self.provider.storage().values.write() {
            for value in values.values_mut() {
                value.zeroize();
            }
            values.clear();
        }
    }
}

#[derive(Default)]
struct EngineState {
    clients: HashMap<String, ClientState>,
    snapshots: HashMap<String, Zeroizing<Vec<u8>>>,
}

/// One mutex serializes all mutations, including ratchet advancement and snapshots.
#[derive(uniffi::Object)]
pub struct MlsEngine {
    state: Mutex<EngineState>,
}

fn input(bytes: &[u8], max: usize) -> Result<(), MlsError> {
    if bytes.is_empty() || bytes.len() > max {
        Err(MlsError::InvalidInput)
    } else {
        Ok(())
    }
}

fn parse(bytes: &[u8]) -> Result<MlsMessageIn, MlsError> {
    input(bytes, MAX_WIRE)?;
    // Use the Read-based parser and require exactly one MLS message.
    let mut cursor = bytes;
    let message =
        MlsMessageIn::tls_deserialize(&mut cursor).map_err(|_| MlsError::InvalidMessage)?;
    if !cursor.is_empty() {
        return Err(MlsError::InvalidMessage);
    }
    Ok(message)
}

fn wire(message: &impl Serialize) -> Result<Vec<u8>, MlsError> {
    let bytes = message
        .tls_serialize_detached()
        .map_err(|_| MlsError::CryptoFailure)?;
    if bytes.len() > MAX_WIRE {
        return Err(MlsError::ResourceLimit);
    }
    Ok(bytes)
}

fn group_info(group: &MlsGroup) -> GroupInfo {
    GroupInfo {
        group_id: group.group_id().as_slice().to_vec(),
        epoch: group.epoch().as_u64(),
        member_count: group.members().count() as u32,
    }
}

fn handle() -> Result<String, MlsError> {
    let bytes = OpenMlsRustCrypto::default()
        .rand()
        .random_vec(32)
        .map_err(|_| MlsError::CryptoFailure)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl MlsEngine {
    fn lock(&self) -> Result<MutexGuard<'_, EngineState>, MlsError> {
        self.state.lock().map_err(|_| MlsError::InvalidState)
    }

    fn with_client<T>(
        &self,
        client_handle: &str,
        f: impl FnOnce(&mut ClientState) -> Result<T, MlsError>,
    ) -> Result<T, MlsError> {
        let mut state = self.lock()?;
        f(state
            .clients
            .get_mut(client_handle)
            .ok_or(MlsError::UnknownClient)?)
    }
}

#[uniffi::export]
impl MlsEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(EngineState::default()),
        })
    }

    pub fn engine_info(&self) -> EngineInfo {
        EngineInfo {
            openmls_version: OPENMLS_VERSION.into(),
            ciphersuite: CIPHERSUITE_NAME.into(),
            protocol_version: "RFC 9420 / MLS 1.0".into(),
            durable_persistence: false,
        }
    }

    pub fn create_client(&self, credential_identity: Vec<u8>) -> Result<ClientInfo, MlsError> {
        input(&credential_identity, 1024)?;
        let mut state = self.lock()?;
        if state.clients.len() + state.snapshots.len() >= MAX_CLIENTS {
            return Err(MlsError::ResourceLimit);
        }
        let client = ClientState::new(credential_identity)?;
        let handle = handle()?;
        let info = client.info(handle.clone());
        state.clients.insert(handle, client);
        Ok(info)
    }

    pub fn generate_key_package(&self, client_handle: String) -> Result<Vec<u8>, MlsError> {
        self.with_client(&client_handle, |client| {
            if client.key_package_count >= MAX_KEY_PACKAGES {
                return Err(MlsError::ResourceLimit);
            }
            let package = KeyPackage::builder()
                .build(SUITE, &client.provider, &client.signer, client.credential())
                .map_err(|_| MlsError::CryptoFailure)?;
            client.key_package_count += 1;
            wire(&MlsMessageOut::from(package.key_package().clone()))
        })
    }

    pub fn create_group(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        input(&group_id, 256)?;
        self.with_client(&client_handle, |client| {
            if client.groups.contains_key(&group_id) {
                return Err(MlsError::GroupAlreadyExists);
            }
            if client.groups.len() >= MAX_GROUPS {
                return Err(MlsError::ResourceLimit);
            }
            let config = MlsGroupCreateConfig::builder()
                .ciphersuite(SUITE)
                .use_ratchet_tree_extension(true)
                .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
                .build();
            let group = MlsGroup::new_with_group_id(
                &client.provider,
                &client.signer,
                &config,
                GroupId::from_slice(&group_id),
                client.credential(),
            )
            .map_err(|_| MlsError::CryptoFailure)?;
            let info = group_info(&group);
            client.groups.insert(group_id, group);
            Ok(info)
        })
    }

    pub fn add_member(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        key_package: Vec<u8>,
    ) -> Result<AddMemberResult, MlsError> {
        let MlsMessageBodyIn::KeyPackage(package) = parse(&key_package)?.extract() else {
            return Err(MlsError::InvalidMessage);
        };
        self.with_client(&client_handle, |client| {
            let package = package
                .validate(client.provider.crypto(), ProtocolVersion::Mls10)
                .map_err(|_| MlsError::InvalidMessage)?;
            if package.ciphersuite() != SUITE {
                return Err(MlsError::InvalidMessage);
            }
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            if group.members().count() >= MAX_MEMBERS {
                return Err(MlsError::ResourceLimit);
            }
            let (commit, welcome, _) = group
                .add_members(&client.provider, &client.signer, &[package])
                .map_err(|_| MlsError::InvalidState)?;
            Ok(AddMemberResult {
                commit: wire(&commit)?,
                welcome: wire(&welcome)?,
            })
        })
    }

    pub fn merge_pending_commit(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            if group.pending_commit().is_none() {
                return Err(MlsError::InvalidState);
            }
            group
                .merge_pending_commit(&client.provider)
                .map_err(|_| MlsError::InvalidState)?;
            Ok(group_info(group))
        })
    }

    pub fn process_welcome(
        &self,
        client_handle: String,
        welcome: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        let MlsMessageBodyIn::Welcome(welcome) = parse(&welcome)?.extract() else {
            return Err(MlsError::InvalidMessage);
        };
        if welcome.ciphersuite() != SUITE {
            return Err(MlsError::InvalidMessage);
        }
        self.with_client(&client_handle, |client| {
            if client.groups.len() >= MAX_GROUPS {
                return Err(MlsError::ResourceLimit);
            }
            let config = MlsGroupJoinConfig::builder()
                .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
                .build();
            let staged = StagedWelcome::new_from_welcome(&client.provider, &config, welcome, None)
                .map_err(|_| MlsError::InvalidMessage)?;
            let group_id = staged.group_context().group_id().as_slice().to_vec();
            input(&group_id, 256)?;
            if staged.members().count() > MAX_MEMBERS {
                return Err(MlsError::ResourceLimit);
            }
            if client.groups.contains_key(&group_id) {
                return Err(MlsError::GroupAlreadyExists);
            }
            let group = staged
                .into_group(&client.provider)
                .map_err(|_| MlsError::InvalidState)?;
            let info = group_info(&group);
            client.groups.insert(group_id, group);
            Ok(info)
        })
    }

    pub fn encrypt(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        plaintext: Vec<u8>,
    ) -> Result<Vec<u8>, MlsError> {
        // Empty application content is valid MLS; bound the size before cryptography.
        if plaintext.len() > 64 * 1024 {
            return Err(MlsError::InvalidInput);
        }
        self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            if group.pending_commit().is_some() {
                return Err(MlsError::InvalidState);
            }
            wire(
                &group
                    .create_message(&client.provider, &client.signer, &plaintext)
                    .map_err(|_| MlsError::InvalidState)?,
            )
        })
    }

    pub fn decrypt(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        message: Vec<u8>,
    ) -> Result<DecryptedMessage, MlsError> {
        let protocol = parse(&message)?
            .try_into_protocol_message()
            .map_err(|_| MlsError::InvalidMessage)?;
        if protocol.content_type() != ContentType::Application {
            return Err(MlsError::InvalidMessage);
        }
        self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            let processed = group
                .process_message(&client.provider, protocol)
                .map_err(|_| MlsError::InvalidMessage)?;
            let sender_identity = BasicCredential::try_from(processed.credential().clone())
                .map_err(|_| MlsError::InvalidMessage)?
                .identity()
                .to_vec();
            let Sender::Member(index) = processed.sender() else {
                return Err(MlsError::InvalidMessage);
            };
            let member = group
                .members()
                .find(|member| member.index == *index)
                .ok_or(MlsError::InvalidMessage)?;
            if member.credential != *processed.credential() {
                return Err(MlsError::InvalidMessage);
            }
            let epoch = processed.epoch().as_u64();
            let ProcessedMessageContent::ApplicationMessage(content) = processed.into_content()
            else {
                return Err(MlsError::InvalidMessage);
            };
            Ok(DecryptedMessage {
                plaintext: content.into_bytes(),
                sender_identity,
                sender_signature_public_key: member.signature_key,
                epoch,
            })
        })
    }

    pub fn self_update(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
    ) -> Result<Vec<u8>, MlsError> {
        self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            let bundle = group
                .self_update(
                    &client.provider,
                    &client.signer,
                    LeafNodeParameters::default(),
                )
                .map_err(|_| MlsError::InvalidState)?;
            wire(bundle.commit())
        })
    }

    pub fn process_commit(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        message: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        let protocol = parse(&message)?
            .try_into_protocol_message()
            .map_err(|_| MlsError::InvalidMessage)?;
        if protocol.content_type() != ContentType::Commit {
            return Err(MlsError::InvalidMessage);
        }
        self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            if group.pending_commit().is_some() {
                return Err(MlsError::InvalidState);
            }
            let processed = group
                .process_message(&client.provider, protocol)
                .map_err(|_| MlsError::InvalidMessage)?;
            let ProcessedMessageContent::StagedCommitMessage(commit) = processed.into_content()
            else {
                return Err(MlsError::InvalidMessage);
            };
            let count = group
                .members()
                .count()
                .saturating_add(commit.add_proposals().count())
                .saturating_sub(commit.remove_proposals().count());
            if count > MAX_MEMBERS {
                return Err(MlsError::ResourceLimit);
            }
            group
                .merge_staged_commit(&client.provider, *commit)
                .map_err(|_| MlsError::InvalidState)?;
            Ok(group_info(group))
        })
    }

    pub fn group_info(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        self.with_client(&client_handle, |client| {
            Ok(group_info(
                client.groups.get(&group_id).ok_or(MlsError::UnknownGroup)?,
            ))
        })
    }

    /// Transfers the only live state to a single-use native memory snapshot.
    pub fn suspend_client(&self, client_handle: String) -> Result<String, MlsError> {
        let mut state = self.lock()?;
        let client = state
            .clients
            .get(&client_handle)
            .ok_or(MlsError::UnknownClient)?;
        let snapshot = persistence::serialize(client)?;
        let handle = handle()?;
        state.clients.remove(&client_handle);
        state.snapshots.insert(handle.clone(), snapshot);
        Ok(handle)
    }

    pub fn restore_client(&self, snapshot_handle: String) -> Result<ClientInfo, MlsError> {
        let mut state = self.lock()?;
        let snapshot = state
            .snapshots
            .remove(&snapshot_handle)
            .ok_or(MlsError::InvalidSnapshot)?;
        let client = persistence::restore(&snapshot)?;
        let handle = handle()?;
        let info = client.info(handle.clone());
        state.clients.insert(handle, client);
        Ok(info)
    }

    pub fn release_client(&self, client_handle: String) -> Result<(), MlsError> {
        self.lock()?
            .clients
            .remove(&client_handle)
            .ok_or(MlsError::UnknownClient)?;
        Ok(())
    }

    pub fn discard_snapshot(&self, snapshot_handle: String) -> Result<(), MlsError> {
        self.lock()?
            .snapshots
            .remove(&snapshot_handle)
            .ok_or(MlsError::InvalidSnapshot)?;
        Ok(())
    }

    /// Diagnostic fixtures only. Android exposes this method only in debug builds.
    pub fn run_alice_bob_test(&self) -> Result<SelfTestReport, MlsError> {
        crate::self_test::run()
    }
}
