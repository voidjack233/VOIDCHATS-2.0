//! Isolated SQLCipher + official Signal tests of durable outgoing feature intents.
use super::*;
use crate::storage::{message_events::MessageReference, messaging::SendState};

fn pair() -> (TestAccount, TestAccount, Database, Database, i64) {
    let alice = TestAccount::new(ALICE, 71);
    let bob = TestAccount::new(BOB, 72);
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    (alice, bob, a, b, chat)
}
fn reference(author: &str, id: &str) -> MessageReference {
    MessageReference {
        author_void_id: author.into(),
        event_id: id.into(),
    }
}
fn receive(
    db: &mut Database,
    peer: &TrustedPeer,
    id: &str,
    payload: &[u8],
) -> crate::storage::Result<IncomingOutcome> {
    db.decrypt_signal_event(peer, &relay_event(peer.void_id(), id, payload), at())
}
fn original(
    a: &mut Database,
    b: &mut Database,
    alice: &TestAccount,
    bob: &TestAccount,
    chat: i64,
) -> String {
    let (id, _) = a.queue_outgoing(chat, "Original", NOW_MS).unwrap();
    let payload = a
        .encrypt_signal_message(&bob.trusted(), chat, &id, "Original", NOW_MS)
        .unwrap();
    receive(b, &alice.trusted(), &id, &payload).unwrap();
    id
}

#[test]
fn outgoing_reply_draft_commit_receipt_and_ciphertext_survive_restart() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    a.save_reply_draft(chat, "Reply", Some(&target)).unwrap();
    a.close().unwrap();
    let mut a = alice.open();
    assert_eq!(a.reply_draft(chat).unwrap(), Some(target.clone()));
    let (reply, _) = a.queue_outgoing(chat, "Reply", NOW_MS + 1).unwrap();
    assert_eq!(a.outgoing_reply(&reply).unwrap(), Some(target.clone()));
    assert!(a.reply_draft(chat).unwrap().is_none());
    assert_eq!(
        a.timeline_before(chat, None, 1).unwrap()[0]
            .presentation
            .quote,
        "Original"
    );
    let payload = a
        .encrypt_signal_message(&bob.trusted(), chat, &reply, "Reply", NOW_MS + 1)
        .unwrap();
    let state = session(&a, BOB);
    a.set_send_state(&reply, SendState::Unconfirmed).unwrap();
    a.close().unwrap();
    let mut a = alice.open();
    a.recover_interrupted_sends().unwrap();
    assert!(state == session(&a, BOB));
    assert!(
        payload
            == a.encrypt_signal_message(&bob.trusted(), chat, &reply, "Reply", NOW_MS + 1)
                .unwrap()
    );
    assert!(state == session(&a, BOB));
    assert_eq!(
        receive(&mut b, &alice.trusted(), &reply, &payload).unwrap(),
        IncomingOutcome::Stored
    );
    assert_eq!(
        receive(&mut b, &alice.trusted(), &reply, &payload).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    let message = b
        .messages(b.conversations(true).unwrap()[0].id)
        .unwrap()
        .into_iter()
        .find(|m| m.body == "Reply")
        .unwrap();
    assert_eq!(b.reply_reference(&message.id).unwrap(), Some(target));
    let due = b.next_receipt_attempt(0).unwrap().unwrap();
    // Deliver both original and reply receipts using the unchanged V1 path.
    for _ in 0..2 {
        let upload = b.claim_receipt(due).unwrap().unwrap();
        let rid = upload.event_id.clone();
        let payload = b.encrypt_signal_receipt(upload).unwrap();
        a.decrypt_signal_event(&bob.trusted(), &relay_event(BOB, &rid, &payload), at())
            .unwrap();
        b.finish_receipt(&rid).unwrap();
    }
    assert!(a.has_verified_delivery(&reply).unwrap());
}

