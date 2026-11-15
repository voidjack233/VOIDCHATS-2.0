use super::*;

const PUBLIC_KEY: [u8; 32] = [5; 32];
const OWNER: Owner<'_> = Owner { void_id: "41", public_key: &PUBLIC_KEY };
const BODY: &str = "Local storage round-trip marker 8e36912d";

pub(crate) struct TestDir(pub std::path::PathBuf);

impl TestDir {
    pub(crate) fn new() -> Self {
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!("void-storage-test-{:032x}", u128::from_ne_bytes(nonce)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn db(&self) -> std::path::PathBuf { self.0.join("messages").join(DATABASE) }
    fn key(&self) -> std::path::PathBuf { self.0.join("messages").join(KEY_FILE) }
}

impl Drop for TestDir {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn sample_message(conversation_id: i64) -> Message {
    Message {
        id: "message-1".into(), conversation_id, direction: Direction::Incoming,
        body: BODY.into(), created_at: 120, delivery_state: DeliveryState::Received,
    }
}

#[test]
fn encrypted_records_survive_reopen_and_require_the_same_key_and_owner() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(db.connection.pragma_query_value(None, "cipher_status", |r| r.get::<_, String>(0)).unwrap(), "1");
    let provider: String = db.connection.pragma_query_value(None, "cipher_provider", |r| r.get(0)).unwrap();
    assert!(provider.to_ascii_lowercase().contains("openssl"));

    let id = db.create_conversation("123", 100).unwrap();
    assert_eq!(db.create_conversation("123", 110).unwrap(), id);
    db.save_message(&sample_message(id)).unwrap();
    assert!(db.update_delivery("message-1", DeliveryState::Read).unwrap());
    db.save_contact(&Contact { void_id: "123".into(), display_name: Some("Test peer".into()),
        public_identity: Some(vec![3, 7]) }).unwrap();
    db.save_draft(id, "draft to retain\nsecond line").unwrap();
    db.set_request(id, true).unwrap();
    let before = read_key(&root.key()).unwrap();
    assert!(matches!(Database::open(&root.0, OWNER), Err(Error::Busy)));
    db.close().unwrap();

    let encrypted = fs::read(root.db()).unwrap();
    assert!(!encrypted.starts_with(b"SQLite format 3\0"));
    assert!(!encrypted.windows(BODY.len()).any(|window| window == BODY.as_bytes()));
    let raw = Connection::open_with_flags(root.db(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(raw.pragma_query_value(None, "cipher_status", |r| r.get::<_, String>(0)).unwrap(), "0");
    let unkeyed = raw.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0));
    assert!(unkeyed.is_err());
    raw.close().unwrap();
    let mut wrong = before.clone();
    wrong[0] ^= 1;
    assert!(matches!(open_keyed(&root.db(), &wrong, writable_flags(false)).and_then(|mut db| migrate(&mut db, OWNER, false)),
        Err(Error::Unreadable)));
    assert!(matches!(Database::open(&root.0, Owner { void_id: "42", ..OWNER }), Err(Error::WrongAccount)));
    assert!(matches!(Database::open(&root.0, Owner { public_key: &[9; 32], ..OWNER }), Err(Error::WrongAccount)));
    assert!(before.as_ref() == read_key(&root.key()).unwrap().as_ref());

    let db = Database::open(&root.0, OWNER).unwrap();
    assert!(db.conversations(false).unwrap().is_empty());
    let requests = db.conversations(true).unwrap();
    assert_eq!(requests[0].display_name.as_deref(), Some("Test peer"));
    assert_eq!(requests[0].created_at, 100);
    assert_eq!(requests[0].last_activity_at, 120);
    let messages = db.messages(id).unwrap();
    assert_eq!(messages[0].body, BODY);
    assert_eq!(messages[0].direction, Direction::Incoming);
    assert_eq!(messages[0].delivery_state, DeliveryState::Read);
    assert_eq!(db.contacts().unwrap()[0].public_identity, Some(vec![3, 7]));
    assert_eq!(db.draft(id).unwrap().as_deref(), Some("draft to retain\nsecond line"));
    assert!(db.update_conversation(id, Some("Updated name"), 140).unwrap());
    db.set_request(id, false).unwrap();
    assert_eq!(db.conversations(false).unwrap()[0].last_activity_at, 140);
    db.save_draft(id, "").unwrap();
    assert!(db.draft(id).unwrap().is_none());
}

#[test]
fn missing_or_unreadable_dpapi_key_never_creates_replacement_storage() {
    let root = TestDir::new();
    let db = Database::open(&root.0, OWNER).unwrap();
    let id = db.create_conversation("123", 100).unwrap();
    db.save_draft(id, BODY).unwrap();
    db.close().unwrap();
    let original_db = fs::read(root.db()).unwrap();
    let original_key = fs::read(root.key()).unwrap();
    fs::remove_file(root.key()).unwrap();
    assert!(matches!(Database::open(&root.0, OWNER), Err(Error::KeyUnavailable)));
    assert!(!root.key().exists());
    assert!(!root.0.join("messages.initializing").exists());
    fs::write(root.key(), b"damaged DPAPI record").unwrap();
    assert!(matches!(Database::open(&root.0, OWNER), Err(Error::KeyUnavailable)));
    assert_eq!(fs::read(root.key()).unwrap(), b"damaged DPAPI record");
    assert!(fs::read(root.db()).unwrap() == original_db);

    fs::write(root.key(), original_key).unwrap();
    assert_eq!(Database::open(&root.0, OWNER).unwrap().draft(id).unwrap().as_deref(), Some(BODY));
}

#[test]
fn migrations_and_message_transactions_roll_back_and_enforce_foreign_keys() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let version = |connection: &Connection| connection.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(version(&db.connection), 4);
    assert!(db.save_draft(999, "missing conversation").is_err());
    assert!(db.set_request(999, true).is_err());
    assert!(db.save_message(&sample_message(999)).is_err());
    let id = db.create_conversation("123", 100).unwrap();
    db.connection.execute_batch("CREATE TEMP TRIGGER reject_activity BEFORE UPDATE ON conversations
        BEGIN SELECT RAISE(ABORT, 'test transaction rollback'); END;").unwrap();
    assert!(db.save_message(&sample_message(id)).is_err());
    assert!(db.messages(id).unwrap().is_empty());
    assert_eq!(db.conversation(id).unwrap().last_activity_at, 100);
    db.connection.execute_batch("DROP TRIGGER reject_activity;").unwrap();
    db.save_message(&sample_message(id)).unwrap();
    db.connection.pragma_update(None, "user_version", 50).unwrap();
    db.close().unwrap();
    assert!(matches!(Database::open(&root.0, OWNER), Err(Error::UnsupportedSchema)));

    let mut failed_migration = Connection::open_in_memory().unwrap();
    // A failing second migration must leave both the schema and version untouched.
    failed_migration.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    assert!(migrate_with_scripts(&mut failed_migration, OWNER, true,
        &[MIGRATIONS[0], "CREATE TABLE extra (id INTEGER); INVALID SQL;"]).is_err());
    assert_eq!(version(&failed_migration), 0);
    assert_eq!(failed_migration.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn completed_staging_database_resumes_without_regenerating_its_key() {
    let root = TestDir::new();
    let staging = root.0.join("messages.initializing");
    fs::create_dir(&staging).unwrap();
    let key = create_key(&staging.join(KEY_FILE)).unwrap();
    let mut connection = open_keyed(&staging.join(DATABASE), &key, writable_flags(true)).unwrap();
    migrate(&mut connection, OWNER, true).unwrap();
    connection.close().unwrap();
    let before = fs::read(staging.join(KEY_FILE)).unwrap();
    let db = Database::open(&root.0, OWNER).unwrap();
    assert!(!staging.exists());
    assert!(before == fs::read(root.key()).unwrap());
    assert!(db.conversations(false).unwrap().is_empty());
}

#[test]
fn routine_open_skips_damage_on_unread_pages_and_explicit_check_reports_it() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let id = db.create_conversation("123", 100).unwrap();
    db.save_message(&sample_message(id)).unwrap();
    let page: usize = db.connection.query_row(
        "SELECT rootpage FROM sqlite_schema WHERE name = 'messages'", [], |row| row.get::<_, u32>(0),
    ).unwrap() as usize;
    let size: usize = db.connection.pragma_query_value(None, "cipher_page_size", |row| row.get::<_, String>(0))
        .unwrap().parse().unwrap();
    db.close().unwrap();
    assert_eq!(check_existing(&root.0, OWNER).unwrap(), IntegrityReport { cipher: true, sqlite: true, foreign_keys: true });

    let mut damaged = fs::read(root.db()).unwrap();
    damaged[(page - 1) * size + 64] ^= 0x80;
    fs::write(root.db(), &damaged).unwrap();
    let db = Database::open(&root.0, OWNER).unwrap();
    assert!(db.messages(id).is_err());
    db.close().unwrap();
    let report = check_existing(&root.0, OWNER).unwrap();
    assert!(!report.cipher && !report.sqlite);
    assert!(fs::read(root.db()).unwrap() == damaged);
}

#[test]
fn existing_v1_history_survives_current_migrations_and_fk_diagnostics_detect_orphans() {
    let root = TestDir::new();
    assert!(check_existing(&root.0, OWNER).is_err());
    assert!(!root.0.join("messages").exists());
    fs::create_dir(root.0.join("messages")).unwrap();
    let key = create_key(&root.key()).unwrap();
    let mut connection = open_keyed(&root.db(), &key, writable_flags(true)).unwrap();
    migrate_with_scripts(&mut connection, OWNER, true, &MIGRATIONS[..1]).unwrap();
    connection.execute_batch(
        "INSERT INTO conversations VALUES (1, '123', NULL, 100, 120);
         INSERT INTO messages VALUES ('legacy', 1, 0, 'Existing body', 120, 0);
         INSERT INTO drafts VALUES (1, 'Existing draft');",
    ).unwrap();
    connection.close().unwrap();
    assert!(check_existing(&root.0, OWNER).unwrap().sqlite);
    let mut connection = open_keyed(&root.db(), &key, writable_flags(false)).unwrap();
    assert_eq!(schema_version(&connection, 2).unwrap(), 1);
    assert!(migrate_with_scripts(&mut connection, OWNER, false, &[MIGRATIONS[0], "INVALID SQL;"]).is_err());
    assert_eq!(schema_version(&connection, 2).unwrap(), 1);
    connection.close().unwrap();

    let db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(schema_version(&db.connection, 4).unwrap(), 4);
    assert_eq!(db.messages(1).unwrap()[0].body, "Existing body");
    assert_eq!(db.draft(1).unwrap().as_deref(), Some("Existing draft"));
    db.connection.pragma_update(None, "foreign_keys", false).unwrap();
    db.connection.execute("INSERT INTO drafts VALUES (999, 'Orphan draft')", []).unwrap();
    db.close().unwrap();
    assert_eq!(check_existing(&root.0, OWNER).unwrap(), IntegrityReport { cipher: true, sqlite: true, foreign_keys: false });
}

#[test]
fn event_writes_commit_together_and_retries_skip_state_changes() {
    use super::events::IncomingOutcome;

    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let id = db.create_conversation("123", 100).unwrap();
    db.connection.execute_batch(
        "CREATE TABLE test_state (slot INTEGER PRIMARY KEY, opaque BLOB NOT NULL);
         CREATE TABLE test_commit_guard (id INTEGER REFERENCES conversations(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TEMP TRIGGER reject_inbox BEFORE INSERT ON inbox BEGIN SELECT RAISE(ABORT, 'test'); END;",
    ).unwrap();
    let fake_state = |tx: &rusqlite::Transaction<'_>, slot| -> Result<()> {
        tx.execute("INSERT INTO test_state VALUES (?1, ?2)", params![slot, &[7u8, 8][..]])?;
        Ok(())
    };
    let outbound = || Message { id: "out-message".into(), direction: Direction::Outgoing, ..sample_message(id) };
    let inbound = || sample_message(id);
    let count = |connection: &Connection, sql| connection.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();

    assert!(db.commit_outgoing(id, "out-event", |tx| {
        fake_state(tx, 1)?;
        Ok((outbound(), vec![])) // outbox rejects the empty payload after the message insert
    }).is_err());
    assert!(db.commit_incoming("123", "in-event", &[9; 32], |tx| {
        fake_state(tx, 2)?;
        Ok(inbound())
    }).is_err());
    assert_eq!(count(&db.connection, "SELECT count(*) FROM test_state"), 0);
    assert_eq!(count(&db.connection, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&db.connection, "SELECT count(*) FROM outbox"), 0);
    assert_eq!(count(&db.connection, "SELECT count(*) FROM inbox"), 0);
    assert_eq!(db.conversation(id).unwrap().last_activity_at, 100);

    db.connection.execute_batch("DROP TRIGGER reject_inbox").unwrap();
    assert!(db.commit_incoming("123", "in-event", &[9; 32], |tx| {
        fake_state(tx, 2)?;
        tx.execute("INSERT INTO test_commit_guard VALUES (999)", [])?;
        Ok(inbound())
    }).is_err(), "a deferred constraint must fail commit, with no ACK outcome");
    assert_eq!(count(&db.connection, "SELECT count(*) FROM test_state"), 0);
    assert_eq!(count(&db.connection, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&db.connection, "SELECT count(*) FROM inbox"), 0);

    let payload = vec![14, 0, 23];
    assert_eq!(db.commit_outgoing(id, "out-event", |tx| {
        fake_state(tx, 1)?;
        Ok((outbound(), payload.clone()))
    }).unwrap(), payload);
    assert_eq!(db.commit_incoming("123", "in-event", &[9; 32], |tx| {
        fake_state(tx, 2)?;
        Ok(inbound())
    }).unwrap(), IncomingOutcome::Stored);
    db.close().unwrap();

    let mut db = Database::open(&root.0, OWNER).unwrap();
    assert_eq!(db.commit_outgoing(id, "out-event", |_| panic!("retry must reuse payload")).unwrap(), payload);
    assert_eq!(db.commit_incoming("123", "in-event", &[9; 32], |_| panic!("duplicate must skip processing")).unwrap(),
        IncomingOutcome::AlreadyStored);
    assert!(matches!(db.commit_incoming("123", "in-event", &[10; 32], |_| panic!("conflict must skip processing")), Err(Error::EventConflict)));
    assert_eq!(count(&db.connection, "SELECT count(*) FROM test_state"), 2);
    assert_eq!(db.messages(id).unwrap().len(), 2);
}

#[test]
#[ignore = "prints timings for synthetic encrypted history"]
fn measure_open_and_integrity_separately() {
    use std::time::Instant;

    for count in [0, 1_000, 10_000, 50_000] {
        let root = TestDir::new();
        let mut db = Database::open(&root.0, OWNER).unwrap();
        let id = db.create_conversation("123", 0).unwrap();
        db.write(|tx| {
            let body = "Synthetic message ".repeat(27);
            let mut insert = tx.prepare("INSERT INTO messages VALUES (?1, ?2, 0, ?3, ?4, 0)")?;
            for index in 0..count {
                insert.execute(params![format!("message-{index}"), id, body, index])?;
            }
            Ok(())
        }).unwrap();
        db.close().unwrap();
        let mut opens = Vec::new();
        let mut checks = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let db = Database::open(&root.0, OWNER).unwrap();
            opens.push(start.elapsed());
            db.close().unwrap();
            let start = Instant::now();
            let report = check_existing(&root.0, OWNER).unwrap();
            checks.push(start.elapsed());
            assert!(report.cipher && report.sqlite && report.foreign_keys);
        }
        opens.sort_unstable();
        checks.sort_unstable();
        println!("{count:>5} messages, {:>8} KiB: median open {:?}; explicit diagnostic {:?}",
            fs::metadata(root.db()).unwrap().len() / 1024, opens[2], checks[2]);
    }
}
