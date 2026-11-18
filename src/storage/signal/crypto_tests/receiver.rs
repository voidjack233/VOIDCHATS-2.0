//! Receiver fixtures use real libsignal ciphertext, isolated keys and SQLCipher.
//! Arbitrary plaintext encryption is confined to this test module.
use super::*;
use crate::storage::message_events::MessageReference;
use crate::storage::signal::{crypto::encrypt_content, wire::Routing};

fn pair() -> (TestAccount, TestAccount, Database, Database) {
    let alice = TestAccount::new(ALICE, 121);
    let bob = TestAccount::new(BOB, 122);
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    (alice, bob, a, b)
}

fn reference(author: &str, index: u32) -> MessageReference {
    MessageReference {
        author_void_id: author.into(),
        event_id: event_id(index),
    }
}

fn content(sender: &str, recipient: &str, index: u32) -> Value {
    json!({"v":2,"client_message_id":event_id(index),"from_void_id":sender,"to_void_id":recipient})
}

fn reaction(index: u32, target: &MessageReference, active: bool, revision: i64) -> Value {
    let mut value = content(ALICE, BOB, index);
    value["type"] = json!("reaction");
    value["target"] = json!(target);
    value["emoji"] = json!("👍");
    value["active"] = json!(active);
    value["revision"] = json!(revision);
    value
}

fn deletion(index: u32, target: &MessageReference) -> Value {
    let mut value = content(ALICE, BOB, index);
    value["type"] = json!("delete_for_everyone");
    value["target"] = json!(target);
    value
}

fn encrypt_raw(
    db: &mut Database,
    peer: &TrustedPeer,
    sender: &str,
    index: u32,
    value: &Value,
) -> Vec<u8> {
    let id = event_id(index);
    let routing = Routing {
        event_id: &id,
        sender,
        recipient: peer.void_id(),
    };
    let bytes = Zeroizing::new(serde_json::to_vec(value).unwrap());
    db.write(|tx| encrypt_content(tx, peer, routing, &bytes))
        .unwrap()
}

fn event(sender: &str, index: u32, payload: &[u8], versioned: bool) -> Vec<u8> {
    let mut value: Value =
        serde_json::from_slice(&relay_event(sender, &event_id(index), payload)).unwrap();
    if versioned {
        value["type"] = json!("event_versioned");
        value["min_protocol_version"] = json!("2");
    }
    serde_json::to_vec(&value).unwrap()
}

fn receive(
    db: &mut Database,
    peer: &TrustedPeer,
    index: u32,
    payload: &[u8],
) -> crate::storage::Result<IncomingOutcome> {
    db.decrypt_signal_event(peer, &event(peer.void_id(), index, payload, true), at())
}

fn count(db: &Database, query: &str) -> i64 {
    db.connection
        .query_row(query, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn replies_persist_stable_local_and_missing_original_references_across_restart() {
    let (alice, bob, mut a, mut b) = pair();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let original = a
        .encrypt_signal_message(&bob.trusted(), chat, &event_id(1), "Original", NOW_MS)
        .unwrap();
    b.decrypt_signal_event(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(1), &original),
        at(),
    )
    .unwrap();
    let mut reply = content(BOB, ALICE, 2);
    reply["body"] = json!("Reply to your message");
    reply["created_at"] = json!(NOW_MS + 1);
    reply["request_delivery_receipt"] = json!(1);
    reply["reply_to"] = json!(reference(ALICE, 1));
    let payload = encrypt_raw(&mut b, &alice.trusted(), BOB, 2, &reply);
    receive(&mut a, &bob.trusted(), 2, &payload).unwrap();
    let reply_id = format!("{BOB}:{}", event_id(2));
    assert_eq!(
        a.reply_reference(&reply_id).unwrap(),
        Some(reference(ALICE, 1))
    );
    assert_eq!(a.messages(chat).unwrap().len(), 2);
    assert_eq!(count(&a, "SELECT count(*) FROM receipt_outbox"), 1);
    a.save_draft(chat, "Keep this draft").unwrap();
    let anchor = a.conversation_timeline(chat).unwrap()[0].cursor.clone();
    a.close().unwrap();
    let a = alice.open();
    assert_eq!(
        a.reply_reference(&reply_id).unwrap(),
        Some(reference(ALICE, 1))
    );
    assert_eq!(a.draft(chat).unwrap().as_deref(), Some("Keep this draft"));
    assert_eq!(a.conversation_timeline(chat).unwrap()[0].cursor, anchor);

    let mut missing = content(ALICE, BOB, 3);
    missing["body"] = json!("Original not here yet");
    missing["created_at"] = json!(NOW_MS + 2);
    missing["reply_to"] = json!(reference(ALICE, 40));
    // Reopen Alice without changing identity or session before making another fixture.
    a.close().unwrap();
    let mut a = alice.open();
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 3, &missing);
    receive(&mut b, &alice.trusted(), 3, &payload).unwrap();
    assert_eq!(
        b.reply_reference(&format!("{ALICE}:{}", event_id(3)))
            .unwrap(),
        Some(reference(ALICE, 40))
    );
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 2);
    let later = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(40),
            "Now available",
            NOW_MS + 3,
        )
        .unwrap();
    b.decrypt_signal_event(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(40), &later),
        at(),
    )
    .unwrap();
    assert_received(&b, ALICE, &event_id(40), "Now available", NOW_MS + 3);
}