#[test]
fn outgoing_reactions_are_idempotent_revisioned_and_order_independent() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let add = event_id(501);
    let remove = event_id(502);
    assert_eq!(
        a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
            .unwrap(),
        add
    );
    assert_eq!(
        a.queue_reaction(chat, &event_id(503), &target, "👍", true, NOW_MS)
            .unwrap(),
        add
    );
    assert!(
        a.queue_reaction(chat, &add, &target, "👍", false, NOW_MS)
            .is_err()
    );
    let p1 = a.encrypt_signal_control(&bob.trusted(), &add).unwrap();
    a.queue_reaction(chat, &remove, &target, "👍", false, NOW_MS)
        .unwrap();
    let p2 = a.encrypt_signal_control(&bob.trusted(), &remove).unwrap();
    assert_eq!(a.control(&remove).unwrap().revision, Some(2));
    // Later Signal message first, then the skipped-key predecessor.
    receive(&mut b, &alice.trusted(), &remove, &p2).unwrap();
    receive(&mut b, &alice.trusted(), &add, &p1).unwrap();
    receive(&mut b, &alice.trusted(), &remove, &p2).unwrap();
    assert!(!b.reaction_states(ALICE, &target).unwrap()[0].2);
    a.set_control_state(&remove, "unconfirmed").unwrap();
    let state = session(&a, BOB);
    a.close().unwrap();
    let mut a = alice.open();
    assert!(a.pending_controls().unwrap().contains(&remove));
    assert_eq!(a.control(&remove).unwrap().state, "unconfirmed");
    assert!(p2 == a.encrypt_signal_control(&bob.trusted(), &remove).unwrap());
    assert!(state == session(&a, BOB));
    let again = event_id(504);
    a.queue_reaction(chat, &again, &target, "👍", true, NOW_MS)
        .unwrap();
    assert_eq!(a.control(&again).unwrap().revision, Some(3));
    assert_eq!(
        a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
            .unwrap(),
        add
    );
    assert_eq!(a.reaction_states(BOB, &target).unwrap()[0].3, 3);
}

#[test]
fn outgoing_reaction_limit_scope_and_write_rollback() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let other = a.create_conversation("303", NOW_MS).unwrap();
    assert!(
        a.queue_reaction(other, &event_id(510), &target, "👍", true, NOW_MS)
            .is_err()
    );
    assert!(
        a.queue_reaction(chat, &id, &target, "👍", true, NOW_MS)
            .is_err()
    );
    assert!(
        a.queue_reaction(
            chat,
            &event_id(510),
            &reference("303", &id),
            "👍",
            true,
            NOW_MS
        )
        .is_err()
    );
    let emojis = ["👍", "👎", "😀", "❤️", "🎉", "🔥", "😢", "😮", "🙏", "✅"];
    for (i, emoji) in emojis.iter().enumerate() {
        a.queue_reaction(
            chat,
            &event_id(520 + i as u32),
            &target,
            emoji,
            true,
            NOW_MS,
        )
        .unwrap();
    }
    assert!(matches!(
        a.queue_reaction(chat, &event_id(540), &target, "💯", true, NOW_MS),
        Err(Error::ReactionLimit)
    ));
    a.queue_reaction(chat, &event_id(541), &target, "👍", false, NOW_MS)
        .unwrap();
    a.queue_reaction(chat, &event_id(542), &target, "💯", true, NOW_MS)
        .unwrap();
    a.connection.execute_batch("CREATE TEMP TRIGGER fail_reaction BEFORE UPDATE ON message_reactions BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(
        a.queue_reaction(chat, &event_id(543), &target, "💯", false, NOW_MS)
            .is_err()
    );
    assert!(a.control(&event_id(543)).is_err());
    assert!(
        a.reaction_states(BOB, &target)
            .unwrap()
            .iter()
            .find(|r| r.1 == "💯")
            .unwrap()
            .2
    );
}

