//! Recovery never decrypts again: all fixtures enter through real Signal first,
//! then assert that stored ratchet bytes do not change during local replay.
use super::*;
use crate::storage::signal::{
    crypto::{encrypt_content, incoming_plaintext},
    wire::Routing,
};
use crate::{
    relay::ApplicationProtocolVersion,
    storage::{events::Incoming, message_events::MessageReference},
};
use rusqlite::Transaction;

fn pair() -> (TestAccount, TestAccount, Database, Database) {
    let alice = TestAccount::new(ALICE, 123);
    let bob = TestAccount::new(BOB, 124);
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    (alice, bob, a, b)
}

fn content(index: u32) -> Value {
    json!({"v":2,"client_message_id":event_id(index),"from_void_id":ALICE,"to_void_id":BOB})
}

fn target(index: u32) -> MessageReference {
    MessageReference {
        author_void_id: ALICE.into(),
        event_id: event_id(index),
    }
}

fn reaction(index: u32, revision: i64, active: bool) -> Value {
    let mut value = content(index);
    value["type"] = json!("reaction");
    value["target"] = json!(target(90));
    value["emoji"] = json!("👍");
    value["active"] = json!(active);
    value["revision"] = json!(revision);
    value
}

fn encrypt_raw(a: &mut Database, bob: &TestAccount, index: u32, value: &Value) -> Vec<u8> {
    let id = event_id(index);
    let peer = bob.trusted();
    a.write(|tx| {
        encrypt_content(
            tx,
            &peer,
            Routing {
                event_id: &id,
                sender: ALICE,
                recipient: BOB,
            },
            &serde_json::to_vec(value).unwrap(),
        )
    })
    .unwrap()
}

fn defer(
    a: &mut Database,
    b: &mut Database,
    alice: &TestAccount,
    bob: &TestAccount,
    index: u32,
    value: &Value,
) -> Vec<u8> {
    let payload = encrypt_raw(a, bob, index, value);
    b.decrypt_signal_event_with_decoder(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(index), &payload),
        at(),
        previous_decoder,
    )
    .unwrap();
    payload
}

// Model the previous prerelease receiver, which durably deferred authenticated
// extensions even over V1 transport. The actual saved bytes remain unchanged.
pub(super) fn previous_decoder(
    tx: &Transaction<'_>,
    routing: Routing<'_>,
    plaintext: Zeroizing<Vec<u8>>,
    minimum: ApplicationProtocolVersion,
    received: i64,
    pinned: bool,
) -> crate::storage::Result<Incoming> {
    let content = crate::storage::signal::wire::decode_content(routing, &plaintext)?;
    if matches!(
        content,
        crate::storage::signal::wire::Content::Reaction(_)
            | crate::storage::signal::wire::Content::Delete(_)
    ) || matches!(content, crate::storage::signal::wire::Content::Message(ref message) if message.reply_to.is_some())
    {
        return Ok(Incoming::Deferred {
            plaintext,
            reason: "protocol_version_required".into(),
            min_protocol_version: minimum,
            received_at: received,
        });
    }
    incoming_plaintext(tx, routing, plaintext, minimum, received, pinned)
}

