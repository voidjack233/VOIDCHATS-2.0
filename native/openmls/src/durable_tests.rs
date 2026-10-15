//! Host verification of the durable callback and real process replacement.
//! Test keys travel over stdin; this fixture is not Android key management.
use crate::{MlsEngine, MlsError, MlsStorage, StoredMlsState};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{OpenMlsProvider, random::OpenMlsRand};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryStore {
    record: Mutex<Option<StoredMlsState>>,
    fail_save: AtomicBool,
}

impl MlsStorage for MemoryStore {
    fn load_state(&self) -> Result<Option<StoredMlsState>, MlsError> {
        Ok(self
            .record
            .lock()
            .unwrap()
            .as_ref()
            .map(|record| StoredMlsState {
                revision: record.revision,
                snapshot: record.snapshot.clone(),
            }))
    }

    fn state_revision(&self) -> Result<u64, MlsError> {
        self.record
            .lock()
            .unwrap()
            .as_ref()
            .map(|record| record.revision)
            .ok_or(MlsError::MissingState)
    }

    fn save_state(&self, expected_revision: u64, snapshot: Vec<u8>) -> Result<u64, MlsError> {
        let bytes = Zeroizing::new(snapshot);
        let mut record = self.record.lock().unwrap();
        if self.fail_save.load(Ordering::SeqCst)
            || record.as_ref().map_or(0, |record| record.revision) != expected_revision
        {
            return Err(MlsError::StorageFailure);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(MlsError::StorageFailure)?;
        *record = Some(StoredMlsState {
            revision,
            snapshot: bytes.to_vec(),
        });
        Ok(revision)
    }
}

fn pair(engine: &MlsEngine, group: &[u8]) -> (String, String) {
    let alice = engine
        .create_client(b"durable-alice-fixture".to_vec())
        .unwrap()
        .client_handle;
    let bob = engine
        .create_client(b"durable-bob-fixture".to_vec())
        .unwrap()
        .client_handle;
    engine.create_group(alice.clone(), group.to_vec()).unwrap();
    let package = engine.generate_key_package(bob.clone()).unwrap();
    let added = engine
        .add_member(alice.clone(), group.to_vec(), package)
        .unwrap();
    engine
        .merge_pending_commit(alice.clone(), group.to_vec())
        .unwrap();
    engine.process_welcome(bob.clone(), added.welcome).unwrap();
    (alice, bob)
}

#[test]
fn durable_restart_preserves_handles_signers_ratchets_pending_and_consumption() {
    let store = Arc::new(MemoryStore::default());
    let engine = MlsEngine::open(store.clone(), true).unwrap();
    assert!(engine.engine_info().durable_persistence);
    assert!(engine.list_clients().unwrap().is_empty());
    let group = b"durable-state-fixture".to_vec();
    let (alice, bob) = pair(&engine, &group);
    let public = engine.list_clients().unwrap();
    let message = engine
        .encrypt(alice.clone(), group.clone(), b"persisted receive".to_vec())
        .unwrap();
    engine
        .decrypt(bob.clone(), group.clone(), message.clone())
        .unwrap();
    let commit = engine.self_update(alice.clone(), group.clone()).unwrap();
    drop(engine);
    let engine = MlsEngine::open(store.clone(), false).unwrap();
    let restored = engine.list_clients().unwrap();
    assert_eq!(public.len(), restored.len());
    for (before, after) in public.iter().zip(&restored) {
        assert!(before.client_handle == after.client_handle);
        assert!(before.signature_public_key == after.signature_public_key);
        assert!(before.credential_identity == after.credential_identity);
    }
    assert!(engine.decrypt(bob.clone(), group.clone(), message).is_err());
    assert_eq!(
        engine
            .merge_pending_commit(alice.clone(), group.clone())
            .unwrap()
            .epoch,
        2
    );
    assert_eq!(
        engine
            .process_commit(bob.clone(), group.clone(), commit.clone())
            .unwrap()
            .epoch,
        2
    );
    assert!(
        engine
            .process_commit(bob.clone(), group.clone(), commit)
            .is_err()
    );
    let sent = engine
        .encrypt(alice, group.clone(), b"new epoch".to_vec())
        .unwrap();
    assert!(engine.decrypt(bob, group, sent).unwrap().plaintext == b"new epoch");
}

#[test]
fn rejected_ciphertext_generation_is_checkpointed_before_error_and_stays_consumed() {
    let store = Arc::new(MemoryStore::default());
    let engine = MlsEngine::open(store.clone(), true).unwrap();
    let group = b"failed-decrypt-fixture".to_vec();
    let (alice, bob) = pair(&engine, &group);
    let message = engine
        .encrypt(
            alice.clone(),
            group.clone(),
            b"authenticated fixture".to_vec(),
        )
        .unwrap();
    let mut tampered = message.clone();
    *tampered.last_mut().unwrap() ^= 1;
    let before = store.state_revision().unwrap();
    assert!(
        engine
            .decrypt(bob.clone(), group.clone(), tampered)
            .is_err()
    );
    assert_eq!(store.state_revision().unwrap(), before + 1);
    drop(engine);
    let engine = MlsEngine::open(store, false).unwrap();
    // The rejected generation was consumed by OpenMLS. A provider snapshot that
    // failed to flush the cached secret tree would accept this original message.
    assert!(engine.decrypt(bob.clone(), group.clone(), message).is_err());
    let next = engine
        .encrypt(alice, group.clone(), b"following generation".to_vec())
        .unwrap();
    assert!(engine.decrypt(bob, group, next).unwrap().plaintext == b"following generation");
}

#[test]
fn storage_failure_withholds_outputs_and_poisoned_engine_cannot_continue() {
    let store = Arc::new(MemoryStore::default());
    let engine = MlsEngine::open(store.clone(), true).unwrap();
    let group = b"write-failure-fixture".to_vec();
    let (alice, bob) = pair(&engine, &group);
    let revision = store.state_revision().unwrap();
    store.fail_save.store(true, Ordering::SeqCst);
    assert!(matches!(
        engine.encrypt(alice.clone(), group.clone(), b"never emitted".to_vec()),
        Err(MlsError::StorageFailure)
    ));
    assert_eq!(store.state_revision().unwrap(), revision);
    assert!(matches!(
        engine.group_info(alice.clone(), group.clone()),
        Err(MlsError::InvalidState)
    ));
    assert!(matches!(engine.list_clients(), Err(MlsError::InvalidState)));
    assert!(matches!(
        engine.create_client(b"cannot continue".to_vec()),
        Err(MlsError::InvalidState)
    ));
    store.fail_save.store(false, Ordering::SeqCst);
    let fresh = MlsEngine::open(store, false).unwrap();
    let next = fresh
        .encrypt(alice, group.clone(), b"committed output".to_vec())
        .unwrap();
    assert!(fresh.decrypt(bob, group, next).unwrap().plaintext == b"committed output");
}

struct RacingStore {
    inner: MemoryStore,
    race: AtomicBool,
    barrier: Barrier,
}

impl MlsStorage for RacingStore {
    fn load_state(&self) -> Result<Option<StoredMlsState>, MlsError> {
        self.inner.load_state()
    }
    fn state_revision(&self) -> Result<u64, MlsError> {
        self.inner.state_revision()
    }
    fn save_state(&self, expected_revision: u64, snapshot: Vec<u8>) -> Result<u64, MlsError> {
        if self.race.load(Ordering::SeqCst) {
            self.barrier.wait();
        }
        self.inner.save_state(expected_revision, snapshot)
    }
}

#[test]
fn concurrent_engines_compare_and_swap_prevents_stale_outputs_or_overwrites() {
    let store = Arc::new(RacingStore {
        inner: MemoryStore::default(),
        race: AtomicBool::new(false),
        barrier: Barrier::new(2),
    });
    let first = MlsEngine::open(store.clone(), true).unwrap();
    let second = MlsEngine::open(store.clone(), false).unwrap();
    store.race.store(true, Ordering::SeqCst);
    let first_worker =
        std::thread::spawn(move || first.create_client(b"concurrent fixture one".to_vec()));
    let second_worker =
        std::thread::spawn(move || second.create_client(b"concurrent fixture two".to_vec()));
    let results = [first_worker.join().unwrap(), second_worker.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(MlsError::StorageFailure)))
            .count(),
        1
    );
    store.race.store(false, Ordering::SeqCst);
    let fresh = MlsEngine::open(store, false).unwrap();
    let clients = fresh.list_clients().unwrap();
    assert_eq!(clients.len(), 1);
    let successful = results.into_iter().find_map(Result::ok).unwrap();
    assert!(clients[0].client_handle == successful.client_handle);
    assert!(clients[0].signature_public_key == successful.signature_public_key);
}