#[test]
fn reactions_reconcile_out_of_order_removals_duplicates_and_revision_conflicts() {
    let (alice, bob, mut a, mut b) = pair();
    let target = reference(ALICE, 40);
    let first = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        1,
        &reaction(1, &target, true, 1),
    );
    let removal = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        2,
        &reaction(2, &target, false, 3),
    );
    let stale = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        3,
        &reaction(3, &target, true, 2),
    );
    receive(&mut b, &alice.trusted(), 2, &removal).unwrap();
    receive(&mut b, &alice.trusted(), 1, &first).unwrap();
    receive(&mut b, &alice.trusted(), 3, &stale).unwrap();
    assert_eq!(
        b.reaction_states(ALICE, &target).unwrap(),
        vec![(ALICE.into(), "👍".into(), false, 3)]
    );
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM conversations"), 0);
    let before = session(&b, ALICE);
    assert_eq!(
        receive(&mut b, &alice.trusted(), 2, &removal).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(session(&b, ALICE) == before);
    let conflict = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        4,
        &reaction(4, &target, true, 3),
    );
    receive(&mut b, &alice.trusted(), 4, &conflict).unwrap();
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM deferred_signal_events WHERE reason='reaction_revision_conflict'"
        ),
        1
    );
    assert!(!b.reaction_states(ALICE, &target).unwrap()[0].2);
    let newer = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        5,
        &reaction(5, &target, true, 4),
    );
    receive(&mut b, &alice.trusted(), 5, &newer).unwrap();
    b.close().unwrap();
    let b = bob.open();
    assert_eq!(
        b.reaction_states(ALICE, &target).unwrap(),
        vec![(ALICE.into(), "👍".into(), true, 4)]
    );
    assert_eq!(count(&b, "SELECT count(*) FROM control_event_inbox"), 5);
}

#[test]
fn deletion_before_original_survives_restart_without_resurrection_or_phantom_unread() {
    let (alice, bob, mut a, mut b) = pair();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let original = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(1),
            "Remove this original",
            NOW_MS,
        )
        .unwrap();
    let target = reference(ALICE, 1);
    let delete = encrypt_raw(&mut a, &bob.trusted(), ALICE, 2, &deletion(2, &target));
    receive(&mut b, &alice.trusted(), 2, &delete).unwrap();
    assert!(b.message_deleted(ALICE, &target).unwrap());
    assert_eq!(count(&b, "SELECT count(*) FROM conversations"), 0);
    b.close().unwrap();
    let mut b = bob.open();
    let original_event = relay_event(ALICE, &event_id(1), &original);
    b.decrypt_signal_event(&alice.trusted(), &original_event, at())
        .unwrap();
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    assert_eq!(count(&b, "SELECT count(*) FROM unread_messages"), 0);
    let state = session(&b, ALICE);
    assert_eq!(
        b.decrypt_signal_event(&alice.trusted(), &original_event, at())
            .unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(session(&b, ALICE) == state);
    let late_reaction = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        3,
        &reaction(3, &target, true, 1),
    );
    receive(&mut b, &alice.trusted(), 3, &late_reaction).unwrap();
    assert!(b.reaction_states(ALICE, &target).unwrap().is_empty());
    assert_eq!(count(&b, "SELECT count(*) FROM inbox"), 1);
}

