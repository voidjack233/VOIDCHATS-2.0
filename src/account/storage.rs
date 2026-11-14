use super::protocol::{API_BASE, Registration, valid_registration, valid_token};
use crate::local_data::{dpapi, move_file, user_data_dir};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"VOID-ACCOUNT\x01";
const CONFIRMATION_MAGIC: &[u8] = b"VOID-REGISTRATION\x01";
const CONFIRMATION_FILE: &str = "account-confirmation.dpapi";
const MAX_RECORD: u64 = 16_384;

pub(super) struct Identity {
    pub key: SigningKey,
    pub state: State,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    api_base: String,
    state: State,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    api_base: String,
    public_key: [u8; 32],
    challenge_id: String,
    account: Registration,
}

#[derive(Deserialize, Serialize)]
pub(super) enum State {
    Prepared,
    SubmissionPending,
    SubmissionPendingFor { challenge_id: String },
    AlreadyRegistered,
    Registered(Registration),
}

impl State {
    pub fn submission_pending(&self) -> bool {
        matches!(self, Self::SubmissionPending | Self::SubmissionPendingFor { .. })
    }

    pub fn challenge_id(&self) -> Option<&str> {
        match self {
            Self::SubmissionPendingFor { challenge_id } => Some(challenge_id),
            _ => None,
        }
    }
}

impl Identity {
    pub fn generate() -> io::Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(io::Error::other)?;
        Ok(Self { key: SigningKey::from_bytes(&seed), state: State::Prepared })
    }
}

pub(super) struct Store(PathBuf);

impl Store {
    #[cfg(all(test, windows))]
    pub fn from_test_path(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn for_user() -> io::Result<Self> {
        user_data_dir().map(Self)
    }

    pub fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.0)?;
        let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(self.0.join("account.lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        Ok(lock)
    }

    pub fn load(&self) -> io::Result<Option<Identity>> {
        let Some(plaintext) = read_protected(&self.0.join("account.dpapi"))? else {
            return Ok(None);
        };
        let record = plaintext.strip_prefix(MAGIC).ok_or_else(invalid_record)?;
        let seed: &[u8; 32] = record.get(..32).and_then(|seed| seed.try_into().ok())
            .ok_or_else(invalid_record)?;
        let metadata: Metadata = serde_json::from_slice(&record[32..])
            .map_err(|_| invalid_record())?;
        if metadata.api_base != API_BASE
            || metadata.state.challenge_id().is_some_and(|id| !valid_token(id))
            || matches!(&metadata.state, State::Registered(account) if !valid_registration(account))
        {
            return Err(invalid_record());
        }
        Ok(Some(Identity { key: SigningKey::from_bytes(seed), state: metadata.state }))
    }

    // Callers hold the account lock when creating or replacing a record.
    pub fn save(&self, identity: &Identity) -> io::Result<()> {
        #[derive(Serialize)]
        struct Record<'a> { api_base: &'static str, state: &'a State }
        // Keep serialization allocations limited to public metadata.
        let metadata = serde_json::to_vec(&Record { api_base: API_BASE, state: &identity.state })?;
        let seed = identity.key.as_bytes();
        let mut plaintext = Zeroizing::new(Vec::with_capacity(MAGIC.len() + seed.len() + metadata.len()));
        plaintext.extend_from_slice(MAGIC);
        plaintext.extend_from_slice(seed);
        plaintext.extend_from_slice(&metadata);
        self.write_protected("account.dpapi", &plaintext)
    }

    pub fn has_confirmation(&self) -> io::Result<bool> {
        self.0.join(CONFIRMATION_FILE).try_exists()
    }

    // Keep the server confirmation separate so it can survive a failed identity-file replacement.
    // The caller holds the account lock through both writes.
    pub fn save_confirmation(&self, identity: &Identity, account: &Registration) -> io::Result<()> {
        let challenge_id = identity.state.challenge_id().ok_or_else(invalid_record)?;
        if !valid_token(challenge_id) || !valid_registration(account) {
            return Err(invalid_record());
        }
        let receipt = Confirmation {
            api_base: API_BASE.into(),
            public_key: identity.key.verifying_key().to_bytes(),
            challenge_id: challenge_id.into(),
            account: account.clone(),
        };
        let mut plaintext = CONFIRMATION_MAGIC.to_vec();
        serde_json::to_writer(&mut plaintext, &receipt)?;
        self.write_protected(CONFIRMATION_FILE, &plaintext)
    }

    pub fn load_confirmation(&self, identity: &Identity) -> io::Result<Option<Registration>> {
        let Some(plaintext) = read_protected(&self.0.join(CONFIRMATION_FILE))? else {
            return Ok(None);
        };
        let record = plaintext.strip_prefix(CONFIRMATION_MAGIC).ok_or_else(invalid_record)?;
        let receipt: Confirmation = serde_json::from_slice(record).map_err(|_| invalid_record())?;
        if receipt.api_base != API_BASE
            || Some(receipt.challenge_id.as_str()) != identity.state.challenge_id()
            || receipt.public_key != identity.key.verifying_key().to_bytes()
            || !valid_registration(&receipt.account)
        {
            return Err(invalid_record());
        }
        Ok(Some(receipt.account))
    }

    pub fn clear_confirmation(&self) {
        // A committed identity is authoritative even if cleanup cannot remove the receipt.
        let _ = fs::remove_file(self.0.join(CONFIRMATION_FILE));
    }

    fn write_protected(&self, name: &str, plaintext: &[u8]) -> io::Result<()> {
        let encrypted = dpapi(plaintext, true)?;
        let mut random = [0; 8];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let temp = self.0.join(format!("account-{:016x}.tmp", u64::from_ne_bytes(random)));
        let mut file = OpenOptions::new().create_new(true).write(true).open(&temp)?;
        let write = file.write_all(&encrypted).and_then(|()| file.sync_all());
        drop(file);
        let save = write.and_then(|()| move_file(&temp, &self.0.join(name), true));
        if save.is_err() {
            let _ = fs::remove_file(&temp);
        }
        save
    }
}

fn read_protected(path: &Path) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid_record());
    }
    dpapi(&bytes, false).map(Some)
}