#[test]
fn missing_corrupt_incompatible_state_does_not_regenerate_identity() {
    let store = Arc::new(MemoryStore::default());
    assert!(matches!(
        MlsEngine::open(store.clone(), false),
        Err(MlsError::MissingState)
    ));
    let engine = MlsEngine::open(store.clone(), true).unwrap();
    engine.create_client(b"existing fixture".to_vec()).unwrap();
    drop(engine);
    let original = store.load_state().unwrap().unwrap();
    {
        let mut record = store.record.lock().unwrap();
        record.as_mut().unwrap().snapshot = b"corrupt serialized state".to_vec();
    }
    assert!(matches!(
        MlsEngine::open(store.clone(), false),
        Err(MlsError::InvalidSnapshot)
    ));
    assert!(matches!(
        MlsEngine::open(store.clone(), true),
        Err(MlsError::InvalidSnapshot)
    ));
    let changed = String::from_utf8(original.snapshot).unwrap().replace(
        "\"openmls_version\":\"0.9.1\"",
        "\"openmls_version\":\"future\"",
    );
    store.record.lock().unwrap().as_mut().unwrap().snapshot = changed.into_bytes();
    assert!(matches!(
        MlsEngine::open(store, true),
        Err(MlsError::InvalidSnapshot)
    ));
}