#[test]
fn pending_reaction_toggles_coalesce_without_consuming_signal_state() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let ratchet = session(&a, BOB);
    let ids = [event_id(601), event_id(602), event_id(603)];
    for (index, active) in [true, false, true].into_iter().enumerate() {
        // Wall-clock order must not override monotonic reaction revisions.
        a.queue_reaction(
            chat,
            &ids[index],
            &target,
            "👍",
            active,
            NOW_MS - index as i64,
        )
        .unwrap();
    }
    assert_eq!(a.pending_controls().unwrap(), vec![ids[2].clone()]);
    for id in &ids[..2] {
        assert!(a.control_superseded(id).unwrap());
        assert!(a.encrypt_signal_control(&bob.trusted(), id).is_err());
        assert!(a.control(id).unwrap().payload.is_none());
        assert_eq!(a.control(id).unwrap().state, "pending"); // Not falsely accepted.
    }
    assert!(session(&a, BOB) == ratchet);
    assert_eq!(a.control(&ids[0]).unwrap().active, Some(true));
    assert_eq!(a.control(&ids[1]).unwrap().active, Some(false));
    a.close().unwrap();
    let mut a = alice.open();
    // Submission retries keep the original meaning and cannot undo later choices.
    assert_eq!(
        a.queue_reaction(chat, &ids[0], &target, "👍", true, NOW_MS)
            .unwrap(),
        ids[0]
    );
    assert_eq!(a.pending_controls().unwrap(), vec![ids[2].clone()]);
    assert_eq!(a.reaction_states(BOB, &target).unwrap()[0].3, 3);
    let payload = a.encrypt_signal_control(&bob.trusted(), &ids[2]).unwrap();
    receive(&mut b, &alice.trusted(), &ids[2], &payload).unwrap();
    assert_eq!(
        b.reaction_states(ALICE, &target).unwrap(),
        vec![(ALICE.into(), "👍".into(), true, 3)]
    );
    assert_eq!(
        receive(&mut b, &alice.trusted(), &ids[2], &payload).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    a.close().unwrap();
    let mut a = alice.open();
    assert_eq!(
        a.encrypt_signal_control(&bob.trusted(), &ids[2]).unwrap(),
        payload
    );
}

#[test]
fn coalesced_removal_delivers_final_inactive_state_without_an_add() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let add = event_id(610);
    let remove = event_id(611);
    a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
        .unwrap();
    a.queue_reaction(chat, &remove, &target, "👍", false, NOW_MS)
        .unwrap();
    assert_eq!(a.pending_controls().unwrap(), vec![remove.clone()]);
    let payload = a.encrypt_signal_control(&bob.trusted(), &remove).unwrap();
    receive(&mut b, &alice.trusted(), &remove, &payload).unwrap();
    assert_eq!(
        b.reaction_states(ALICE, &target).unwrap(),
        vec![(ALICE.into(), "👍".into(), false, 2)]
    );
    let count: i64 = b
        .connection
        .query_row("SELECT count(*) FROM control_event_inbox", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    let bchat = b.conversations(true).unwrap()[0].id;
    assert!(
        b.timeline_before(bchat, None, 1).unwrap()[0]
            .presentation
            .reactions
            .is_empty()
    );
}

#[test]
fn encrypted_and_uncertain_reactions_are_never_coalesced_or_reencrypted() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let add = event_id(620);
    let remove = event_id(621);
    let latest = event_id(622);
    a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
        .unwrap();
    let p1 = a.encrypt_signal_control(&bob.trusted(), &add).unwrap();
    a.queue_reaction(chat, &remove, &target, "👍", false, NOW_MS + 1)
        .unwrap();
    // Even pending state is immutable once ciphertext exists.
    assert!(!a.control_superseded(&add).unwrap());
    assert_eq!(
        a.pending_controls().unwrap(),
        vec![add.clone(), remove.clone()]
    );
    a.set_control_state(&add, "unconfirmed").unwrap();
    a.queue_reaction(chat, &latest, &target, "👍", true, NOW_MS + 2)
        .unwrap();
    assert_eq!(
        a.pending_controls().unwrap(),
        vec![add.clone(), latest.clone()]
    );
    assert!(a.control_superseded(&remove).unwrap());
    a.close().unwrap();
    let mut a = alice.open();
    let ratchet = session(&a, BOB);
    assert_eq!(a.encrypt_signal_control(&bob.trusted(), &add).unwrap(), p1);
    assert!(session(&a, BOB) == ratchet);
    let p3 = a.encrypt_signal_control(&bob.trusted(), &latest).unwrap();
    receive(&mut b, &alice.trusted(), &latest, &p3).unwrap();
    receive(&mut b, &alice.trusted(), &add, &p1).unwrap();
    assert_eq!(b.reaction_states(ALICE, &target).unwrap()[0].3, 3);
    assert!(b.reaction_states(ALICE, &target).unwrap()[0].2);
    a.set_control_state(&add, "accepted").unwrap();
    assert!(!a.control_superseded(&add).unwrap());
    assert_eq!(a.control(&add).unwrap().payload.unwrap(), p1);
}