fn invalid_record() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid account identity record")
}

#[cfg(all(test, windows))]
pub(super) mod tests {
    use super::*;
    use std::process::Command;

    pub(in crate::account) struct TestDir(PathBuf);

    impl TestDir {
        pub fn new() -> Self {
            let mut random = [0; 16];
            getrandom::fill(&mut random).unwrap();
            let path = std::env::temp_dir().join(format!("void-native-test-{:032x}", u128::from_ne_bytes(random)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        pub(in crate::account) fn store(&self) -> Store {
            Store(self.0.clone())
        }

        pub(in crate::account) fn record_path(&self) -> PathBuf {
            self.0.join("account.dpapi")
        }

        pub(in crate::account) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            for file in ["account.dpapi", "account.lock", CONFIRMATION_FILE] {
                let _ = fs::remove_file(self.0.join(file));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn isolated_identity_survives_a_process_restart() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        const DIR: &str = "VOID_NATIVE_TEST_RESTART_DIR";
        const KEY: &str = "VOID_NATIVE_TEST_RESTART_PUBLIC";
        if let Some(directory) = std::env::var_os(DIR) {
            let identity = Store(directory.into()).load().unwrap().unwrap();
            assert_eq!(STANDARD.encode(identity.key.verifying_key().as_bytes()), std::env::var(KEY).unwrap());
            assert!(matches!(identity.state, State::Registered(account) if account.void_id == "300"));
            return;
        }

        let dir = TestDir::new();
        let store = dir.store();
        let lock = store.lock().unwrap();
        assert!(store.lock().is_err());
        let mut identity = Identity::generate().unwrap();
        store.save(&identity).unwrap();
        identity.state = State::Registered(Registration { void_id: "300".into(), created_at: "2026-11-14T00:00:00Z".into() });
        store.save(&identity).unwrap();
        drop(lock);
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "account::storage::tests::isolated_identity_survives_a_process_restart"])
            .env(DIR, &dir.0)
            .env(KEY, STANDARD.encode(identity.key.verifying_key().as_bytes()))
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