#[test]
fn single_use_suspended_state_and_discard_survive_restart() {
    let store = Arc::new(MemoryStore::default());
    let engine = MlsEngine::open(store.clone(), true).unwrap();
    let client = engine.create_client(b"suspend fixture".to_vec()).unwrap();
    let snapshot = engine.suspend_client(client.client_handle).unwrap();
    drop(engine);
    let engine = MlsEngine::open(store.clone(), false).unwrap();
    assert!(engine.list_clients().unwrap().is_empty());
    let restored = engine.restore_client(snapshot.clone()).unwrap();
    assert!(restored.signature_public_key == client.signature_public_key);
    assert!(matches!(
        engine.restore_client(snapshot),
        Err(MlsError::InvalidSnapshot)
    ));
    let discarded = engine.suspend_client(restored.client_handle).unwrap();
    engine.discard_snapshot(discarded.clone()).unwrap();
    drop(engine);
    let engine = MlsEngine::open(store, false).unwrap();
    assert!(matches!(
        engine.restore_client(discarded),
        Err(MlsError::InvalidSnapshot)
    ));
}

/// Standard AES-GCM authenticated encryption protects the restart fixture.
/// There is no plaintext snapshot/key file and no production filesystem store.
struct ProtectedFixture {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    lock: Mutex<()>,
}

impl ProtectedFixture {
    fn read_record(&self) -> Result<Option<StoredMlsState>, MlsError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(MlsError::StorageFailure),
        };
        if bytes.len() < 44
            || bytes.len() > crate::persistence::MAX_ENGINE_SNAPSHOT + 44
            || &bytes[..8] != b"VOIDMLS1"
        {
            return Err(MlsError::StorageFailure);
        }
        let revision = u64::from_be_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| MlsError::StorageFailure)?,
        );
        let nonce = Nonce::from(
            <[u8; 12]>::try_from(&bytes[16..28]).map_err(|_| MlsError::StorageFailure)?,
        );
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_slice()).map_err(|_| MlsError::StorageFailure)?;
        let snapshot = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &bytes[28..],
                    aad: &bytes[..16],
                },
            )
            .map_err(|_| MlsError::StorageFailure)?;
        Ok(Some(StoredMlsState { revision, snapshot }))
    }
}

impl MlsStorage for ProtectedFixture {
    fn load_state(&self) -> Result<Option<StoredMlsState>, MlsError> {
        let _guard = self.lock.lock().map_err(|_| MlsError::StorageFailure)?;
        self.read_record()
    }

    fn state_revision(&self) -> Result<u64, MlsError> {
        let _guard = self.lock.lock().map_err(|_| MlsError::StorageFailure)?;
        let loaded = self.read_record()?.ok_or(MlsError::MissingState)?;
        let revision = loaded.revision;
        let _wipe = Zeroizing::new(loaded.snapshot);
        Ok(revision)
    }

