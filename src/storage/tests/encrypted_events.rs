use super::*;
use rusqlite::types::Value;

// Compare the complete legacy rows without printing isolated private key bytes
// in assertion output if a migration regression occurs.
fn legacy_rows(connection: &Connection) -> Vec<Vec<Vec<Value>>> {
    [
        "local_account",
        "conversations",
        "messages",
        "contacts",
        "drafts",
        "requests",
        "outbox",
        "inbox",
        "receipt_outbox",
        "receipt_inbox",
        "outgoing_operations",
        "signal_local_identity",
        "signal_prekeys",
        "signal_sessions",
        "signal_identities",
        "signal_kyber_uses",
        "signal_peer_bindings",
        "signal_publication",
        "unread_messages",
        "peer_account_status",
    ]
    .into_iter()
    .map(|table| {
        let mut query = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = query.column_count();
        query
            .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    })
    .collect()
}

#[test]
fn schema_nine_history_receipts_and_signal_material_survive_receiver_migration_and_reopen() {
    let root = TestDir::new();
    let account = ed25519_dalek::SigningKey::from_bytes(&[117; 32]);
    let public = account.verifying_key();
    let owner = Owner {
        void_id: "41",
        public_key: public.as_bytes(),
    };
    fs::create_dir(root.0.join("messages")).unwrap();
    let key = create_key(&root.key()).unwrap();
    let protected_key = fs::read(root.key()).unwrap();
    let mut connection = open_keyed(&root.db(), &key, writable_flags(true)).unwrap();
    migrate_with_scripts(&mut connection, owner, true, &MIGRATIONS[..9]).unwrap();
    let mut legacy = Database {
        connection,
        _lock: File::create(root.0.join("messages.lock")).unwrap(),
    };
    legacy.initialize_signal(owner.void_id, &account).unwrap();
    let conversation = legacy.create_conversation("123", 100).unwrap();
    let request = legacy.create_conversation("456", 100).unwrap();
    legacy.set_request(request, true).unwrap();
    legacy
        .save_draft(conversation, "Unsent draft\nwith paragraphs")
        .unwrap();
    let original_event = "00000000-0000-0000-0000-000000000001";
    let original_id = format!("123:{original_event}");
    legacy
        .save_message(&Message {
            id: original_id.clone(),
            conversation_id: conversation,
            direction: Direction::Incoming,
            body: "Historical incoming message".into(),
            created_at: 120,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
    legacy.connection.execute(
        "INSERT INTO inbox (sender_void_id, event_id, payload_digest, message_id) VALUES ('123', ?1, ?2, ?3)",
        params![original_event, [7u8; 32].as_slice(), original_id],
    ).unwrap();
    legacy.connection.execute(
        "INSERT INTO receipt_outbox (event_id, peer_void_id, original_event_id, payload, attempts, next_attempt_at, deadline_at)
         VALUES ('00000000-0000-0000-0000-000000000002', '123', ?1, ?2, 1, 1000, 2000)",
        params![original_event, b"Saved encrypted receipt".as_slice()],
    ).unwrap();
    let (outgoing_event, _) = legacy
        .queue_outgoing(conversation, "Historical outgoing message", 130)
        .unwrap();
    legacy
        .commit_outgoing(conversation, &outgoing_event, true, |_| {
            Ok((
                Message {
                    id: format!("41:{outgoing_event}"),
                    conversation_id: conversation,
                    direction: Direction::Outgoing,
                    body: "Historical outgoing message".into(),
                    created_at: 130,
                    delivery_state: DeliveryState::Delivered,
                },
                b"Saved encrypted outgoing payload".to_vec(),
            ))
        })
        .unwrap();
    legacy.connection.execute(
        "INSERT INTO receipt_inbox (sender_void_id, event_id, payload_digest, original_event_id)
         VALUES ('123', '00000000-0000-0000-0000-000000000003', ?1, ?2)",
        params![[8u8; 32].as_slice(), outgoing_event],
    ).unwrap();
    let session = libsignal_protocol::SessionRecord::new_fresh()
        .serialize()
        .unwrap();
    legacy
        .connection
        .execute(
            "INSERT INTO signal_sessions VALUES ('123', 1, ?1)",
            [session],
        )
        .unwrap();
    let rows = legacy_rows(&legacy.connection);
    let cursor_keys: Vec<String> = legacy
        .conversation_timeline(conversation)
        .unwrap()
        .iter()
        .map(|message| message.cursor.key())
        .collect();
    legacy.close().unwrap();

    for _ in 0..2 {
        let mut db = Database::open(&root.0, owner).unwrap();
        assert_eq!(
            schema_version(&db.connection, MIGRATIONS.len()).unwrap(),
            MIGRATIONS.len()
        );
        assert!(
            legacy_rows(&db.connection) == rows,
            "receiver migration changed existing rows"
        );
        assert!(
            fs::read(root.key()).unwrap() == protected_key,
            "migration replaced protected storage key"
        );
        assert_eq!(
            db.conversation_timeline(conversation)
                .unwrap()
                .iter()
                .map(|message| message.cursor.key())
                .collect::<Vec<_>>(),
            cursor_keys
        );
        assert!(db.has_verified_delivery(&outgoing_event).unwrap());
        for table in [
            "message_reply_refs",
            "message_reactions",
            "message_tombstones",
            "control_event_inbox",
            "deferred_signal_events",
        ] {
            let count: i64 = db
                .connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "new receiver table {table} must start empty");
        }
        db.initialize_signal(owner.void_id, &account).unwrap();
        assert!(
            legacy_rows(&db.connection) == rows,
            "reopen regenerated Signal material"
        );
        db.close().unwrap();
    }
    assert_eq!(
        check_existing(&root.0, owner).unwrap(),
        IntegrityReport {
            cipher: true,
            sqlite: true,
            foreign_keys: true
        }
    );
    let encrypted = fs::read(root.db()).unwrap();
    assert!(!encrypted.starts_with(b"SQLite format 3\0"));
    assert!(
        !encrypted
            .windows(b"Historical incoming message".len())
            .any(|bytes| bytes == b"Historical incoming message")
    );
}

#[test]
fn failed_receiver_migration_rolls_back_new_tables_and_preserves_schema_nine_history() {
    let root = TestDir::new();
    fs::create_dir(root.0.join("messages")).unwrap();
    let key = create_key(&root.key()).unwrap();
    let protected_key = fs::read(root.key()).unwrap();
    let mut connection = open_keyed(&root.db(), &key, writable_flags(true)).unwrap();
    migrate_with_scripts(&mut connection, OWNER, true, &MIGRATIONS[..9]).unwrap();
    connection
        .execute_batch(
            "INSERT INTO conversations VALUES (1, '123', NULL, 100, 120);
        INSERT INTO messages VALUES ('old-row', 1, 0, 'Retained after failed migration', 120, 0);
        INSERT INTO drafts VALUES (1, 'Retained draft');",
        )
        .unwrap();
    let before = legacy_rows(&connection);
    let invalid = format!("{}\nINVALID SQL;", MIGRATIONS[9]);
    let mut scripts = MIGRATIONS[..9].to_vec();
    scripts.push(&invalid);
    assert!(migrate_with_scripts(&mut connection, OWNER, false, &scripts).is_err());
    assert_eq!(schema_version(&connection, MIGRATIONS.len()).unwrap(), 9);
    assert!(
        legacy_rows(&connection) == before,
        "failed migration changed existing rows"
    );
    let new_tables: i64 = connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type = 'table'
        AND name IN ('message_reply_refs', 'message_reactions', 'message_tombstones', 'control_event_inbox', 'deferred_signal_events')", [], |row| row.get(0)).unwrap();
    assert_eq!(new_tables, 0);
    connection.close().unwrap();
    let connection = open_keyed(&root.db(), &key, writable_flags(false)).unwrap();
    assert_eq!(schema_version(&connection, MIGRATIONS.len()).unwrap(), 9);
    assert!(legacy_rows(&connection) == before);
    connection.close().unwrap();
    let db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(
        schema_version(&db.connection, MIGRATIONS.len()).unwrap(),
        MIGRATIONS.len()
    );
    assert!(legacy_rows(&db.connection) == before);
    assert!(fs::read(root.key()).unwrap() == protected_key);
    db.close().unwrap();
}