fn count(db: &Database, table: &str) -> i64 {
    assert!(matches!(
        table,
        "deferred_signal_events"
            | "messages"
            | "message_reactions"
            | "control_event_inbox"
            | "inbox"
            | "receipt_outbox"
    ));
    db.connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn experimental_minimum_two_stays_deferred_without_retagging_or_ratchet_changes() {
    let (alice, bob, mut a, mut b) = pair();
    defer(&mut a, &mut b, &alice, &bob, 1, &reaction(1, 1, true));
    // An existing prerelease record is not rewritten as part of consolidation.
    b.connection
        .execute(
            "UPDATE deferred_signal_events SET min_protocol_version='2'",
            [],
        )
        .unwrap();
    let state = session(&b, ALICE);
    b.close().unwrap();
    let mut b = bob.open();
    let stats = b.recover_deferred_events().unwrap();
    assert_eq!(
        (stats.recovered, stats.deferred, stats.quarantined),
        (0, 1, 0)
    );
    assert_eq!(
        b.connection
            .query_row(
                "SELECT min_protocol_version FROM deferred_signal_events",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "2"
    );
    assert_eq!(count(&b, "message_reactions"), 0);
    assert!(session(&b, ALICE) == state);
}

#[test]
fn restart_recovers_supported_replies_without_ratchet_changes_or_new_receipt_deadlines() {
    let (alice, bob, mut a, mut b) = pair();
    let mut reply = content(1);
    reply["body"] = json!("Recovered reply");
    reply["created_at"] = json!(NOW_MS);
    reply["reply_to"] = json!(target(90));
    reply["request_delivery_receipt"] = json!(1);
    let payload = defer(&mut a, &mut b, &alice, &bob, 1, &reply);
    let received: i64 = b
        .connection
        .query_row(
            "SELECT received_at FROM deferred_signal_events",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let state = session(&b, ALICE);
    b.close().unwrap();
    let mut b = bob.open();
    let stats = b.recover_deferred_events().unwrap();
    assert_eq!(
        (stats.recovered, stats.deferred, stats.quarantined),
        (1, 0, 0)
    );
    assert!(session(&b, ALICE) == state);
    assert_received(&b, ALICE, &event_id(1), "Recovered reply", NOW_MS);
    assert_eq!(
        b.reply_reference(&format!("{ALICE}:{}", event_id(1)))
            .unwrap(),
        Some(target(90))
    );
    let receipt: (i64, i64) = b
        .connection
        .query_row(
            "SELECT next_attempt_at,deadline_at FROM receipt_outbox",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, (received, received + 30 * 24 * 60 * 60 * 1000));
    assert_eq!(
        b.decrypt_signal_event(
            &alice.trusted(),
            &relay_event(ALICE, &event_id(1), &payload),
            at()
        )
        .unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 0);
    b.close().unwrap();
    let mut b = bob.open();
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 0);
    assert_eq!(count(&b, "messages"), 1);
    assert_eq!(count(&b, "receipt_outbox"), 1);
    assert!(session(&b, ALICE) == state);
}

#[test]
fn recovery_respects_deletes_revision_high_water_and_does_not_starve_after_unknown_pages() {
    let (alice, bob, mut a, mut b) = pair();
    for index in 1..=70 {
        let mut future = content(index);
        future["type"] = json!("future_event");
        defer(&mut a, &mut b, &alice, &bob, index, &future);
    }
    defer(&mut a, &mut b, &alice, &bob, 71, &reaction(71, 3, false));
    defer(&mut a, &mut b, &alice, &bob, 72, &reaction(72, 1, true));
    let mut original = content(90);
    original["body"] = json!("Must remain deleted");
    original["created_at"] = json!(NOW_MS);
    original["reply_to"] = json!(target(91));
    defer(&mut a, &mut b, &alice, &bob, 90, &original);
    let mut deletion = content(73);
    deletion["type"] = json!("delete_for_everyone");
    deletion["target"] = json!(target(90));
    defer(&mut a, &mut b, &alice, &bob, 73, &deletion);
    let state = session(&b, ALICE);
    let stats = b.recover_deferred_events().unwrap();
    assert_eq!((stats.recovered, stats.deferred), (4, 70));
    assert_received(&b, ALICE, &event_id(90), "", NOW_MS);
    assert!(b.message_deleted(ALICE, &target(90)).unwrap());
    assert!(
        b.reaction_states(ALICE, &target(90))
            .unwrap()
            .iter()
            .all(|row| !row.2)
    );
    assert!(session(&b, ALICE) == state);
    assert_eq!(b.recover_deferred_events().unwrap().deferred, 70);
    assert_eq!(count(&b, "messages"), 1);
}

fn upgraded_decoder(
    tx: &Transaction<'_>,
    routing: Routing<'_>,
    plaintext: Zeroizing<Vec<u8>>,
    minimum: ApplicationProtocolVersion,
    received: i64,
    pinned: bool,
) -> crate::storage::Result<Incoming> {
    let mut value: Value = serde_json::from_slice(&plaintext).map_err(|_| Error::SignalEnvelope)?;
    if value["type"] == "future_message" && value["v"] == 3 {
        value.as_object_mut().unwrap().remove("type");
        value["v"] = json!(2);
        return incoming_plaintext(
            tx,
            routing,
            Zeroizing::new(serde_json::to_vec(&value).unwrap()),
            minimum,
            received,
            pinned,
        );
    }
    incoming_plaintext(tx, routing, plaintext, minimum, received, pinned)
}

#[test]
fn a_new_decoder_reprocesses_saved_authenticated_bytes_without_ciphertext_replay() {
    let (alice, bob, mut a, mut b) = pair();
    let mut future = content(1);
    future["type"] = json!("future_message");
    future["v"] = json!(3);
    future["body"] = json!("Understood after upgrade");
    future["created_at"] = json!(NOW_MS);
    let payload = defer(&mut a, &mut b, &alice, &bob, 1, &future);
    assert_eq!(b.recover_deferred_events().unwrap().deferred, 1);
    let raw: Vec<u8> = b
        .connection
        .query_row("SELECT plaintext FROM deferred_signal_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(raw, serde_json::to_vec(&future).unwrap());
    let state = session(&b, ALICE);
    b.close().unwrap();
    let mut b = bob.open();
    assert_eq!(
        b.recover_deferred_with_decoder(upgraded_decoder)
            .unwrap()
            .recovered,
        1
    );
    assert_received(&b, ALICE, &event_id(1), "Understood after upgrade", NOW_MS);
    assert!(session(&b, ALICE) == state);
    assert_eq!(count(&b, "deferred_signal_events"), 0);
    assert_eq!(
        b.decrypt_signal_event(
            &alice.trusted(),
            &relay_event(ALICE, &event_id(1), &payload),
            at()
        )
        .unwrap(),
        IncomingOutcome::AlreadyStored
    );
}

#[test]
fn recovery_quarantines_invalid_content_authorization_and_provenance_without_blocking_valid_records()
 {
    let (alice, bob, mut a, mut b) = pair();
    for index in 1..=4 {
        defer(
            &mut a,
            &mut b,
            &alice,
            &bob,
            index,
            &reaction(index, 1, true),
        );
    }
    let mut unauthorized = content(1);
    unauthorized["type"] = json!("delete_for_everyone");
    unauthorized["target"] = json!({"author_void_id":BOB,"event_id":event_id(90)});
    b.connection
        .execute(
            "UPDATE deferred_signal_events SET plaintext=?2 WHERE event_id=?1",
            params![event_id(1), serde_json::to_vec(&unauthorized).unwrap()],
        )
        .unwrap();
    b.connection
        .execute(
            "UPDATE deferred_signal_events SET plaintext=x'7b' WHERE event_id=?1",
            [event_id(2)],
        )
        .unwrap();
    b.connection
        .execute(
            "UPDATE deferred_signal_events SET sender_void_id='303' WHERE event_id=?1",
            [event_id(3)],
        )
        .unwrap_err();
    // Corrupt only the saved routing in the third record; its ledger still proves
    // Alice's account, so a changed sender inside plaintext must not be accepted.
    let mut wrong = reaction(3, 1, true);
    wrong["from_void_id"] = json!("303");
    b.connection
        .execute(
            "UPDATE deferred_signal_events SET plaintext=?2 WHERE event_id=?1",
            params![event_id(3), serde_json::to_vec(&wrong).unwrap()],
        )
        .unwrap();
    let state = session(&b, ALICE);
    let stats = b.recover_deferred_events().unwrap();
    assert_eq!((stats.recovered, stats.quarantined), (1, 3));
    assert!(session(&b, ALICE) == state);
    assert_eq!(b.recover_deferred_events().unwrap().quarantined, 0);
    assert_eq!(count(&b, "messages"), 0);
    assert!(!b.message_deleted(ALICE, &target(90)).unwrap());
}

#[test]
fn failed_recovery_commit_restores_saved_plaintext_ledger_and_all_effects() {
    let (alice, bob, mut a, mut b) = pair();
    defer(&mut a, &mut b, &alice, &bob, 1, &reaction(1, 1, true));
    let raw: Vec<u8> = b
        .connection
        .query_row("SELECT plaintext FROM deferred_signal_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    let state = session(&b, ALICE);
    fail_next_event_commit(&b, "control_event_inbox");
    assert!(b.recover_deferred_events().is_err());
    assert!(b.reaction_states(ALICE, &target(90)).unwrap().is_empty());
    assert_eq!(count(&b, "deferred_signal_events"), 1);
    assert_eq!(count(&b, "control_event_inbox"), 1);
    let saved: Vec<u8> = b
        .connection
        .query_row("SELECT plaintext FROM deferred_signal_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(saved, raw);
    assert!(session(&b, ALICE) == state);
    allow_event_commit(&b);
    b.close().unwrap();
    let mut b = bob.open();
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 1);
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 0);
    assert!(session(&b, ALICE) == state);
}

#[test]
fn revision_conflicts_stay_deferred_until_a_higher_revision_makes_them_stale() {
    let (alice, bob, mut a, mut b) = pair();
    defer(&mut a, &mut b, &alice, &bob, 1, &reaction(1, 1, true));
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 1);
    defer(&mut a, &mut b, &alice, &bob, 2, &reaction(2, 1, false));
    assert_eq!(b.recover_deferred_events().unwrap().deferred, 1);
    assert!(b.reaction_states(ALICE, &target(90)).unwrap()[0].2);
    defer(&mut a, &mut b, &alice, &bob, 3, &reaction(3, 2, false));
    let stats = b.recover_deferred_events().unwrap();
    // The conflict remains on the first pass before the later revision is applied.
    assert_eq!((stats.recovered, stats.deferred), (1, 1));
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 1);
    assert_eq!(
        b.reaction_states(ALICE, &target(90)).unwrap()[0],
        (ALICE.into(), "👍".into(), false, 2)
    );
}

#[test]
fn schema_ten_upgrade_preserves_authenticated_deferred_plaintext_keys_and_ratchet() {
    use crate::storage::{
        MIGRATIONS, create_key, migrate_with_scripts, open_keyed, writable_flags,
    };
    let alice = TestAccount::new(ALICE, 123);
    let bob = TestAccount::new(BOB, 124);
    let mut a = alice.open();
    let directory = bob.dir.0.join("messages");
    std::fs::create_dir(&directory).unwrap();
    let key_path = directory.join("messages-key.dpapi");
    let key = create_key(&key_path).unwrap();
    let protected = std::fs::read(&key_path).unwrap();
    let mut connection =
        open_keyed(&directory.join("messages.db"), &key, writable_flags(true)).unwrap();
    let public = bob.key.verifying_key();
    let owner = Owner {
        void_id: BOB,
        public_key: public.as_bytes(),
    };
    migrate_with_scripts(&mut connection, owner, true, &MIGRATIONS[..10]).unwrap();
    let mut b = Database {
        connection,
        _lock: std::fs::File::create(bob.dir.0.join("old-format-test.lock")).unwrap(),
    };
    b.initialize_signal(BOB, &bob.key).unwrap();
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let payload = defer(&mut a, &mut b, &alice, &bob, 1, &reaction(1, 1, true));
    let raw: (Vec<u8>, Vec<u8>) = b
        .connection
        .query_row(
            "SELECT d.plaintext,c.payload_digest FROM deferred_signal_events d
        JOIN control_event_inbox c USING(sender_void_id,event_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let state = session(&b, ALICE);
    b.close().unwrap();
    let mut b = bob.open();
    let recovered: (Vec<u8>, Vec<u8>, String) = b
        .connection
        .query_row(
            "SELECT d.plaintext,c.payload_digest,d.recovery_state
        FROM deferred_signal_events d JOIN control_event_inbox c USING(sender_void_id,event_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(
        raw.0 == recovered.0 && raw.1 == recovered.1,
        "migration changed authenticated event bytes"
    );
    assert_eq!(recovered.2, "pending");
    assert!(
        std::fs::read(&key_path).unwrap() == protected,
        "migration replaced the protected database key"
    );
    assert!(session(&b, ALICE) == state);
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 1);
    assert!(session(&b, ALICE) == state);
    assert_eq!(
        b.decrypt_signal_event(
            &alice.trusted(),
            &relay_event(ALICE, &event_id(1), &payload),
            at()
        )
        .unwrap(),
        IncomingOutcome::AlreadyStored
    );
}

#[test]
fn missing_authenticated_peer_provenance_is_quarantined_without_ratchet_changes() {
    let (alice, bob, mut a, mut b) = pair();
    defer(&mut a, &mut b, &alice, &bob, 1, &reaction(1, 1, true));
    let state = session(&b, ALICE);
    b.connection
        .execute(
            "DELETE FROM signal_peer_bindings WHERE peer_void_id=?1",
            [ALICE],
        )
        .unwrap();
    let stats = b.recover_deferred_events().unwrap();
    assert_eq!((stats.recovered, stats.quarantined), (0, 1));
    assert!(session(&b, ALICE) == state);
    assert!(b.reaction_states(ALICE, &target(90)).unwrap().is_empty());
    assert_eq!(count(&b, "deferred_signal_events"), 1);
}
