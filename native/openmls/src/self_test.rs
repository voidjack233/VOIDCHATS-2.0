//! Alice/Bob are isolated diagnostic fixtures, never application accounts.
use crate::{MlsEngine, MlsError, OPENMLS_VERSION, SelfTestReport};

pub(crate) fn run() -> Result<SelfTestReport, MlsError> {
    let engine = MlsEngine::new();
    let alice = engine.create_client(b"alice-test-credential".to_vec())?;
    let bob = engine.create_client(b"bob-test-credential".to_vec())?;
    let group = b"void-native-mls-self-test".to_vec();
    engine.create_group(alice.client_handle.clone(), group.clone())?;
    let package = engine.generate_key_package(bob.client_handle.clone())?;
    let added = engine.add_member(alice.client_handle.clone(), group.clone(), package)?;
    engine.merge_pending_commit(alice.client_handle.clone(), group.clone())?;
    engine.process_welcome(bob.client_handle.clone(), added.welcome)?;

    let first = engine.encrypt(
        alice.client_handle.clone(),
        group.clone(),
        b"Alice to Bob".to_vec(),
    )?;
    let received = engine.decrypt(bob.client_handle.clone(), group.clone(), first)?;
    let alice_to_bob = received.plaintext == b"Alice to Bob"
        && received.sender_identity == alice.credential_identity
        && received.sender_signature_public_key == alice.signature_public_key;
    let mut tampered = engine.encrypt(
        alice.client_handle.clone(),
        group.clone(),
        b"Tamper fixture".to_vec(),
    )?;
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let tamper_rejected = engine
        .decrypt(bob.client_handle.clone(), group.clone(), tampered)
        .is_err();
    // Invalid authentication can consume a receive generation; use a fresh message.
    let fresh = engine.encrypt(
        alice.client_handle.clone(),
        group.clone(),
        b"After tamper".to_vec(),
    )?;
    if engine
        .decrypt(bob.client_handle.clone(), group.clone(), fresh)?
        .plaintext
        != b"After tamper"
    {
        return Err(MlsError::InvalidState);
    }
    let reply = engine.encrypt(
        bob.client_handle.clone(),
        group.clone(),
        b"Bob to Alice".to_vec(),
    )?;
    let received = engine.decrypt(alice.client_handle.clone(), group.clone(), reply)?;
    let bob_to_alice = received.plaintext == b"Bob to Alice"
        && received.sender_identity == bob.credential_identity
        && received.sender_signature_public_key == bob.signature_public_key;

    let commit = engine.self_update(alice.client_handle.clone(), group.clone())?;
    let alice_epoch = engine
        .merge_pending_commit(alice.client_handle.clone(), group.clone())?
        .epoch;
    let bob_epoch = engine
        .process_commit(bob.client_handle.clone(), group.clone(), commit)?
        .epoch;
    let after_commit = engine.encrypt(alice.client_handle, group.clone(), b"New epoch".to_vec())?;
    let snapshot = engine.suspend_client(bob.client_handle)?;
    let restored = engine.restore_client(snapshot)?;
    let received = engine.decrypt(restored.client_handle, group, after_commit)?;
    let snapshot_restored = restored.signature_public_key == bob.signature_public_key
        && received.plaintext == b"New epoch";
    let commit_processed =
        alice_epoch == 2 && bob_epoch == alice_epoch && received.epoch == alice_epoch;
    if !(alice_to_bob && bob_to_alice && tamper_rejected && commit_processed && snapshot_restored) {
        return Err(MlsError::InvalidState);
    }
    Ok(SelfTestReport {
        openmls_version: OPENMLS_VERSION.into(),
        alice_to_bob,
        bob_to_alice,
        tamper_rejected,
        commit_processed,
        snapshot_restored,
    })
}
