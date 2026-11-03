//! Narrow operations for account-bound two-member conversations. Private keys,
//! unvalidated plaintext and rejected state never escape this native boundary.
use super::*;
use crate::{DirectGroupInfo, DirectMessage};
use serde::Deserialize;

const MAX_PLAINTEXT: usize = 65_536;
const MAX_JS_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    client_message_id: String,
    conversation_id: String,
    from_void_id: String,
    to_void_id: String,
    body: String,
    created_at: u64,
}

impl Drop for Envelope {
    fn drop(&mut self) {
        self.body.zeroize();
    }
}

struct PendingDirectMessage {
    body: Zeroizing<String>,
    created_at: u64,
}

fn credential_identity(credential: &Credential) -> Result<Vec<u8>, MlsError> {
    let identity = BasicCredential::try_from(credential.clone())
        .map_err(|_| MlsError::InvalidMessage)?
        .identity()
        .to_vec();
    if identity.is_empty() || identity.len() > 1024 {
        return Err(MlsError::InvalidMessage);
    }
    Ok(identity)
}

fn peer_credential(
    members: impl Iterator<Item = Member>,
    own_index: LeafNodeIndex,
    own_key: &[u8],
    own_identity: &[u8],
    expected_peer_key: &[u8],
    expected_peer_identity: Option<&[u8]>,
) -> Result<Vec<u8>, MlsError> {
    let members: Vec<_> = members.collect();
    if members.len() != 2 || own_key == expected_peer_key {
        return Err(MlsError::InvalidMessage);
    }
    let own = members
        .iter()
        .find(|member| member.index == own_index)
        .ok_or(MlsError::InvalidMessage)?;
    if own.signature_key != own_key || credential_identity(&own.credential)? != own_identity {
        return Err(MlsError::InvalidMessage);
    }
    let peer = members
        .iter()
        .find(|member| member.index != own_index)
        .ok_or(MlsError::InvalidMessage)?;
    let identity = credential_identity(&peer.credential)?;
    if peer.signature_key != expected_peer_key
        || identity == own_identity
        || expected_peer_identity.is_some_and(|expected| identity != expected)
    {
        return Err(MlsError::InvalidMessage);
    }
    Ok(identity)
}

fn peer_inputs(key: &[u8], identity: Option<&[u8]>) -> Result<(), MlsError> {
    if key.len() != 32 {
        return Err(MlsError::InvalidInput);
    }
    if let Some(identity) = identity {
        input(identity, 1024)?;
    }
    Ok(())
}

fn valid_void_id(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|number| number > 0 && number.to_string() == value)
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

#[uniffi::export]
impl MlsEngine {
    /// Auth proof only. Android additionally validates account, audience, exact
    /// public key, canonical tokens and expiry against protected local state.
    pub fn sign_auth_challenge(
        &self,
        client_handle: String,
        message: Vec<u8>,
    ) -> Result<Vec<u8>, MlsError> {
        input(&message, 4096)?;
        let text = std::str::from_utf8(&message).map_err(|_| MlsError::InvalidInput)?;
        let fields: Vec<_> = text.split('\n').collect();
        if fields.len() != 8
            || fields[0] != "VOID-NOTHINGNESS:authenticate:v2"
            || !fields[7].is_empty()
            || fields[1..7]
                .iter()
                .any(|field| field.is_empty() || field.contains('\r'))
        {
            return Err(MlsError::InvalidInput);
        }
        self.read(|state| {
            state
                .clients
                .get(&client_handle)
                .ok_or(MlsError::UnknownClient)?
                .signer
                .sign(&message)
                .map_err(|_| MlsError::CryptoFailure)
        })
    }

    /// Parse and cryptographically verify the actual TLS KeyPackage before
    /// trusting any discovery metadata. This operation creates no MLS state.
    pub fn validate_key_package(
        &self,
        key_package: Vec<u8>,
        expected_peer_public_key: Vec<u8>,
    ) -> Result<Vec<u8>, MlsError> {
        peer_inputs(&expected_peer_public_key, None)?;
        let MlsMessageBodyIn::KeyPackage(package) = parse(&key_package)?.extract() else {
            return Err(MlsError::InvalidMessage);
        };
        self.read(|_| {
            let provider = OpenMlsRustCrypto::default();
            let package = package
                .validate(provider.crypto(), ProtocolVersion::Mls10)
                .map_err(|_| MlsError::InvalidMessage)?;
            if package.ciphersuite() != SUITE
                || package.leaf_node().signature_key().as_slice() != expected_peer_public_key
            {
                return Err(MlsError::InvalidMessage);
            }
            credential_identity(package.leaf_node().credential())
        })
    }

