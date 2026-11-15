use super::*;
use crate::storage::tests::TestDir;
use ed25519_dalek::Signer;
use libsignal_protocol::{
    DeviceId, Direction, IdentityChange, IdentityKeyStore, KyberPreKeyStore, PreKeyStore,
    ProtocolAddress, ProtocolStore, SessionRecord, SessionStore, SignalProtocolError,
    SignedPreKeyStore,
};

const VOID_ID: &str = "123";
const PEER_ID: &str = "456";

fn open(root: &TestDir, account: &SigningKey) -> Database {
    let public_key = account.verifying_key();
    Database::open(
        &root.0,
        Owner {
            void_id: VOID_ID,
            public_key: public_key.as_bytes(),
        },
    )
    .unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().build().unwrap()
}

#[derive(PartialEq, Eq)]
struct LocalState {
    key_pair: Zeroizing<Vec<u8>>,
    device_id: u32,
    registration_id: u32,
    binding_signature: Vec<u8>,
    next_prekey_id: u32,
    signed_id: u32,
    last_resort_id: u32,
}

#[derive(PartialEq, Eq)]
struct SavedPrekey {
    kind: String,
    id: u32,
    record: Zeroizing<Vec<u8>>,
    last_resort: bool,
    published: bool,
}

fn local_state(db: &Database) -> LocalState {
    db.connection
        .query_row(
            "SELECT key_pair, device_id, registration_id, binding_signature,
                    next_prekey_id, signed_prekey_id, kyber_last_resort_id
             FROM signal_local_identity WHERE slot = 1",
            [],
            |row| {
                Ok(LocalState {
                    key_pair: Zeroizing::new(row.get(0)?),
                    device_id: row.get(1)?,
                    registration_id: row.get(2)?,
                    binding_signature: row.get(3)?,
                    next_prekey_id: row.get(4)?,
                    signed_id: row.get(5)?,
                    last_resort_id: row.get(6)?,
                })
            },
        )
        .unwrap()
}