#[test]
fn deletion_preserves_reply_references_drafts_history_ids_and_receipts() {
    let (alice, bob, mut a, mut b) = pair();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let original = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(1),
            "Sensitive original",
            NOW_MS,
        )
        .unwrap();
    b.decrypt_signal_event(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(1), &original),
        at(),
    )
    .unwrap();
    let target = reference(ALICE, 1);
    let mut reply = content(ALICE, BOB, 2);
    reply["body"] = json!("Follow-up");
    reply["created_at"] = json!(NOW_MS + 1);
    reply["reply_to"] = json!(target);
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 2, &reply);
    receive(&mut b, &alice.trusted(), 2, &payload).unwrap();
    let reaction = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        3,
        &reaction(3, &target, true, 1),
    );
    receive(&mut b, &alice.trusted(), 3, &reaction).unwrap();
    let conversation = b.conversations(true).unwrap().remove(0).id;
    b.save_draft(conversation, "Unsent draft").unwrap();
    let before = b
        .conversation_timeline(conversation)
        .unwrap()
        .into_iter()
        .map(|row| row.cursor)
        .collect::<Vec<_>>();
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 4, &deletion(4, &target));
    receive(&mut b, &alice.trusted(), 4, &payload).unwrap();
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    assert_received(&b, ALICE, &event_id(2), "Follow-up", NOW_MS + 1);
    assert_eq!(
        b.reply_reference(&format!("{ALICE}:{}", event_id(2)))
            .unwrap(),
        Some(target.clone())
    );
    assert_eq!(
        b.draft(conversation).unwrap().as_deref(),
        Some("Unsent draft")
    );
    assert_eq!(
        before,
        b.conversation_timeline(conversation)
            .unwrap()
            .into_iter()
            .map(|row| row.cursor)
            .collect::<Vec<_>>()
    );
    assert_eq!(count(&b, "SELECT count(*) FROM receipt_outbox"), 1);
    assert!(
        b.reaction_states(ALICE, &target)
            .unwrap()
            .iter()
            .all(|state| !state.2)
    );
}

#[test]
fn authenticated_unsupported_content_is_durable_and_does_not_strand_later_text() {
    let (alice, bob, mut a, mut b) = pair();
    let mut future = content(ALICE, BOB, 1);
    future["type"] = json!("future_control");
    future["future_data"] = json!({"retain":"exact data for a later receiver"});
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 1, &future);
    receive(&mut b, &alice.trusted(), 1, &payload).unwrap();
    let expected = serde_json::to_vec(&future).unwrap();
    let saved: (Vec<u8>, String, String) = b
        .connection
        .query_row(
            "SELECT plaintext, reason, min_protocol_version FROM deferred_signal_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        saved,
        (expected, "unsupported_event_type".into(), "2".into())
    );
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM conversations"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM receipt_outbox"), 0);
    let state = session(&b, ALICE);
    b.close().unwrap();
    let mut b = bob.open();
    assert_eq!(
        receive(&mut b, &alice.trusted(), 1, &payload).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(session(&b, ALICE) == state);
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let text = a
        .encrypt_signal_message(&bob.trusted(), chat, &event_id(2), "Still readable", NOW_MS)
        .unwrap();
    b.decrypt_signal_event(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(2), &text),
        at(),
    )
    .unwrap();
    assert_received(&b, ALICE, &event_id(2), "Still readable", NOW_MS);
}

#[test]
fn invalid_author_routing_and_reused_control_ids_do_not_change_signal_or_history() {
    let (alice, bob, mut a, mut b) = pair();
    let valid = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        1,
        &reaction(1, &reference(ALICE, 40), true, 1),
    );
    receive(&mut b, &alice.trusted(), 1, &valid).unwrap();
    let before = session(&b, ALICE);
    let mut invalid = deletion(2, &reference(BOB, 40));
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 2, &invalid);
    assert!(matches!(
        receive(&mut b, &alice.trusted(), 2, &payload),
        Err(Error::SignalEnvelope)
    ));
    assert!(session(&b, ALICE) == before);
    invalid = reaction(3, &reference("303", 40), true, 1);
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 3, &invalid);
    assert!(receive(&mut b, &alice.trusted(), 3, &payload).is_err());
    assert!(session(&b, ALICE) == before);
    invalid = content(ALICE, "303", 4);
    invalid["type"] = json!("future_control");
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 4, &invalid);
    assert!(receive(&mut b, &alice.trusted(), 4, &payload).is_err());
    assert!(session(&b, ALICE) == before);
    let payload = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        1,
        &reaction(1, &reference(ALICE, 40), false, 2),
    );
    assert!(matches!(
        receive(&mut b, &alice.trusted(), 1, &payload),
        Err(Error::EventConflict)
    ));
    assert!(session(&b, ALICE) == before);
    assert_eq!(count(&b, "SELECT count(*) FROM control_event_inbox"), 1);
    assert_eq!(count(&b, "SELECT count(*) FROM message_tombstones"), 0);
}

