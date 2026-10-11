//! Native-only MLS engine. Secret storage is never part of the FFI interface.
mod engine;
mod persistence;
mod self_test;

pub use engine::MlsEngine;

pub const OPENMLS_VERSION: &str = "0.9.1";
pub const CIPHERSUITE_NAME: &str = "MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519";

#[derive(Debug, thiserror::Error, uniffi::Error, PartialEq, Eq)]
pub enum MlsError {
    #[error("Invalid input")]
    InvalidInput,
    #[error("Unknown client")]
    UnknownClient,
    #[error("Unknown group")]
    UnknownGroup,
    #[error("Group already exists")]
    GroupAlreadyExists,
    #[error("Message validation failed")]
    InvalidMessage,
    #[error("Invalid MLS state")]
    InvalidState,
    #[error("Native storage failed")]
    StorageFailure,
    #[error("Cryptographic operation failed")]
    CryptoFailure,
    #[error("Invalid or consumed snapshot")]
    InvalidSnapshot,
    #[error("Native resource limit reached")]
    ResourceLimit,
}

#[derive(uniffi::Record)]
pub struct EngineInfo {
    pub openmls_version: String,
    pub ciphersuite: String,
    pub protocol_version: String,
    pub durable_persistence: bool,
}

#[derive(uniffi::Record)]
pub struct ClientInfo {
    pub client_handle: String,
    pub credential_identity: Vec<u8>,
    pub signature_public_key: Vec<u8>,
}

#[derive(uniffi::Record)]
pub struct GroupInfo {
    pub group_id: Vec<u8>,
    pub epoch: u64,
    pub member_count: u32,
}

#[derive(uniffi::Record)]
pub struct AddMemberResult {
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
}

#[derive(uniffi::Record)]
pub struct DecryptedMessage {
    pub plaintext: Vec<u8>,
    pub sender_identity: Vec<u8>,
    pub sender_signature_public_key: Vec<u8>,
    pub epoch: u64,
}

#[derive(uniffi::Record)]
pub struct SelfTestReport {
    pub openmls_version: String,
    pub alice_to_bob: bool,
    pub bob_to_alice: bool,
    pub tamper_rejected: bool,
    pub commit_processed: bool,
    pub snapshot_restored: bool,
}

uniffi::setup_scaffolding!();