#[test]
fn reaction_coalescing_is_scoped_and_failed_writes_do_not_cancel_old_intents() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let other = a.create_conversation("303", NOW_MS).unwrap();
    let (other_message, _) = a.queue_outgoing(other, "Another peer", NOW_MS).unwrap();
    let (second, _) = a.queue_outgoing(chat, "Another message", NOW_MS).unwrap();
    let add = event_id(630);
    let failure = event_id(634);
    a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
        .unwrap();
    a.queue_reaction(chat, &event_id(631), &target, "🔥", true, NOW_MS)
        .unwrap();
    a.queue_reaction(
        chat,
        &event_id(632),
        &reference(ALICE, &second),
        "👍",
        true,
        NOW_MS,
    )
    .unwrap();
    a.queue_reaction(
        other,
        &event_id(633),
        &reference(ALICE, &other_message),
        "👍",
        true,
        NOW_MS,
    )
    .unwrap();
    a.connection.execute_batch("CREATE TEMP TRIGGER reject_reaction BEFORE UPDATE ON message_reactions BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(
        a.queue_reaction(chat, &failure, &target, "👍", false, NOW_MS)
            .is_err()
    );
    assert!(!a.control_superseded(&add).unwrap());
    assert!(a.control(&failure).is_err());
    assert_eq!(a.pending_controls().unwrap().len(), 4);
    a.connection
        .execute_batch("DROP TRIGGER reject_reaction;")
        .unwrap();
    a.queue_reaction(chat, &failure, &target, "👍", false, NOW_MS)
        .unwrap();
    assert!(a.control_superseded(&add).unwrap());
    assert_eq!(a.pending_controls().unwrap().len(), 4);
    for id in [event_id(631), event_id(632), event_id(633)] {
        assert!(!a.control_superseded(&id).unwrap());
        assert!(a.pending_controls().unwrap().contains(&id));
    }
}

#[test]
fn outgoing_control_ciphertext_write_rolls_back_signal_ratchet() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let control = event_id(550);
    a.queue_reaction(chat, &control, &target, "👍", true, NOW_MS)
        .unwrap();
    let state = session(&a, BOB);
    a.connection.execute_batch("CREATE TEMP TRIGGER fail_control BEFORE UPDATE OF payload ON outgoing_controls BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(a.encrypt_signal_control(&bob.trusted(), &control).is_err());
    assert!(state == session(&a, BOB));
    assert!(a.control(&control).unwrap().payload.is_none());
    a.connection
        .execute_batch("DROP TRIGGER fail_control")
        .unwrap();
    let payload = a.encrypt_signal_control(&bob.trusted(), &control).unwrap();
    receive(&mut b, &alice.trusted(), &control, &payload).unwrap();
}

#[test]
fn delete_for_me_hides_history_quotes_and_resends_without_losing_dedup() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let bchat = b.conversations(true).unwrap()[0].id;
    b.delete_for_me(bchat, &target).unwrap();
    assert!(b.timeline_before(bchat, None, 120).unwrap().is_empty());
    assert!(b.reference_cursor(bchat, &target).unwrap().is_none());
    assert!(b.last_message_previews([bchat]).unwrap().is_empty());
    let payload = a.committed_outgoing(&id).unwrap().unwrap().payload;
    assert_eq!(
        receive(&mut b, &alice.trusted(), &id, &payload).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    b.close().unwrap();
    let b = bob.open();
    assert!(b.timeline_before(bchat, None, 120).unwrap().is_empty());
    assert_eq!(b.messages(bchat).unwrap().len(), 1); // Replay evidence is retained.
    a.delete_for_me(chat, &target).unwrap();
    assert!(a.outgoing_message_blocked(&id).unwrap());
    assert!(crate::messaging::send(&mut a, None, &id).is_err());
    assert!(
        a.encrypt_signal_message(&bob.trusted(), chat, &id, "Original", NOW_MS)
            .is_err()
    );
}