    fn save_state(&self, expected_revision: u64, snapshot: Vec<u8>) -> Result<u64, MlsError> {
        let snapshot = Zeroizing::new(snapshot);
        let _guard = self.lock.lock().map_err(|_| MlsError::StorageFailure)?;
        let previous = self.read_record()?;
        let old_revision = previous.as_ref().map_or(0, |record| record.revision);
        if let Some(previous) = previous {
            let _wipe = Zeroizing::new(previous.snapshot);
        }
        if old_revision != expected_revision {
            return Err(MlsError::StorageFailure);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(MlsError::StorageFailure)?;
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_slice()).map_err(|_| MlsError::StorageFailure)?;
        let random_nonce: [u8; 12] = OpenMlsRustCrypto::default()
            .rand()
            .random_vec(12)
            .map_err(|_| MlsError::CryptoFailure)?
            .try_into()
            .map_err(|_| MlsError::CryptoFailure)?;
        let nonce = Nonce::from(random_nonce);
        let mut header = b"VOIDMLS1".to_vec();
        header.extend_from_slice(&revision.to_be_bytes());
        let encrypted = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &snapshot,
                    aad: &header,
                },
            )
            .map_err(|_| MlsError::StorageFailure)?;
        let mut bytes = header;
        bytes.extend_from_slice(&random_nonce);
        bytes.extend_from_slice(&encrypted);
        let directory = self.path.parent().ok_or(MlsError::StorageFailure)?;
        let mut pending =
            tempfile::NamedTempFile::new_in(directory).map_err(|_| MlsError::StorageFailure)?;
        pending
            .write_all(&bytes)
            .map_err(|_| MlsError::StorageFailure)?;
        pending
            .as_file()
            .sync_all()
            .map_err(|_| MlsError::StorageFailure)?;
        if std::env::var("VOID_MLS_TEST_PHASE").as_deref() == Ok("interrupt-before-checkpoint") {
            // Abrupt process termination leaves only an encrypted incomplete
            // write; the committed snapshot remains intact and no output exits.
            std::process::exit(88);
        }
        pending
            .persist(&self.path)
            .map_err(|_| MlsError::StorageFailure)?;
        if std::env::var("VOID_MLS_TEST_PHASE").as_deref() == Ok("interrupt-after-checkpoint") {
            // Simulate termination after durable replacement but before the
            // operation can return a ciphertext to its caller.
            std::process::exit(89);
        }
        Ok(revision)
    }
}

#[derive(Serialize, Deserialize)]
struct PublicFixture {
    alice: String,
    bob: String,
    alice_public: Vec<u8>,
    bob_public: Vec<u8>,
    welcome: Vec<u8>,
    update: Vec<u8>,
    replay: Vec<u8>,
    rejected_generation: Vec<u8>,
}

fn write_public(directory: &Path, fixture: &PublicFixture) {
    fs::write(
        directory.join("public-fixture.json"),
        serde_json::to_vec(fixture).unwrap(),
    )
    .unwrap();
}

fn read_public(directory: &Path) -> PublicFixture {
    serde_json::from_slice(&fs::read(directory.join("public-fixture.json")).unwrap()).unwrap()
}

fn verify_public(engine: &MlsEngine, fixture: &PublicFixture) {
    let clients = engine.list_clients().unwrap();
    assert_eq!(clients.len(), 2);
    assert!(
        clients
            .iter()
            .any(|client| client.client_handle == fixture.alice
                && client.signature_public_key == fixture.alice_public)
    );
    assert!(
        clients
            .iter()
            .any(|client| client.client_handle == fixture.bob
                && client.signature_public_key == fixture.bob_public)
    );
}