    /// KeyPackage consumption is checkpointed even on rejection. Membership,
    /// group and the Welcome author are checked while the group is still staged.
    pub fn process_welcome_checked(
        &self,
        client_handle: String,
        welcome: Vec<u8>,
        expected_group_id: Vec<u8>,
        expected_peer_public_key: Vec<u8>,
    ) -> Result<DirectGroupInfo, MlsError> {
        input(&expected_group_id, 256)?;
        peer_inputs(&expected_peer_public_key, None)?;
        let MlsMessageBodyIn::Welcome(welcome) = parse(&welcome)?.extract() else {
            return Err(MlsError::InvalidMessage);
        };
        if welcome.ciphersuite() != SUITE {
            return Err(MlsError::InvalidMessage);
        }
        self.with_client(&client_handle, |client| {
            if client.groups.contains_key(&expected_group_id) {
                return Err(MlsError::GroupAlreadyExists);
            }
            if client.groups.len() >= MAX_GROUPS {
                return Err(MlsError::ResourceLimit);
            }
            let config = MlsGroupJoinConfig::builder()
                .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
                .build();
            let staged = StagedWelcome::new_from_welcome(&client.provider, &config, welcome, None)
                .map_err(|_| MlsError::InvalidMessage)?;
            if staged.group_context().group_id().as_slice() != expected_group_id {
                return Err(MlsError::InvalidMessage);
            }
            let peer_identity = peer_credential(
                staged.members(),
                staged.own_leaf_index(),
                client.signer.public(),
                &client.identity,
                &expected_peer_public_key,
                None,
            )?;
            let author = staged
                .welcome_sender()
                .map_err(|_| MlsError::InvalidMessage)?;
            if author.signature_key().as_slice() != expected_peer_public_key
                || credential_identity(author.credential())? != peer_identity
            {
                return Err(MlsError::InvalidMessage);
            }
            let group = staged
                .into_group(&client.provider)
                .map_err(|_| MlsError::InvalidState)?;
            let info = group_info(&group);
            client.groups.insert(expected_group_id, group);
            Ok(DirectGroupInfo {
                group: info,
                peer_credential: peer_identity,
            })
        })
    }

    /// Recheck the pinned identities after restoring a persisted conversation.
    pub fn check_direct_group(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        expected_peer_public_key: Vec<u8>,
        expected_peer_credential: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        input(&group_id, 256)?;
        peer_inputs(&expected_peer_public_key, Some(&expected_peer_credential))?;
        self.read(|state| {
            let client = state
                .clients
                .get(&client_handle)
                .ok_or(MlsError::UnknownClient)?;
            let group = client.groups.get(&group_id).ok_or(MlsError::UnknownGroup)?;
            peer_credential(
                group.members(),
                group.own_leaf_index(),
                client.signer.public(),
                &client.identity,
                &expected_peer_public_key,
                Some(&expected_peer_credential),
            )?;
            Ok(group_info(group))
        })
    }

