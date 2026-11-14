use super::protocol::{API_BASE, Registration, valid_registration};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"VOID-ACCOUNT\x01";
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
pub(super) enum State {
    Prepared,
    SubmissionPending,
    AlreadyRegistered,
    Registered(Registration),
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
    pub fn for_user() -> io::Result<Self> {
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| io::Error::other("Windows local app data directory is unavailable"))?;
        Ok(Self(directory.join("VOIDCHATS").join("Native")))
    }

    pub fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.0)?;
        let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(self.0.join("account.lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        Ok(lock)
    }

    pub fn load(&self) -> io::Result<Option<Identity>> {
        let file = match File::open(self.0.join("account.dpapi")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(invalid_record());
        }
        let plaintext = platform::dpapi(&bytes, false)?;
        let record = plaintext.strip_prefix(MAGIC).ok_or_else(invalid_record)?;
        let seed: &[u8; 32] = record.get(..32).and_then(|seed| seed.try_into().ok())
            .ok_or_else(invalid_record)?;
        let metadata: Metadata = serde_json::from_slice(&record[32..])
            .map_err(|_| invalid_record())?;
        if metadata.api_base != API_BASE
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
        let encrypted = platform::dpapi(&plaintext, true)?;

        let mut random = [0; 8];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let temp = self.0.join(format!("account-{:016x}.tmp", u64::from_ne_bytes(random)));
        let mut file = OpenOptions::new().create_new(true).write(true).open(&temp)?;
        let write = file.write_all(&encrypted).and_then(|()| file.sync_all());
        drop(file);
        let save = write.and_then(|()| platform::replace(&temp, &self.0.join("account.dpapi")));
        if save.is_err() {
            let _ = fs::remove_file(&temp);
        }
        save
    }
}

fn invalid_record() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid account identity record")
}

#[cfg(windows)]
mod platform {
    use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    };
    use zeroize::{Zeroize, Zeroizing};

    pub fn dpapi(bytes: &[u8], protect: bool) -> io::Result<Zeroizing<Vec<u8>>> {
        let input = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr().cast_mut() };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
        // SAFETY: the input lives through the call. DPAPI returns a LocalAlloc buffer on success.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, ptr::null(), ptr::null(), ptr::null(), ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN, &mut output)
            } else {
                CryptUnprotectData(&input, ptr::null_mut(), ptr::null(), ptr::null(), ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN, &mut output)
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if output.pbData.is_null() {
            return Err(io::Error::other("DPAPI returned no data"));
        }
        // SAFETY: DPAPI owns this non-null buffer until LocalFree. Wipe its contents before freeing it.
        unsafe {
            let data = slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let copy = Zeroizing::new(data.to_vec());
            data.zeroize();
            LocalFree(output.pbData.cast());
            Ok(copy)
        }
    }

    pub fn replace(source: &Path, destination: &Path) -> io::Result<()> {
        let from: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: both Windows path buffers are NUL-terminated and remain live during the call.
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::{io, path::Path};
    use zeroize::Zeroizing;

    pub fn dpapi(_: &[u8], _: bool) -> io::Result<Zeroizing<Vec<u8>>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows DPAPI is required"))
    }

    pub fn replace(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows storage is required"))
    }
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
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            for file in ["account.dpapi", "account.lock"] {
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