#[test]
fn failed_control_and_deferral_commits_roll_back_signal_then_retry_exact_ciphertext() {
    let (alice, bob, mut a, mut b) = pair();
    for index in [1, 2] {
        let value = if index == 1 {
            reaction(index, &reference(ALICE, 40), true, 1)
        } else {
            let mut future = content(ALICE, BOB, index);
            future["type"] = json!("future_control");
            future
        };
        let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, index, &value);
        let before = session(&b, ALICE);
        fail_next_event_commit(&b, "control_event_inbox");
        assert!(
            receive(&mut b, &alice.trusted(), index, &payload).is_err(),
            "failed commit must not authorize ACK"
        );
        assert!(session(&b, ALICE) == before);
        assert_eq!(
            count(&b, "SELECT count(*) FROM control_event_inbox"),
            i64::from(index) - 1
        );
        allow_event_commit(&b);
        assert_eq!(
            receive(&mut b, &alice.trusted(), index, &payload).unwrap(),
            IncomingOutcome::Stored
        );
        assert_eq!(
            receive(&mut b, &alice.trusted(), index, &payload).unwrap(),
            IncomingOutcome::AlreadyStored
        );
    }
    assert_eq!(count(&b, "SELECT count(*) FROM deferred_signal_events"), 1);
}

#[test]
fn extensions_arriving_as_legacy_events_are_preserved_for_safe_recovery() {
    let (alice, bob, mut a, mut b) = pair();
    let payload = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        1,
        &reaction(1, &reference(ALICE, 40), true, 1),
    );
    b.decrypt_signal_event(&alice.trusted(), &event(ALICE, 1, &payload, false), at())
        .unwrap();
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM deferred_signal_events WHERE reason='protocol_version_required'"
        ),
        1
    );
    assert_eq!(count(&b, "SELECT count(*) FROM message_reactions"), 0);
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 0);
}

#[test]
fn failed_deletion_commit_preserves_original_until_a_durable_retry() {
    let (alice, bob, mut a, mut b) = pair();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let original = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(1),
            "Original before deletion",
            NOW_MS,
        )
        .unwrap();
    b.decrypt_signal_event(
        &alice.trusted(),
        &relay_event(ALICE, &event_id(1), &original),
        at(),
    )
    .unwrap();
    let target = reference(ALICE, 1);
    let active = encrypt_raw(
        &mut a,
        &bob.trusted(),
        ALICE,
        2,
        &reaction(2, &target, true, 1),
    );
    receive(&mut b, &alice.trusted(), 2, &active).unwrap();
    let payload = encrypt_raw(&mut a, &bob.trusted(), ALICE, 3, &deletion(3, &target));
    let before = session(&b, ALICE);
    fail_next_event_commit(&b, "control_event_inbox");
    assert!(receive(&mut b, &alice.trusted(), 3, &payload).is_err());
    assert!(session(&b, ALICE) == before);
    assert_received(&b, ALICE, &event_id(1), "Original before deletion", NOW_MS);
    assert!(!b.message_deleted(ALICE, &target).unwrap());
    assert!(b.reaction_states(ALICE, &target).unwrap()[0].2);
    assert_eq!(count(&b, "SELECT count(*) FROM unread_messages"), 1);
    assert_eq!(count(&b, "SELECT count(*) FROM control_event_inbox"), 1);
    allow_event_commit(&b);
    receive(&mut b, &alice.trusted(), 3, &payload).unwrap();
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    assert!(b.message_deleted(ALICE, &target).unwrap());
    assert!(!b.reaction_states(ALICE, &target).unwrap()[0].2);
    assert_eq!(count(&b, "SELECT count(*) FROM unread_messages"), 0);
}
