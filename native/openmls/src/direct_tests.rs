use super::*;
use crate::{ClientInfo, MlsStorage, StoredMlsState};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const CONVERSATION: &str = "50000000-0000-4000-8000-000000000001";
const CLIENT_MESSAGE: &str = "60000000-0000-4000-8000-000000000001";
const ALICE_ID: &str = "110559210000000000";
const BOB_ID: &str = "110559210000000001";

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
        let mut current = self.record.lock().unwrap();
        if self.fail_save.load(Ordering::SeqCst)
            || current.as_ref().map_or(0, |record| record.revision) != expected_revision
        {
            return Err(MlsError::StorageFailure);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(MlsError::StorageFailure)?;
        *current = Some(StoredMlsState {
            revision,
            snapshot: bytes.to_vec(),
        });
        Ok(revision)
    }
}

struct Pair {
    engine: Arc<MlsEngine>,
    alice: ClientInfo,
    bob: ClientInfo,
    group: Vec<u8>,
}

fn pair(engine: Arc<MlsEngine>) -> Pair {
    let alice = engine
        .create_client(b"direct-alice-device".to_vec())
        .unwrap();
    let bob = engine.create_client(b"direct-bob-device".to_vec()).unwrap();
    let group = b"direct-checked-conversation".to_vec();
    let package = engine
        .generate_key_package(bob.client_handle.clone())
        .unwrap();
    assert_eq!(
        engine
            .validate_key_package(package.clone(), bob.signature_public_key.clone())
            .unwrap(),
        bob.credential_identity
    );
    engine
        .create_group(alice.client_handle.clone(), group.clone())
        .unwrap();
    let added = engine
        .add_member(alice.client_handle.clone(), group.clone(), package)
        .unwrap();
    engine
        .merge_pending_commit(alice.client_handle.clone(), group.clone())
        .unwrap();
    let joined = engine
        .process_welcome_checked(
            bob.client_handle.clone(),
            added.welcome,
            group.clone(),
            alice.signature_public_key.clone(),
        )
        .unwrap();
    assert_eq!(joined.group.member_count, 2);
    assert_eq!(joined.group.group_id, group);
    assert_eq!(joined.peer_credential, alice.credential_identity);
    Pair {
        engine,
        alice,
        bob,
        group,
    }
}

fn envelope(body: &str) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "client_message_id": CLIENT_MESSAGE, "conversation_id": CONVERSATION,
        "from_void_id": ALICE_ID, "to_void_id": BOB_ID, "body": body,
        "created_at": 1_794_000_000_123u64,
    })
}

fn receive(pair: &Pair, ciphertext: Vec<u8>) -> Result<DirectMessage, MlsError> {
    pair.engine.decrypt_direct_message(
        pair.bob.client_handle.clone(),
        pair.group.clone(),
        ciphertext,
        pair.alice.signature_public_key.clone(),
        pair.alice.credential_identity.clone(),
        CONVERSATION.into(),
        CLIENT_MESSAGE.into(),
        ALICE_ID.into(),
        BOB_ID.into(),
    )
}