fn inventory(db: &Database) -> Vec<SavedPrekey> {
    db.connection
        .prepare("SELECT kind, id, record, last_resort, published FROM signal_prekeys ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok(SavedPrekey {
                kind: row.get(0)?,
                id: row.get(1)?,
                record: Zeroizing::new(row.get(2)?),
                last_resort: row.get(3)?,
                published: row.get(4)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

async fn check_serialized_inventory(store: &impl ProtocolStore, records: &[SavedPrekey]) {
    for item in records {
        let encoded = Zeroizing::new(match item.kind.as_str() {
            "ec" => store.get_pre_key(item.id.into()).await.unwrap().serialize().unwrap(),
            "signed" => store.get_signed_pre_key(item.id.into()).await.unwrap().serialize().unwrap(),
            "kyber" => {
                let record = store.get_kyber_pre_key(item.id.into()).await.unwrap();
                assert_eq!(record.key_pair().unwrap().public_key.key_type(), kem::KeyType::Kyber1024);
                record.serialize().unwrap()
            }
            _ => panic!("unexpected prekey kind"),
        });
        assert!(encoded.as_slice() == item.record.as_slice());
    }
}

#[test]
fn identity_binding_and_inventory_survive_reopen_without_regeneration() {
    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let public_key = account.verifying_key();
    let account_signature = account.sign(b"existing account signing proof");
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();

    let state = local_state(&db);
    let records = inventory(&db);
    assert_eq!(state.device_id, DEVICE_ID);
    assert!((1..=16_380).contains(&state.registration_id));
    assert_eq!(state.next_prekey_id, NEXT_ID_AFTER_INITIAL);
    assert_eq!(state.signed_id, INITIAL_SIGNED_ID);
    assert_eq!(state.last_resort_id, INITIAL_LAST_RESORT_ID);
    assert_eq!(records.iter().filter(|item| item.kind == "signed").count(), 1);
    assert_eq!(records.iter().filter(|item| item.kind == "ec").count(), 20);
    assert_eq!(records.iter().filter(|item| item.kind == "kyber" && !item.last_resort).count(), 20);
    assert_eq!(records.iter().filter(|item| item.last_resort).count(), 1);
    assert!(records.iter().all(|item| !item.published));
    assert_eq!(
        records.iter().map(|item| item.id).collect::<Vec<_>>(),
        (1..NEXT_ID_AFTER_INITIAL).collect::<Vec<_>>(),
    );

    let rt = runtime();
    db.write(|tx| {
        let store = adapters::SignalStore::new(tx);
        rt.block_on(async {
            let pair = store.get_identity_key_pair().await.unwrap();
            let signal_public = pair.identity_key().serialize();
            assert!(pair.serialize().as_ref() == state.key_pair.as_slice());
            assert_eq!(store.get_local_registration_id().await.unwrap(), state.registration_id);
            assert!(binding::verify_binding(
                &public_key, VOID_ID, state.device_id, state.registration_id,
                &signal_public, &state.binding_signature,
            ));
            assert!(!binding::verify_binding(
                &public_key, PEER_ID, state.device_id, state.registration_id,
                &signal_public, &state.binding_signature,
            ));
            check_serialized_inventory(&store, &records).await;
        });
        Ok(())
    })
    .unwrap();

    db.connection.execute(
        "UPDATE signal_prekeys SET published = 1 WHERE id = ?1",
        [INITIAL_SIGNED_ID],
    ).unwrap();
    let expected = inventory(&db);
    db.close().unwrap();
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    assert!(local_state(&db) == state);
    assert!(inventory(&db) == expected);
    assert_eq!(account.verifying_key(), public_key);
    assert_eq!(account.sign(b"existing account signing proof"), account_signature);
    assert!(matches!(
        db.initialize_signal(VOID_ID, &SigningKey::from_bytes(&[8; 32])),
        Err(Error::WrongAccount)
    ));
    assert!(local_state(&db) == state);
}

#[test]
fn failed_initialization_rolls_back_identity_and_all_keys() {
    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let mut db = open(&root, &account);
    db.connection.execute_batch(
        "CREATE TEMP TRIGGER reject_signal_identity BEFORE UPDATE ON signal_local_identity
         BEGIN SELECT RAISE(ABORT, 'test identity rollback'); END;",
    ).unwrap();

    assert!(db.initialize_signal(VOID_ID, &account).is_err());
    assert!(inventory(&db).is_empty());
    let (pristine, next_id) = db.connection.query_row(
        "SELECT key_pair IS NULL AND registration_id IS NULL AND binding_signature IS NULL,
                next_prekey_id FROM signal_local_identity WHERE slot = 1",
        [],
        |row| Ok((row.get::<_, bool>(0)?, row.get::<_, u32>(1)?)),
    ).unwrap();
    assert!(pristine);
    assert_eq!(next_id, 1);
    db.close().unwrap();

    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    assert_eq!(inventory(&db).len(), 42);
}

#[test]
fn damaged_or_missing_identity_never_replaces_existing_prekeys() {
    let malformed = [0xff_u8];
    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    let original = inventory(&db);
    db.connection.execute(
        "UPDATE signal_local_identity SET key_pair = ?1 WHERE slot = 1",
        [malformed.as_slice()],
    ).unwrap();

    for remove_identity in [false, true] {
        if remove_identity {
            db.connection.execute("DELETE FROM signal_local_identity WHERE slot = 1", []).unwrap();
        }
        db.close().unwrap();
        db = open(&root, &account);
        assert!(matches!(db.initialize_signal(VOID_ID, &account), Err(Error::SignalState)));
        let stored = db.connection.query_row(
            "SELECT key_pair FROM signal_local_identity WHERE slot = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        ).optional().unwrap().map(Zeroizing::new);
        assert!(match stored {
            Some(bytes) => !remove_identity && bytes.as_slice() == malformed,
            None => remove_identity,
        });
        assert!(inventory(&db) == original);
    }
}

#[test]
fn adapters_commit_and_roll_back_one_time_keys_sessions_and_replay_checks() {
    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    let rt = runtime();
    let peer = ProtocolAddress::new(PEER_ID.into(), DeviceId::new(1).unwrap());
    let mut rng = StdRng::seed_from_u64(17);
    let base_key = KeyPair::generate(&mut rng).public_key;
    let extra_key = PreKeyRecord::new(NEXT_ID_AFTER_INITIAL.into(), &KeyPair::generate(&mut rng));
    let session = SessionRecord::new_fresh();

    for commit in [false, true] {
        let outcome = db.write(|tx| {
            let mut store = adapters::SignalStore::new(tx);
            rt.block_on(async {
                assert!(store.load_session(&peer).await.unwrap().is_none());
                assert!(store.get_pre_key(3.into()).await.is_ok());
                assert!(store.get_kyber_pre_key(4.into()).await.is_ok());
                store.save_pre_key(NEXT_ID_AFTER_INITIAL.into(), &extra_key).await.unwrap();
                store.remove_pre_key(3.into()).await.unwrap();
                store.store_session(&peer, &session).await.unwrap();
                store.mark_kyber_pre_key_used(4.into(), INITIAL_SIGNED_ID.into(), &base_key).await.unwrap();
                store.mark_kyber_pre_key_used(INITIAL_LAST_RESORT_ID.into(), INITIAL_SIGNED_ID.into(), &base_key).await.unwrap();
                assert!(store.get_kyber_pre_key(INITIAL_LAST_RESORT_ID.into()).await.is_ok());
                assert!(matches!(
                    store.mark_kyber_pre_key_used(INITIAL_LAST_RESORT_ID.into(), INITIAL_SIGNED_ID.into(), &base_key).await,
                    Err(SignalProtocolError::InvalidMessage(..))
                ));
            });
            if commit { Ok(()) } else { Err(Error::SignalState) }
        });
        assert_eq!(outcome.is_ok(), commit);
    }

    db.close().unwrap();
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    assert_eq!(local_state(&db).next_prekey_id, NEXT_ID_AFTER_INITIAL + 1);
    db.write(|tx| {
        let mut store = adapters::SignalStore::new(tx);
        rt.block_on(async {
            assert!(matches!(store.get_pre_key(3.into()).await, Err(SignalProtocolError::InvalidPreKeyId)));
            assert!(matches!(store.get_kyber_pre_key(4.into()).await, Err(SignalProtocolError::InvalidKyberPreKeyId)));
            let loaded_key = Zeroizing::new(store.get_pre_key(NEXT_ID_AFTER_INITIAL.into()).await.unwrap().serialize().unwrap());
            assert!(loaded_key.as_slice() == Zeroizing::new(extra_key.serialize().unwrap()).as_slice());
            let loaded_session = Zeroizing::new(store.load_session(&peer).await.unwrap().unwrap().serialize().unwrap());
            assert!(loaded_session.as_slice() == Zeroizing::new(session.serialize().unwrap()).as_slice());
            assert!(matches!(
                store.mark_kyber_pre_key_used(INITIAL_LAST_RESORT_ID.into(), INITIAL_SIGNED_ID.into(), &base_key).await,
                Err(SignalProtocolError::InvalidMessage(..))
            ));
        });
        Ok(())
    }).unwrap();
}

#[test]
fn adapters_reject_unverified_identities_and_malformed_records() {
    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    let rt = runtime();
    let peer = ProtocolAddress::new(PEER_ID.into(), DeviceId::new(1).unwrap());
    let bad_device = ProtocolAddress::new(PEER_ID.into(), DeviceId::new(2).unwrap());
    let mut rng = StdRng::seed_from_u64(18);
    let trusted = IdentityKeyPair::generate(&mut rng);
    let replacement = IdentityKeyPair::generate(&mut rng);
    db.write::<()>(|tx| {
        let mut store = adapters::SignalStore::new(tx);
        rt.block_on(async {
            assert!(store.get_identity(&peer).await.unwrap().is_none());
            assert!(store.get_identity(&bad_device).await.is_err());
            assert!(store.save_identity(&peer, trusted.identity_key()).await.is_err());

            // Simulate a trusted key pinned by a future, separately verified account-binding flow.
            tx.execute(
                "INSERT INTO signal_identities VALUES (?1, 1, ?2)",
                params![PEER_ID, trusted.identity_key().serialize().as_ref()],
            ).unwrap();
            assert_eq!(
                store.save_identity(&peer, trusted.identity_key()).await.unwrap(),
                IdentityChange::NewOrUnchanged,
            );
            assert!(store.is_trusted_identity(&peer, trusted.identity_key(), Direction::Receiving).await.unwrap());
            assert!(!store.is_trusted_identity(&peer, replacement.identity_key(), Direction::Sending).await.unwrap());
            assert!(store.save_identity(&peer, replacement.identity_key()).await.is_err());

            tx.execute("UPDATE signal_identities SET identity_key = zeroblob(33)", []).unwrap();
            tx.execute("UPDATE signal_prekeys SET record = x'ff'", []).unwrap();
            tx.execute("INSERT INTO signal_sessions VALUES (?1, 1, x'ff')", [PEER_ID]).unwrap();
            assert!(store.get_identity(&peer).await.is_err());
            assert!(store.get_pre_key(3.into()).await.is_err());
            assert!(store.get_signed_pre_key(INITIAL_SIGNED_ID.into()).await.is_err());
            assert!(store.get_kyber_pre_key(INITIAL_LAST_RESORT_ID.into()).await.is_err());
            assert!(store.load_session(&peer).await.is_err());
        });
        Err(Error::SignalState)
    }).unwrap_err();
    db.initialize_signal(VOID_ID, &account).unwrap();
}

#[test]
fn event_transactions_include_official_prekey_store_writes() {
    use crate::storage::{DeliveryState, Direction as MessageDirection, Message};

    let root = TestDir::new();
    let account = SigningKey::from_bytes(&[7; 32]);
    let mut db = open(&root, &account);
    db.initialize_signal(VOID_ID, &account).unwrap();
    let conversation_id = db.create_conversation(PEER_ID, 100).unwrap();
    let rt = runtime();

    // These test payloads are opaque bytes. The event closures exercise storage only.
    let message = |id: &str, direction, delivery_state| Message {
        id: id.into(), conversation_id, direction, delivery_state,
        body: "Test message".into(), created_at: 120,
    };
    let incoming = |tx: &Transaction<'_>| {
        rt.block_on(adapters::SignalStore::new(tx).remove_pre_key(3.into()))
            .map_err(|_| Error::SignalState)?;
        Ok(message("in-message", MessageDirection::Incoming, DeliveryState::Received))
    };
    assert!(db.commit_outgoing(conversation_id, "out-event", |tx| {
        rt.block_on(adapters::SignalStore::new(tx).remove_pre_key(3.into()))
            .map_err(|_| Error::SignalState)?;
        Ok((message("out-message", MessageDirection::Outgoing, DeliveryState::Pending), Vec::new()))
    }).is_err());
    assert!(db.messages(conversation_id).unwrap().is_empty());
    assert!(inventory(&db).iter().any(|record| record.id == 3));

    let digest = [4; 32];
    assert!(db.commit_incoming(PEER_ID, "in-event", &digest, incoming).is_ok());
    assert!(db.commit_incoming(PEER_ID, "in-event", &digest, |_| panic!("duplicate ran the Signal store")).is_ok());
    assert!(!inventory(&db).iter().any(|record| record.id == 3));
    assert_eq!(db.messages(conversation_id).unwrap().len(), 1);
}