#[test]
fn silent_remote_delete_removes_only_target_and_local_delete_stays_local() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let mut ids = Vec::new();
    for (index, text) in ["Hello!", "Remove me", "How are you?"]
        .into_iter()
        .enumerate()
    {
        let (id, _) = a.queue_outgoing(chat, text, NOW_MS + index as i64).unwrap();
        let payload = a
            .encrypt_signal_message(&bob.trusted(), chat, &id, text, NOW_MS + index as i64)
            .unwrap();
        receive(&mut b, &alice.trusted(), &id, &payload).unwrap();
        ids.push(id);
    }
    let bchat = b.conversations(true).unwrap()[0].id;
    assert_eq!(a.timeline_before(chat, None, 120).unwrap().len(), 3);
    assert_eq!(b.timeline_before(bchat, None, 120).unwrap().len(), 3);
    let delete = event_id(590);
    a.queue_delete_everyone(chat, &delete, &reference(ALICE, &ids[1]), NOW_MS + 3)
        .unwrap();
    let payload = a.encrypt_signal_control(&bob.trusted(), &delete).unwrap();
    // Recipient restarts before the pending encrypted deletion is delivered.
    b.close().unwrap();
    let mut b = bob.open();
    receive(&mut b, &alice.trusted(), &delete, &payload).unwrap();
    let ratchet = session(&b, ALICE);
    assert_eq!(
        receive(&mut b, &alice.trusted(), &delete, &payload).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(session(&b, ALICE) == ratchet);
    for (db, conversation) in [(&a, chat), (&b, bchat)] {
        let visible = db.timeline_before(conversation, None, 120).unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|row| row.body.as_str())
                .collect::<Vec<_>>(),
            ["Hello!", "How are you?"]
        );
        assert!(
            db.reference_cursor(conversation, &reference(ALICE, &ids[1]))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.last_message_previews([conversation]).unwrap()[&conversation].body,
            "How are you?"
        );
    }
    let controls = a.pending_controls().unwrap();
    a.delete_for_me(chat, &reference(ALICE, &ids[0])).unwrap();
    assert_eq!(a.pending_controls().unwrap(), controls);
    assert_eq!(a.timeline_before(chat, None, 120).unwrap().len(), 1);
    assert_eq!(b.timeline_before(bchat, None, 120).unwrap().len(), 2);
    a.close().unwrap();
    b.close().unwrap();
    let a = alice.open();
    let b = bob.open();
    assert_eq!(a.timeline_before(chat, None, 120).unwrap().len(), 1);
    assert_eq!(b.timeline_before(bchat, None, 120).unwrap().len(), 2);
    assert!(
        b.message_deleted(ALICE, &reference(ALICE, &ids[1]))
            .unwrap()
    );
}

#[test]
fn delete_everyone_tombstone_intent_restart_and_late_target() {
    let (alice, bob, mut a, mut b, chat) = pair();
    // Prepare original but leave it offline; deletion must survive arriving first.
    let (id, _) = a.queue_outgoing(chat, "Offline original", NOW_MS).unwrap();
    let target = reference(ALICE, &id);
    let original = a
        .encrypt_signal_message(&bob.trusted(), chat, &id, "Offline original", NOW_MS)
        .unwrap();
    let deletion = event_id(560);
    a.queue_delete_everyone(chat, &deletion, &target, NOW_MS)
        .unwrap();
    assert_eq!(
        a.queue_delete_everyone(chat, &event_id(561), &target, NOW_MS)
            .unwrap(),
        deletion
    );
    assert!(a.timeline_before(chat, None, 1).unwrap().is_empty());
    assert!(a.last_message_previews([chat]).unwrap().is_empty());
    assert!(a.outgoing_message_blocked(&id).unwrap());
    assert!(crate::messaging::send(&mut a, None, &id).is_err());
    a.close().unwrap();
    let mut a = alice.open();
    assert!(a.pending_controls().unwrap().contains(&deletion));
    let payload = a.encrypt_signal_control(&bob.trusted(), &deletion).unwrap();
    receive(&mut b, &alice.trusted(), &deletion, &payload).unwrap();
    receive(&mut b, &alice.trusted(), &id, &original).unwrap();
    let bchat = b.conversations(true).unwrap()[0].id;
    assert_eq!(b.messages(bchat).unwrap()[0].body, "");
    assert!(b.timeline_before(bchat, None, 120).unwrap().is_empty());
    assert!(b.last_message_previews([bchat]).unwrap().is_empty());
    assert!(
        b.queue_delete_everyone(bchat, &event_id(562), &target, NOW_MS)
            .is_err()
    );
    assert!(
        a.queue_reaction(chat, &event_id(563), &target, "👍", true, NOW_MS)
            .is_err()
    );
    b.close().unwrap();
    let b = bob.open();
    assert!(b.message_deleted(ALICE, &target).unwrap());
}

