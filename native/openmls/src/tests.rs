use crate::{MlsEngine, MlsError};

fn pair() -> (std::sync::Arc<MlsEngine>, String, String, Vec<u8>) {
    let engine = MlsEngine::new();
    let alice = engine
        .create_client(b"alice-fixture".to_vec())
        .unwrap()
        .client_handle;
    let bob = engine
        .create_client(b"bob-fixture".to_vec())
        .unwrap()
        .client_handle;
    let group = b"rust-test-group".to_vec();
    engine.create_group(alice.clone(), group.clone()).unwrap();
    let package = engine.generate_key_package(bob.clone()).unwrap();
    let added = engine
        .add_member(alice.clone(), group.clone(), package)
        .unwrap();
    assert!(matches!(
        engine.encrypt(alice.clone(), group.clone(), vec![]),
        Err(MlsError::InvalidState)
    ));
    engine
        .merge_pending_commit(alice.clone(), group.clone())
        .unwrap();
    engine.process_welcome(bob.clone(), added.welcome).unwrap();
    (engine, alice, bob, group)
}

#[test]
fn independent_clients_authenticated_exchange_tamper_commit_and_restore() {
    let report = MlsEngine::new().run_alice_bob_test().unwrap();
    assert!(
        report.alice_to_bob
            && report.bob_to_alice
            && report.tamper_rejected
            && report.commit_processed
            && report.snapshot_restored
    );
}

#[test]
fn replay_rejected_after_single_use_snapshot_restoration() {
    let (engine, alice, bob, group) = pair();
    let message = engine
        .encrypt(alice.clone(), group.clone(), b"authenticated".to_vec())
        .unwrap();
    engine
        .decrypt(bob.clone(), group.clone(), message.clone())
        .unwrap();
    let snapshot = engine.suspend_client(bob.clone()).unwrap();
    assert!(matches!(
        engine.group_info(bob, group.clone()),
        Err(MlsError::UnknownClient)
    ));
    let restored = engine
        .restore_client(snapshot.clone())
        .unwrap()
        .client_handle;
    assert!(matches!(
        engine.restore_client(snapshot),
        Err(MlsError::InvalidSnapshot)
    ));
    assert!(
        engine
            .decrypt(restored.clone(), group.clone(), message)
            .is_err()
    );
    let next = engine
        .encrypt(alice, group.clone(), b"after restore".to_vec())
        .unwrap();
    assert_eq!(
        engine.decrypt(restored, group, next).unwrap().plaintext,
        b"after restore"
    );
}

#[test]
fn pending_commit_survives_snapshot_and_advances_both_clients() {
    let (engine, alice, bob, group) = pair();
    let commit = engine.self_update(alice.clone(), group.clone()).unwrap();
    let snapshot = engine.suspend_client(alice).unwrap();
    let alice = engine.restore_client(snapshot).unwrap().client_handle;
    assert!(matches!(
        engine.encrypt(alice.clone(), group.clone(), vec![]),
        Err(MlsError::InvalidState)
    ));
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
    let message = engine.encrypt(bob, group.clone(), vec![]).unwrap();
    assert!(
        engine
            .decrypt(alice, group, message)
            .unwrap()
            .plaintext
            .is_empty()
    );
}

#[test]
fn key_package_private_material_survives_provider_recreation() {
    let engine = MlsEngine::new();
    let alice = engine
        .create_client(b"alice-fixture".to_vec())
        .unwrap()
        .client_handle;
    let bob = engine.create_client(b"bob-fixture".to_vec()).unwrap();
    let package = engine
        .generate_key_package(bob.client_handle.clone())
        .unwrap();
    let snapshot = engine.suspend_client(bob.client_handle).unwrap();
    let restored = engine.restore_client(snapshot).unwrap();
    assert_eq!(restored.signature_public_key, bob.signature_public_key);
    let group = b"key-package-restart".to_vec();
    engine.create_group(alice.clone(), group.clone()).unwrap();
    let added = engine
        .add_member(alice.clone(), group.clone(), package)
        .unwrap();
    engine
        .merge_pending_commit(alice.clone(), group.clone())
        .unwrap();
    engine
        .process_welcome(restored.client_handle.clone(), added.welcome.clone())
        .unwrap();
    assert!(
        engine
            .process_welcome(restored.client_handle.clone(), added.welcome)
            .is_err()
    );
    let message = engine
        .encrypt(alice, group.clone(), b"restored key package".to_vec())
        .unwrap();
    assert_eq!(
        engine
            .decrypt(restored.client_handle, group, message)
            .unwrap()
            .plaintext,
        b"restored key package"
    );
}

#[test]
fn malformed_trailing_wrong_kind_and_wrong_group_messages_rejected() {
    let (engine, alice, bob, group) = pair();
    let message = engine
        .encrypt(alice.clone(), group.clone(), b"valid".to_vec())
        .unwrap();
    let mut trailing = message.clone();
    trailing.push(0);
    assert!(
        engine
            .decrypt(bob.clone(), group.clone(), trailing)
            .is_err()
    );
    assert!(
        engine
            .decrypt(bob.clone(), group.clone(), vec![0; 10])
            .is_err()
    );
    assert!(
        engine
            .process_commit(bob.clone(), group.clone(), message.clone())
            .is_err()
    );
    assert!(matches!(
        engine.decrypt(bob.clone(), b"wrong-group".to_vec(), message.clone()),
        Err(MlsError::UnknownGroup)
    ));
    assert_eq!(
        engine.decrypt(bob, group, message).unwrap().plaintext,
        b"valid"
    );
    assert!(matches!(
        engine.create_client(vec![]),
        Err(MlsError::InvalidInput)
    ));
}

#[test]
fn serialized_storage_recreates_engine_without_exporting_secret_ffi() {
    let (engine, alice, bob, group) = pair();
    let pending = engine
        .encrypt(alice, group.clone(), b"provider restart".to_vec())
        .unwrap();
    // Exercise the internal serialization boundary across entirely new engine objects.
    let bytes = {
        let mut state = engine.state.lock().unwrap();
        let client = state.clients.remove(&bob).unwrap();
        crate::persistence::serialize(&client).unwrap()
    };
    drop(engine);
    let fresh = MlsEngine::new();
    let client = crate::persistence::restore(&bytes).unwrap();
    fresh
        .state
        .lock()
        .unwrap()
        .clients
        .insert("restored-test-only".into(), client);
    assert_eq!(
        fresh
            .decrypt("restored-test-only".into(), group, pending)
            .unwrap()
            .plaintext,
        b"provider restart"
    );
}

#[test]
fn sending_ratchet_survives_restoration_without_reusing_a_generation() {
    let (engine, alice, bob, group) = pair();
    let before = engine
        .encrypt(alice.clone(), group.clone(), b"before suspension".to_vec())
        .unwrap();
    engine.decrypt(bob.clone(), group.clone(), before).unwrap();
    let snapshot = engine.suspend_client(alice).unwrap();
    let restored = engine.restore_client(snapshot).unwrap().client_handle;
    let after = engine
        .encrypt(restored, group.clone(), b"after suspension".to_vec())
        .unwrap();
    // Reusing the previously consumed sending generation would fail at this receiver.
    assert_eq!(
        engine.decrypt(bob, group, after).unwrap().plaintext,
        b"after suspension"
    );
}
