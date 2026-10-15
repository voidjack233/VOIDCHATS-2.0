//! The platform owns encryption, key protection, and transaction commit. These
//! callbacks are implemented only in native code, never a React Native module.
use crate::MlsError;

/// Plaintext secret bytes travel only between Rust and the trusted native
/// storage implementation. No bridge method exposes this record to JavaScript.
#[derive(uniffi::Record)]
pub struct StoredMlsState {
    pub revision: u64,
    pub snapshot: Vec<u8>,
}

/// Storage must authenticate/encrypt the complete snapshot, atomically compare
/// and increment its revision, and participate in the native operation's
/// transaction. The platform commits before emitting any operation result.
///
/// A successful save does not authorize rolling back an operation error:
/// OpenMLS can advance provider/ratchet state even when rejecting a message.
/// Commit that checkpoint while rejecting the request. Storage, journal, or
/// final-commit failure must poison/close the engine and emit no result.
/// Missing state after initialization is an error, never an empty-state reset.
#[uniffi::export(rust, foreign)]
pub trait MlsStorage: Send + Sync {
    fn load_state(&self) -> Result<Option<StoredMlsState>, MlsError>;
    fn state_revision(&self) -> Result<u64, MlsError>;
    fn save_state(&self, expected_revision: u64, snapshot: Vec<u8>) -> Result<u64, MlsError>;
}