    /// Validate the authenticated member and exact application envelope while
    /// the receiving ratchet is still inside the protected mutation boundary.
    #[allow(clippy::too_many_arguments)] // Explicit pinned inputs on the native FFI boundary.
    pub fn decrypt_direct_message(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        ciphertext: Vec<u8>,
        expected_peer_public_key: Vec<u8>,
        expected_peer_credential: Vec<u8>,
        expected_conversation_id: String,
        expected_client_message_id: String,
        expected_sender_void_id: String,
        expected_recipient_void_id: String,
    ) -> Result<DirectMessage, MlsError> {
        input(&group_id, 256)?;
        peer_inputs(&expected_peer_public_key, Some(&expected_peer_credential))?;
        if !valid_uuid(&expected_conversation_id)
            || !valid_uuid(&expected_client_message_id)
            || !valid_void_id(&expected_sender_void_id)
            || !valid_void_id(&expected_recipient_void_id)
            || expected_sender_void_id == expected_recipient_void_id
        {
            return Err(MlsError::InvalidInput);
        }
        let protocol = parse(&ciphertext)?
            .try_into_protocol_message()
            .map_err(|_| MlsError::InvalidMessage)?;
        if protocol.content_type() != ContentType::Application {
            return Err(MlsError::InvalidMessage);
        }
        let mut pending = self.with_client(&client_handle, |client| {
            let group = client
                .groups
                .get_mut(&group_id)
                .ok_or(MlsError::UnknownGroup)?;
            peer_credential(
                group.members(),
                group.own_leaf_index(),
                client.signer.public(),
                &client.identity,
                &expected_peer_public_key,
                Some(&expected_peer_credential),
            )?;
            let processed = group
                .process_message(&client.provider, protocol)
                .map_err(|_| MlsError::InvalidMessage)?;
            let Sender::Member(index) = processed.sender() else {
                return Err(MlsError::InvalidMessage);
            };
            let sender = group
                .members()
                .find(|member| member.index == *index)
                .ok_or(MlsError::InvalidMessage)?;
            if sender.signature_key != expected_peer_public_key
                || sender.credential != *processed.credential()
                || credential_identity(processed.credential())? != expected_peer_credential
            {
                return Err(MlsError::InvalidMessage);
            }
            let ProcessedMessageContent::ApplicationMessage(content) = processed.into_content()
            else {
                return Err(MlsError::InvalidMessage);
            };
            let plaintext = Zeroizing::new(content.into_bytes());
            if plaintext.len() > MAX_PLAINTEXT {
                return Err(MlsError::InvalidMessage);
            }
            let mut envelope: Envelope =
                serde_json::from_slice(&plaintext).map_err(|_| MlsError::InvalidMessage)?;
            if envelope.v != 1
                || envelope.conversation_id != expected_conversation_id
                || envelope.client_message_id != expected_client_message_id
                || envelope.from_void_id != expected_sender_void_id
                || envelope.to_void_id != expected_recipient_void_id
                || envelope.body.is_empty()
                || envelope.body.contains('\0')
                || envelope.created_at > MAX_JS_INTEGER
            {
                return Err(MlsError::InvalidMessage);
            }
            Ok(PendingDirectMessage {
                body: Zeroizing::new(std::mem::take(&mut envelope.body)),
                created_at: envelope.created_at,
            })
        })?;
        Ok(DirectMessage {
            body: std::mem::take(&mut *pending.body),
            created_at: pending.created_at,
        })
    }

    /// Version one accepts only peer self-updates that preserve both account
    /// bindings. Add/remove/update proposals require a separate lifecycle API.
    pub fn process_direct_commit(
        &self,
        client_handle: String,
        group_id: Vec<u8>,
        message: Vec<u8>,
        expected_peer_public_key: Vec<u8>,
        expected_peer_credential: Vec<u8>,
    ) -> Result<GroupInfo, MlsError> {
        input(&group_id, 256)?;
        peer_inputs(&expected_peer_public_key, Some(&expected_peer_credential))?;
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
            peer_credential(
                group.members(),
                group.own_leaf_index(),
                client.signer.public(),
                &client.identity,
                &expected_peer_public_key,
                Some(&expected_peer_credential),
            )?;
            if group.pending_commit().is_some() {
                return Err(MlsError::InvalidState);
            }
            let processed = group
                .process_message(&client.provider, protocol)
                .map_err(|_| MlsError::InvalidMessage)?;
            let Sender::Member(index) = processed.sender() else {
                return Err(MlsError::InvalidMessage);
            };
            let sender = group
                .members()
                .find(|member| member.index == *index)
                .ok_or(MlsError::InvalidMessage)?;
            if sender.signature_key != expected_peer_public_key
                || sender.credential != *processed.credential()
                || credential_identity(processed.credential())? != expected_peer_credential
            {
                return Err(MlsError::InvalidMessage);
            }
            let ProcessedMessageContent::StagedCommitMessage(commit) = processed.into_content()
            else {
                return Err(MlsError::InvalidMessage);
            };
            if commit.self_removed() || commit.queued_proposals().next().is_some() {
                return Err(MlsError::InvalidMessage);
            }
            let leaf = commit
                .update_path_leaf_node()
                .ok_or(MlsError::InvalidMessage)?;
            if leaf.signature_key().as_slice() != expected_peer_public_key
                || credential_identity(leaf.credential())? != expected_peer_credential
            {
                return Err(MlsError::InvalidMessage);
            }
            group
                .merge_staged_commit(&client.provider, *commit)
                .map_err(|_| MlsError::InvalidState)?;
            Ok(group_info(group))
        })
    }
}

#[cfg(test)]
#[path = "direct_tests.rs"]
mod tests;
