use super::*;
use crate::storage::{
    DeliveryState, Direction, Message, Owner,
    events::{Incoming, IncomingOutcome},
    tests::TestDir,
};
use zeroize::Zeroizing;

#[test]
fn cross_peer_controls_cannot_reference_a_legacy_outbox_message_row() {
    let root = TestDir::new();
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "101",
            public_key: &[5; 32],
        },
    )
    .unwrap();
    let charlie = db.create_conversation("303", 10).unwrap();
    db.save_draft(charlie, "preserve this draft").unwrap();
    let original = "00000000-0000-4000-8000-000000000001";
    let control = "00000000-0000-4000-8000-000000000002";
    db.commit_outgoing(charlie, original, false, |_| {
        Ok((
            Message {
                id: "historical-opaque-row-id".into(),
                conversation_id: charlie,
                direction: Direction::Outgoing,
                body: "history for Charlie".into(),
                created_at: 11,
                delivery_state: DeliveryState::Pending,
            },
            vec![1, 2, 3],
        ))
    })
    .unwrap();
    let target = MessageReference {
        author_void_id: "101".into(),
        event_id: original.into(),
    };
    let reaction = ReactionEvent {
        target: target.clone(),
        emoji: "👍".into(),
        active: true,
        revision: 1,
    };

    // The stable reference resolves through outbox, despite the arbitrary old
    // message row ID. Bob cannot address Charlie's conversation or commit any
    // state changes made in the same transaction as that invalid control.
    let result = db.commit_signal_incoming("202", control, &[9; 32], |tx| {
        tx.execute(
            "UPDATE drafts SET body = 'must roll back' WHERE conversation_id = ?1",
            [charlie],
        )?;
        Ok(Incoming::Reaction {
            event: reaction.clone(),
            plaintext: Zeroizing::new(b"isolated control fixture".to_vec()),
            min_protocol_version: ApplicationProtocolVersion::V2,
            received_at: 12,
        })
    });
    assert!(matches!(result, Err(Error::SignalEnvelope)));
    assert_eq!(
        db.draft(charlie).unwrap().as_deref(),
        Some("preserve this draft")
    );
    assert!(db.reaction_states("202", &target).unwrap().is_empty());
    assert_eq!(
        db.connection
            .query_row("SELECT count(*) FROM control_event_inbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.connection
            .query_row("SELECT count(*) FROM conversations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );

    // The actual peer may react to the same historical row normally.
    assert_eq!(
        db.commit_signal_incoming("303", control, &[10; 32], |_| Ok(Incoming::Reaction {
            event: reaction,
            plaintext: Zeroizing::new(b"isolated control fixture".to_vec()),
            min_protocol_version: ApplicationProtocolVersion::V2,
            received_at: 12,
        }))
        .unwrap(),
        IncomingOutcome::Stored
    );
    assert_eq!(
        db.reaction_states("303", &target).unwrap(),
        vec![("303".into(), "👍".into(), true, 1)]
    );
    let messages = db.messages(charlie).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "historical-opaque-row-id");
    assert_eq!(messages[0].body, "history for Charlie");
}

#[test]
fn queued_originals_are_scoped_before_signal_ciphertext_is_created() {
    let root = TestDir::new();
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "101",
            public_key: &[5; 32],
        },
    )
    .unwrap();
    let charlie = db.create_conversation("303", 10).unwrap();
    let (original, _) = db
        .queue_outgoing(charlie, "Queued for Charlie", 11)
        .unwrap();
    let target = MessageReference {
        author_void_id: "101".into(),
        event_id: original.clone(),
    };
    let control = "00000000-0000-4000-8000-000000000002";
    let reaction = ReactionEvent {
        target: target.clone(),
        emoji: "👍".into(),
        active: true,
        revision: 1,
    };
    assert!(matches!(
        db.commit_signal_incoming("202", control, &[9; 32], |_| Ok(Incoming::Reaction {
            event: reaction.clone(),
            plaintext: Zeroizing::new(vec![1]),
            min_protocol_version: ApplicationProtocolVersion::V2,
            received_at: 12,
        })),
        Err(Error::SignalEnvelope)
    ));
    assert!(db.reaction_states("202", &target).unwrap().is_empty());
    assert!(db.committed_outgoing(&original).unwrap().is_none());
    assert_eq!(
        db.outgoing_operation(&original).unwrap().body,
        "Queued for Charlie"
    );
    assert_eq!(
        db.commit_signal_incoming("303", control, &[10; 32], |_| Ok(Incoming::Reaction {
            event: reaction,
            plaintext: Zeroizing::new(vec![1]),
            min_protocol_version: ApplicationProtocolVersion::V2,
            received_at: 12,
        }))
        .unwrap(),
        IncomingOutcome::Stored
    );
    assert_eq!(db.reaction_states("303", &target).unwrap().len(), 1);
}