/// Selected only by the parent test in a genuinely separate OS process.
#[test]
#[ignore = "restart worker is launched by protected_state_survives_actual_process_restarts"]
fn process_fixture_worker() {
    let directory = PathBuf::from(
        std::env::var_os("VOID_MLS_TEST_DIRECTORY").expect("worker fixture path missing"),
    );
    let phase = std::env::var("VOID_MLS_TEST_PHASE").expect("worker phase missing");
    let mut key = Zeroizing::new([0u8; 32]);
    std::io::stdin()
        .read_exact(key.as_mut_slice())
        .expect("worker key transport failed");
    let store = Arc::new(ProtectedFixture {
        path: directory.join("protected-state.bin"),
        key,
        lock: Mutex::new(()),
    });
    let group = b"process-pending-fixture".to_vec();
    let ratchet_group = b"process-ratchet-fixture".to_vec();
    match phase.as_str() {
        "initialize" => {
            let engine = MlsEngine::open(store, true).unwrap();
            let alice = engine
                .create_client(b"process-alice-fixture".to_vec())
                .unwrap();
            let bob = engine
                .create_client(b"process-bob-fixture".to_vec())
                .unwrap();
            engine
                .create_group(alice.client_handle.clone(), group.clone())
                .unwrap();
            let package = engine
                .generate_key_package(bob.client_handle.clone())
                .unwrap();
            let pending = engine
                .add_member(alice.client_handle.clone(), group, package)
                .unwrap();
            engine
                .create_group(alice.client_handle.clone(), ratchet_group.clone())
                .unwrap();
            let package = engine
                .generate_key_package(bob.client_handle.clone())
                .unwrap();
            let added = engine
                .add_member(alice.client_handle.clone(), ratchet_group.clone(), package)
                .unwrap();
            engine
                .merge_pending_commit(alice.client_handle.clone(), ratchet_group.clone())
                .unwrap();
            engine
                .process_welcome(bob.client_handle.clone(), added.welcome)
                .unwrap();
            let before = engine
                .encrypt(
                    alice.client_handle.clone(),
                    ratchet_group.clone(),
                    b"before restart".to_vec(),
                )
                .unwrap();
            assert!(
                engine
                    .decrypt(bob.client_handle.clone(), ratchet_group, before)
                    .unwrap()
                    .plaintext
                    == b"before restart"
            );
            write_public(
                &directory,
                &PublicFixture {
                    alice: alice.client_handle,
                    bob: bob.client_handle,
                    alice_public: alice.signature_public_key,
                    bob_public: bob.signature_public_key,
                    welcome: pending.welcome,
                    update: Vec::new(),
                    replay: Vec::new(),
                    rejected_generation: Vec::new(),
                },
            );
        }
        "first-restart" => {
            let engine = MlsEngine::open(store, false).unwrap();
            let mut fixture = read_public(&directory);
            verify_public(&engine, &fixture);
            assert!(matches!(
                engine.encrypt(fixture.alice.clone(), group.clone(), vec![]),
                Err(MlsError::InvalidState)
            ));
            assert_eq!(
                engine
                    .merge_pending_commit(fixture.alice.clone(), group.clone())
                    .unwrap()
                    .epoch,
                1
            );
            assert_eq!(
                engine
                    .process_welcome(fixture.bob.clone(), fixture.welcome.clone())
                    .unwrap()
                    .epoch,
                1
            );
            fixture.replay = engine
                .encrypt(
                    fixture.alice.clone(),
                    ratchet_group.clone(),
                    b"after first restart".to_vec(),
                )
                .unwrap();
            assert!(
                engine
                    .decrypt(
                        fixture.bob.clone(),
                        ratchet_group.clone(),
                        fixture.replay.clone()
                    )
                    .unwrap()
                    .plaintext
                    == b"after first restart"
            );
            fixture.rejected_generation = engine
                .encrypt(
                    fixture.alice.clone(),
                    ratchet_group.clone(),
                    b"generation consumed on error".to_vec(),
                )
                .unwrap();
            let mut tampered = fixture.rejected_generation.clone();
            *tampered.last_mut().unwrap() ^= 1;
            assert!(
                engine
                    .decrypt(fixture.bob.clone(), ratchet_group, tampered)
                    .is_err()
            );
            fixture.update = engine.self_update(fixture.alice.clone(), group).unwrap();
            write_public(&directory, &fixture);
        }
        "second-restart" => {
            let engine = MlsEngine::open(store, false).unwrap();
            let fixture = read_public(&directory);
            verify_public(&engine, &fixture);
            assert!(
                engine
                    .decrypt(fixture.bob.clone(), ratchet_group.clone(), fixture.replay)
                    .is_err()
            );
            assert!(
                engine
                    .decrypt(
                        fixture.bob.clone(),
                        ratchet_group.clone(),
                        fixture.rejected_generation
                    )
                    .is_err()
            );
            assert!(
                engine
                    .process_welcome(fixture.bob.clone(), fixture.welcome)
                    .is_err()
            );
            assert_eq!(
                engine
                    .merge_pending_commit(fixture.alice.clone(), group.clone())
                    .unwrap()
                    .epoch,
                2
            );
            assert_eq!(
                engine
                    .process_commit(fixture.bob.clone(), group, fixture.update)
                    .unwrap()
                    .epoch,
                2
            );
            let after = engine
                .encrypt(
                    fixture.alice,
                    ratchet_group.clone(),
                    b"after second restart".to_vec(),
                )
                .unwrap();
            assert!(
                engine
                    .decrypt(fixture.bob, ratchet_group, after)
                    .unwrap()
                    .plaintext
                    == b"after second restart"
            );
        }
        "wrong-key" | "corrupt" => {
            assert!(matches!(
                MlsEngine::open(store.clone(), false),
                Err(MlsError::StorageFailure)
            ));
            assert!(matches!(
                MlsEngine::open(store, true),
                Err(MlsError::StorageFailure)
            ));
        }
        "interrupt-before-checkpoint" | "interrupt-after-checkpoint" => {
            let engine = MlsEngine::open(store, false).unwrap();
            let fixture = read_public(&directory);
            verify_public(&engine, &fixture);
            let _unemitted = engine.encrypt(
                fixture.alice,
                ratchet_group,
                b"never leaves native worker".to_vec(),
            );
            panic!("interrupted checkpoint unexpectedly returned");
        }
        "after-interruption" => {
            let engine = MlsEngine::open(store, false).unwrap();
            let fixture = read_public(&directory);
            verify_public(&engine, &fixture);
            let sent = engine
                .encrypt(
                    fixture.alice,
                    ratchet_group.clone(),
                    b"after interrupted write".to_vec(),
                )
                .unwrap();
            assert!(
                engine
                    .decrypt(fixture.bob, ratchet_group, sent)
                    .unwrap()
                    .plaintext
                    == b"after interrupted write"
            );
        }
        "missing" => assert!(matches!(
            MlsEngine::open(store, false),
            Err(MlsError::MissingState)
        )),
        _ => panic!("unknown restart fixture phase"),
    }
}