fn hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn synthetic_v2_authentication_vector_uses_raw_ed25519_and_is_domain_restricted() {
    let engine = MlsEngine::new();
    let client = engine
        .create_client(b"public-auth-vector-fixture".to_vec())
        .unwrap();
    let public_key = hex("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c");
    {
        let mut state = engine.state.lock().unwrap();
        let client = state.clients.get_mut(&client.client_handle).unwrap();
        let signer = SignatureKeyPair::from_raw(
            SUITE.signature_algorithm(),
            vec![1; 32],
            public_key.clone(),
        );
        signer.store(client.provider.storage()).unwrap();
        client.signer = signer;
    }
    // Synthetic interoperability fixture using the deployed authenticate:v2
    // framing and public seed 0x01. Its expired timestamp has no live authority.
    let message = concat!(
        "VOID-NOTHINGNESS:authenticate:v2\n",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\n",
        "110559210000000000\n",
        "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=\n",
        "2026-11-02T00:00:10.123456789Z\n",
        "aHR0cHM6Ly9hcGkudm9pZGNoYXRzLnh5ei9kbTlwWkc1dmRHaHBibWR1WlhOeg==\n",
        "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8\n"
    )
    .as_bytes()
    .to_vec();
    let signature = engine
        .sign_auth_challenge(client.client_handle.clone(), message.clone())
        .unwrap();
    assert_eq!(
        signature,
        hex(
            "d7f7bc898291db1080085fdcdde24067f53d6390896954b16cf0cee56ce53d0f5f8b7eb56c0b44a1ae4f63f4a40a8f01572255550f19f5c3054c102fe4732b0c"
        )
    );
    OpenMlsRustCrypto::default()
        .crypto()
        .verify_signature(
            SUITE.signature_algorithm(),
            &message,
            &public_key,
            &signature,
        )
        .unwrap();
    for invalid in [
        b"arbitrary bytes".to_vec(),
        message[..message.len() - 1].to_vec(),
        [message.as_slice(), b"extra\n"].concat(),
        String::from_utf8(message.clone())
            .unwrap()
            .replace("authenticate:v2", "register:v1")
            .into_bytes(),
        String::from_utf8(message.clone())
            .unwrap()
            .replace("\n", "\r\n")
            .into_bytes(),
    ] {
        assert!(matches!(
            engine.sign_auth_challenge(client.client_handle.clone(), invalid),
            Err(MlsError::InvalidInput)
        ));
    }
    assert!(matches!(
        engine.sign_auth_challenge("unknown-client".into(), message),
        Err(MlsError::UnknownClient)
    ));
}

#[test]
fn key_package_validation_checks_real_signatures_expected_key_and_exact_wire_framing() {
    let engine = MlsEngine::new();
    let alice = engine.create_client(b"validation-alice".to_vec()).unwrap();
    let bob = engine.create_client(b"validation-bob".to_vec()).unwrap();
    let package = engine.generate_key_package(bob.client_handle).unwrap();
    assert_eq!(
        engine
            .validate_key_package(package.clone(), bob.signature_public_key.clone())
            .unwrap(),
        bob.credential_identity
    );
    assert!(matches!(
        engine.validate_key_package(package.clone(), alice.signature_public_key),
        Err(MlsError::InvalidMessage)
    ));
    let mut corrupted = package.clone();
    *corrupted.last_mut().unwrap() ^= 1;
    assert!(matches!(
        engine.validate_key_package(corrupted, bob.signature_public_key.clone()),
        Err(MlsError::InvalidMessage)
    ));
    let mut trailing = package;
    trailing.push(0);
    assert!(matches!(
        engine.validate_key_package(trailing, bob.signature_public_key.clone()),
        Err(MlsError::InvalidMessage)
    ));
    assert!(
        engine
            .validate_key_package(vec![0; 16], bob.signature_public_key)
            .is_err()
    );
}

#[test]
fn checked_welcome_rejects_wrong_group_before_activation_and_persists_key_consumption() {
    let store = Arc::new(MemoryStore::default());
    let mut engine = MlsEngine::open(store.clone(), true).unwrap();
    let alice = engine.create_client(b"welcome-alice".to_vec()).unwrap();
    let bob = engine.create_client(b"welcome-bob".to_vec()).unwrap();
    let group = b"actual-welcome-group".to_vec();
    engine
        .create_group(alice.client_handle.clone(), group.clone())
        .unwrap();
    let package = engine
        .generate_key_package(bob.client_handle.clone())
        .unwrap();
    let added = engine
        .add_member(alice.client_handle.clone(), group.clone(), package)
        .unwrap();
    engine
        .merge_pending_commit(alice.client_handle, group.clone())
        .unwrap();
    let before = store.state_revision().unwrap();
    assert!(matches!(
        engine.process_welcome_checked(
            bob.client_handle.clone(),
            added.welcome.clone(),
            b"wrong-group".to_vec(),
            alice.signature_public_key.clone()
        ),
        Err(MlsError::InvalidMessage)
    ));
    assert!(store.state_revision().unwrap() > before);
    assert!(matches!(
        engine.group_info(bob.client_handle.clone(), group.clone()),
        Err(MlsError::UnknownGroup)
    ));
    drop(engine);
    engine = MlsEngine::open(store, false).unwrap();
    assert!(
        engine
            .process_welcome_checked(
                bob.client_handle,
                added.welcome,
                group,
                alice.signature_public_key
            )
            .is_err()
    );
}

