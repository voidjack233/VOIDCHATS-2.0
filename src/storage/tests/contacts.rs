use super::*;

#[test]
fn contacts_require_both_directions_and_reply_promotion_is_atomic() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    db.save_draft(chat, "Not a message").unwrap();
    assert!(db.contacts().unwrap().is_empty());
    db.set_request(chat, true).unwrap();
    db.commit_incoming("123", "incoming", &[8; 32], |_| Ok(sample_message(chat)))
        .unwrap();
    db.mark_read(chat, i64::MAX).unwrap();
    assert!(db.contacts().unwrap().is_empty());
    assert!(db.conversation(chat).unwrap().is_request);
    db.connection.execute_batch("CREATE TEMP TRIGGER reject_contact BEFORE INSERT ON contacts BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(db.queue_outgoing(chat, "First reply", 200).is_err());
    assert!(db.conversation(chat).unwrap().is_request);
    assert_eq!(db.conversation_timeline(chat).unwrap().len(), 1);
    assert!(db.contacts().unwrap().is_empty());
    db.connection
        .execute_batch("DROP TRIGGER reject_contact;")
        .unwrap();
    assert!(db.queue_outgoing(chat, "First reply", 200).unwrap().1);
    assert_eq!(db.contacts().unwrap().len(), 1);
    assert!(!db.conversation(chat).unwrap().is_request);
    db.queue_outgoing(chat, "Another reply", 201).unwrap();
    assert_eq!(db.contacts().unwrap().len(), 1);
    let one_way = db.create_conversation("456", 300).unwrap();
    db.queue_outgoing(one_way, "No response yet", 300).unwrap();
    assert!(db.contact("456").unwrap().is_none());
    db.close().unwrap();
    let db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(db.contacts().unwrap().len(), 1);
    assert!(db.contact("456").unwrap().is_none());
    assert_eq!(db.draft(chat).unwrap().as_deref(), Some("Not a message"));
}

#[test]
fn contact_recovery_names_and_independence_preserve_existing_data() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    db.update_conversation(chat, Some("Old conversation name"), 100)
        .unwrap();
    db.commit_incoming("123", "incoming", &[8; 32], |_| Ok(sample_message(chat)))
        .unwrap();
    db.queue_outgoing(chat, "Reply", 200).unwrap();
    // A prior build's established chat has no contact row yet.
    db.connection.execute("DELETE FROM contacts", []).unwrap();
    db.set_request(chat, true).unwrap();
    let version: i64 = db
        .connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    db.close().unwrap();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(db.contacts().unwrap().len(), 1);
    assert!(!db.conversation(chat).unwrap().is_request);
    assert_eq!(
        db.connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        version
    );
    let identity = vec![7; 32];
    db.save_contact(&Contact {
        void_id: "123".into(),
        display_name: Some("Original contact".into()),
        public_identity: Some(identity.clone()),
    })
    .unwrap();
    db.save_draft(chat, "Keep draft").unwrap();
    assert!(db.rename_contact("123", "  Maya Chen  ").unwrap());
    db.update_conversation(chat, Some("Stale alias"), 200)
        .unwrap();
    assert_eq!(
        db.conversation(chat).unwrap().display_name.as_deref(),
        Some("Maya Chen")
    );
    assert_eq!(
        db.conversations(false).unwrap()[0].display_name.as_deref(),
        Some("Maya Chen")
    );
    assert_eq!(
        db.contact("123").unwrap().unwrap().public_identity,
        Some(identity)
    );
    assert!(!db.rename_contact("999", "Must not add contact").unwrap());
    for invalid in ["bad\nname".to_owned(), "x".repeat(101)] {
        assert!(matches!(
            db.rename_contact("123", &invalid),
            Err(Error::InvalidContactName)
        ));
    }
    db.connection.execute_batch("CREATE TEMP TRIGGER reject_rename BEFORE UPDATE OF display_name ON conversations BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(db.rename_contact("123", "Should roll back").is_err());
    assert_eq!(
        db.contact("123").unwrap().unwrap().display_name.as_deref(),
        Some("Maya Chen")
    );
    db.connection
        .execute_batch("DROP TRIGGER reject_rename;")
        .unwrap();
    db.close().unwrap();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(
        db.contact("123").unwrap().unwrap().display_name.as_deref(),
        Some("Maya Chen")
    );
    assert_eq!(db.draft(chat).unwrap().as_deref(), Some("Keep draft"));
    assert_eq!(db.conversation_timeline(chat).unwrap().len(), 2);
    db.rename_contact("123", "   ").unwrap();
    assert!(db.conversation(chat).unwrap().display_name.is_none());
    // Contact ownership does not cascade with conversation ownership.
    db.connection
        .execute(
            "DELETE FROM outgoing_operations WHERE conversation_id=?1",
            [chat],
        )
        .unwrap();
    db.connection.execute("DELETE FROM inbox", []).unwrap();
    db.connection
        .execute("DELETE FROM conversations WHERE id=?1", [chat])
        .unwrap();
    assert!(db.contact("123").unwrap().is_some());
    assert!(db.rename_contact("123", "Still saved").unwrap());
    db.close().unwrap();
    assert_eq!(
        Database::open(&root.0, OWNER)
            .unwrap()
            .contacts()
            .unwrap()
            .len(),
        1
    );
}