fn run_process(directory: &Path, phase: &str, key: &[u8; 32]) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "engine::durable_tests::process_fixture_worker",
            "--ignored",
        ])
        .env("VOID_MLS_TEST_DIRECTORY", directory)
        .env("VOID_MLS_TEST_PHASE", phase)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart worker could not start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(key)
        .expect("restart key transport failed");
    let status = child.wait().unwrap();
    match phase {
        "interrupt-before-checkpoint" => assert_eq!(status.code(), Some(88)),
        "interrupt-after-checkpoint" => assert_eq!(status.code(), Some(89)),
        _ => assert!(
            status.success(),
            "native restart worker failed in phase {phase}"
        ),
    }
}

#[test]
fn protected_state_survives_actual_process_restarts_and_rejects_wrong_keys_corruption_missing_state()
 {
    let directory = tempfile::tempdir().unwrap();
    let key: [u8; 32] = OpenMlsRustCrypto::default()
        .rand()
        .random_vec(32)
        .unwrap()
        .try_into()
        .unwrap();
    let key = Zeroizing::new(key);
    run_process(directory.path(), "initialize", &key);
    let protected_path = directory.path().join("protected-state.bin");
    let disk = fs::read(&protected_path).unwrap();
    assert!(
        !disk
            .windows(b"process-alice-fixture".len())
            .any(|bytes| bytes == b"process-alice-fixture")
    );
    assert!(
        !disk
            .windows(b"signature_public_key".len())
            .any(|bytes| bytes == b"signature_public_key")
    );
    run_process(directory.path(), "first-restart", &key);
    run_process(directory.path(), "second-restart", &key);
    let committed = fs::read(&protected_path).unwrap();
    run_process(directory.path(), "interrupt-before-checkpoint", &key);
    assert!(fs::read(&protected_path).unwrap() == committed);
    run_process(directory.path(), "after-interruption", &key);
    let committed = fs::read(&protected_path).unwrap();
    let revision = u64::from_be_bytes(committed[8..16].try_into().unwrap());
    run_process(directory.path(), "interrupt-after-checkpoint", &key);
    let after = fs::read(&protected_path).unwrap();
    assert_eq!(
        u64::from_be_bytes(after[8..16].try_into().unwrap()),
        revision + 1
    );
    run_process(directory.path(), "after-interruption", &key);
    let mut wrong_key = Zeroizing::new(*key);
    wrong_key[0] ^= 1;
    run_process(directory.path(), "wrong-key", &wrong_key);
    let mut corrupted = fs::read(&protected_path).unwrap();
    *corrupted.last_mut().unwrap() ^= 1;
    fs::write(&protected_path, corrupted).unwrap();
    run_process(directory.path(), "corrupt", &key);
    fs::remove_file(&protected_path).unwrap();
    run_process(directory.path(), "missing", &key);
}