#[test]
fn checked_welcome_rejects_an_unexpected_peer_and_extra_group_members() {
    for extra_member in [false, true] {
        let engine = MlsEngine::new();
        let alice = engine.create_client(b"members-alice".to_vec()).unwrap();
        let bob = engine.create_client(b"members-bob".to_vec()).unwrap();
        let mallory = engine.create_client(b"members-mallory".to_vec()).unwrap();
        let group = b"checked-member-set".to_vec();
        engine
            .create_group(alice.client_handle.clone(), group.clone())
            .unwrap();
        if extra_member {
            let package = engine
                .generate_key_package(mallory.client_handle.clone())
                .unwrap();
            let welcome = engine
                .add_member(alice.client_handle.clone(), group.clone(), package)
                .unwrap()
                .welcome;
            engine
                .merge_pending_commit(alice.client_handle.clone(), group.clone())
                .unwrap();
            engine
                .process_welcome(mallory.client_handle, welcome)
                .unwrap();
        }
        let package = engine
            .generate_key_package(bob.client_handle.clone())
            .unwrap();
        let welcome = engine
            .add_member(alice.client_handle.clone(), group.clone(), package)
            .unwrap()
            .welcome;
        engine
            .merge_pending_commit(alice.client_handle, group.clone())
            .unwrap();
        let expected = if extra_member {
            alice.signature_public_key
        } else {
            mallory.signature_public_key
        };
        assert!(matches!(
            engine.process_welcome_checked(
                bob.client_handle.clone(),
                welcome,
                group.clone(),
                expected
            ),
            Err(MlsError::InvalidMessage)
        ));
        assert!(matches!(
            engine.group_info(bob.client_handle, group),
            Err(MlsError::UnknownGroup)
        ));
    }
}

#[test]
fn checked_direct_group_and_peer_self_update_preserve_pinned_bindings() {
    let pair = pair(MlsEngine::new());
    let checked = pair
        .engine
        .check_direct_group(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            pair.bob.signature_public_key.clone(),
            pair.bob.credential_identity.clone(),
        )
        .unwrap();
    assert_eq!(checked.member_count, 2);
    assert!(matches!(
        pair.engine.check_direct_group(
            pair.bob.client_handle.clone(),
            pair.group.clone(),
            pair.alice.signature_public_key.clone(),
            b"wrong-peer-device".to_vec()
        ),
        Err(MlsError::InvalidMessage)
    ));
    let commit = pair
        .engine
        .self_update(pair.alice.client_handle.clone(), pair.group.clone())
        .unwrap();
    pair.engine
        .merge_pending_commit(pair.alice.client_handle.clone(), pair.group.clone())
        .unwrap();
    let next = pair
        .engine
        .process_direct_commit(
            pair.bob.client_handle.clone(),
            pair.group.clone(),
            commit,
            pair.alice.signature_public_key.clone(),
            pair.alice.credential_identity.clone(),
        )
        .unwrap();
    assert_eq!(next.epoch, 2);
    let wire = pair
        .engine
        .encrypt(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            serde_json::to_vec(&envelope("after verified self-update")).unwrap(),
        )
        .unwrap();
    assert_eq!(
        receive(&pair, wire).unwrap().body,
        "after verified self-update"
    );
}

#[test]
fn checked_direct_commit_rejects_membership_addition_without_activating_it() {
    let pair = pair(MlsEngine::new());
    let mallory = pair.engine.create_client(b"third-member".to_vec()).unwrap();
    let package = pair
        .engine
        .generate_key_package(mallory.client_handle)
        .unwrap();
    let added = pair
        .engine
        .add_member(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            package,
        )
        .unwrap();
    assert!(matches!(
        pair.engine.process_direct_commit(
            pair.bob.client_handle.clone(),
            pair.group.clone(),
            added.commit,
            pair.alice.signature_public_key.clone(),
            pair.alice.credential_identity.clone()
        ),
        Err(MlsError::InvalidMessage)
    ));
    let info = pair
        .engine
        .check_direct_group(
            pair.bob.client_handle,
            pair.group,
            pair.alice.signature_public_key,
            pair.alice.credential_identity,
        )
        .unwrap();
    assert_eq!(info.member_count, 2);
    assert_eq!(info.epoch, 1);
}