#[test]
fn both_participants_reply_react_and_delete_only_their_own_messages() {
    let (alice, bob, mut a, mut b, chat) = pair();
    let id = original(&mut a, &mut b, &alice, &bob, chat);
    let target = reference(ALICE, &id);
    let bchat = b.conversations(true).unwrap()[0].id;
    b.save_reply_draft(bchat, "Reply to received message", Some(&target))
        .unwrap();
    let (reply, promoted) = b
        .queue_outgoing(bchat, "Reply to received message", NOW_MS + 1)
        .unwrap();
    assert!(promoted);
    let payload = b
        .encrypt_signal_message(
            &alice.trusted(),
            bchat,
            &reply,
            "Reply to received message",
            NOW_MS + 1,
        )
        .unwrap();
    receive(&mut a, &bob.trusted(), &reply, &payload).unwrap();
    assert_eq!(
        a.timeline_before(chat, None, 1).unwrap()[0]
            .presentation
            .quote,
        "Original"
    );
    let aid = event_id(580);
    let bid = event_id(581);
    a.queue_reaction(chat, &aid, &target, "👍", true, NOW_MS)
        .unwrap();
    b.queue_reaction(bchat, &bid, &target, "👍", true, NOW_MS)
        .unwrap();
    let payload = a.encrypt_signal_control(&bob.trusted(), &aid).unwrap();
    receive(&mut b, &alice.trusted(), &aid, &payload).unwrap();
    let payload = b.encrypt_signal_control(&alice.trusted(), &bid).unwrap();
    receive(&mut a, &bob.trusted(), &bid, &payload).unwrap();
    for (db, conversation) in [(&a, chat), (&b, bchat)] {
        assert_eq!(
            db.timeline_before(conversation, None, 2).unwrap()[0]
                .presentation
                .reactions,
            vec![("👍".into(), 2, true)]
        );
    }
    let deletion = event_id(582);
    b.queue_delete_everyone(bchat, &deletion, &reference(BOB, &reply), NOW_MS)
        .unwrap();
    let payload = b
        .encrypt_signal_control(&alice.trusted(), &deletion)
        .unwrap();
    receive(&mut a, &bob.trusted(), &deletion, &payload).unwrap();
    assert!(a.message_deleted(BOB, &reference(BOB, &reply)).unwrap());
    assert!(!a.message_deleted(BOB, &target).unwrap());
    assert!(
        b.queue_delete_everyone(bchat, &event_id(583), &target, NOW_MS)
            .is_err()
    );
}

#[test]
fn delete_transaction_failure_preserves_original_and_drafts() {
    let (_, _, mut a, _, chat) = pair();
    let (id, _) = a.queue_outgoing(chat, "Not deleted", NOW_MS).unwrap();
    let target = reference(ALICE, &id);
    a.save_reply_draft(chat, "Draft", Some(&target)).unwrap();
    a.connection.execute_batch("CREATE TEMP TRIGGER fail_delete BEFORE INSERT ON message_tombstones BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(
        a.queue_delete_everyone(chat, &event_id(570), &target, NOW_MS)
            .is_err()
    );
    assert!(a.pending_controls().unwrap().is_empty());
    assert!(!a.outgoing_message_blocked(&id).unwrap());
    assert_eq!(a.outgoing_operation(&id).unwrap().body, "Not deleted");
    assert_eq!(a.reply_draft(chat).unwrap(), Some(target));
    assert_eq!(a.draft(chat).unwrap().as_deref(), Some("Draft"));
}