#[test]
fn direct_decrypt_returns_only_a_valid_bound_unicode_envelope_after_checkpoint() {
    let pair = pair(MlsEngine::new());
    let mut content = envelope("中文 👋\nAuthenticated text");
    content["created_at"] = serde_json::json!(MAX_JS_INTEGER);
    let wire = pair
        .engine
        .encrypt(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            serde_json::to_vec(&content).unwrap(),
        )
        .unwrap();
    let result = receive(&pair, wire.clone()).unwrap();
    assert_eq!(result.body, "中文 👋\nAuthenticated text");
    assert_eq!(result.created_at, MAX_JS_INTEGER);
    assert!(receive(&pair, wire).is_err());
}

#[test]
fn malformed_or_misbound_envelopes_are_rejected_and_their_ratchets_survive_restart() {
    let store = Arc::new(MemoryStore::default());
    let mut pair = pair(MlsEngine::open(store.clone(), true).unwrap());
    let good = envelope("Synthetic private text");
    let mut variants = vec![
        b"not JSON".to_vec(),
        [serde_json::to_vec(&good).unwrap(), b"{}".to_vec()].concat(),
    ];
    for (field, value) in [
        ("v", serde_json::json!(2)),
        (
            "client_message_id",
            serde_json::json!("60000000-0000-4000-8000-000000000002"),
        ),
        (
            "conversation_id",
            serde_json::json!("50000000-0000-4000-8000-000000000002"),
        ),
        ("from_void_id", serde_json::json!(BOB_ID)),
        ("to_void_id", serde_json::json!(ALICE_ID)),
        ("body", serde_json::json!("")),
        ("body", serde_json::json!("hidden\u{0}text")),
        ("body", serde_json::Value::Null),
        ("body", serde_json::json!(123)),
        ("created_at", serde_json::json!(-1)),
        ("created_at", serde_json::json!(1.5)),
        ("created_at", serde_json::json!(MAX_JS_INTEGER + 1)),
        ("unknown_field", serde_json::json!(true)),
    ] {
        let mut changed = good.clone();
        changed[field] = value;
        variants.push(serde_json::to_vec(&changed).unwrap());
    }
    let encoded = serde_json::to_string(&good).unwrap();
    variants.push(
        encoded
            .replacen("\"body\":", "\"body\":\"duplicate\",\"body\":", 1)
            .into_bytes(),
    );
    for invalid in variants {
        let wire = pair
            .engine
            .encrypt(
                pair.alice.client_handle.clone(),
                pair.group.clone(),
                invalid,
            )
            .unwrap();
        let before = store.state_revision().unwrap();
        assert!(matches!(
            receive(&pair, wire.clone()),
            Err(MlsError::InvalidMessage)
        ));
        assert!(store.state_revision().unwrap() > before);
        pair.engine = MlsEngine::open(store.clone(), false).unwrap();
        assert!(
            pair.engine
                .decrypt(pair.bob.client_handle.clone(), pair.group.clone(), wire)
                .is_err()
        );
    }
    let wire = pair
        .engine
        .encrypt(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();
    assert_eq!(receive(&pair, wire).unwrap().body, "Synthetic private text");
}

#[test]
fn direct_plaintext_is_withheld_when_its_checkpoint_fails() {
    let store = Arc::new(MemoryStore::default());
    let pair = pair(MlsEngine::open(store.clone(), true).unwrap());
    let wire = pair
        .engine
        .encrypt(
            pair.alice.client_handle.clone(),
            pair.group.clone(),
            serde_json::to_vec(&envelope("must remain private")).unwrap(),
        )
        .unwrap();
    store.fail_save.store(true, Ordering::SeqCst);
    assert!(matches!(
        receive(&pair, wire),
        Err(MlsError::StorageFailure)
    ));
    assert!(matches!(
        pair.engine.list_clients(),
        Err(MlsError::InvalidState)
    ));
}
