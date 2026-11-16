use super::{
    INITIAL_LAST_RESORT_ID, INITIAL_SIGNED_ID, adapters::SignalStore, binding::sign_binding,
    peer::TrustedPeer,
};
use crate::storage::{Database, Error, Owner, events::IncomingOutcome, tests::TestDir};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use libsignal_protocol::{
    GenericSignedPreKey, IdentityKeyStore, KyberPreKeyStore, PreKeySignalMessage, PreKeyStore,
    SignalMessage, SignedPreKeyStore,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use zeroize::Zeroizing;

const ALICE: &str = "101";
const BOB: &str = "202";
const EC_ID: u32 = 3;
const KYBER_ID: u32 = 4;
const NOW_MS: i64 = 1_795_000_000_000;

struct TestAccount {
    dir: TestDir,
    id: &'static str,
    key: SigningKey,
}

impl TestAccount {
    fn new(id: &'static str, key_byte: u8) -> Self {
        Self {
            dir: TestDir::new(),
            id,
            key: SigningKey::from_bytes(&[key_byte; 32]),
        }
    }

    fn open(&self) -> Database {
        let public_key = self.key.verifying_key();
        let mut db = Database::open(
            &self.dir.0,
            Owner {
                void_id: self.id,
                public_key: public_key.as_bytes(),
            },
        )
        .unwrap();
        db.initialize_signal(self.id, &self.key).unwrap();
        db
    }

    fn trusted(&self) -> TrustedPeer {
        TrustedPeer::from_trusted_lookup(self.id, self.key.verifying_key().as_bytes()).unwrap()
    }
}

fn at() -> SystemTime {
    static NOW: OnceLock<SystemTime> = OnceLock::new();
    *NOW.get_or_init(SystemTime::now)
}

fn timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn event_id(i: u32) -> String {
    format!("00000000-0000-4000-8000-{i:012x}")
}

fn public_state(db: &mut Database) -> (Box<[u8]>, u32, Vec<u8>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    db.write(|tx| {
        let store = SignalStore::new(tx);
        let (key, registration) = runtime.block_on(async {
            (
                store
                    .get_identity_key_pair()
                    .await
                    .unwrap()
                    .identity_key()
                    .serialize(),
                store.get_local_registration_id().await.unwrap(),
            )
        });
        let signature = tx.query_row(
            "SELECT binding_signature FROM signal_local_identity WHERE slot = 1",
            [],
            |row| row.get(0),
        )?;
        Ok((key, registration, signature))
    })
    .unwrap()
}

fn claim(db: &mut Database, last_resort: bool) -> Value {
    let (identity, registration_id, binding) = public_state(db);
    let kyber_id = if last_resort {
        INITIAL_LAST_RESORT_ID
    } else {
        KYBER_ID
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    db.write(|tx| {
        let store = SignalStore::new(tx);
        let (signed, kyber, one_time) = runtime.block_on(async {
            let signed = store
                .get_signed_pre_key(INITIAL_SIGNED_ID.into())
                .await
                .unwrap();
            let kyber = store.get_kyber_pre_key(kyber_id.into()).await.unwrap();
            let one_time = if last_resort {
                None
            } else {
                Some(store.get_pre_key(EC_ID.into()).await.unwrap())
            };
            (signed, kyber, one_time)
        });
        // Build a VOID prekey claim from this test account's persisted libsignal keys.
        let mut bundle = json!({
            "v": 1,
            "device_id": 1,
            "registration_id": registration_id,
            "identity_key": STANDARD.encode(identity),
            "account_signature": STANDARD.encode(binding),
            "signed_prekey": {
                "key_id": INITIAL_SIGNED_ID,
                "public_key": STANDARD.encode(signed.public_key().unwrap().serialize()),
                "signature": STANDARD.encode(signed.signature().unwrap()),
            },
            "kyber_prekey": {
                "key_id": kyber_id,
                "public_key": STANDARD.encode(kyber.public_key().unwrap().serialize()),
                "signature": STANDARD.encode(kyber.signature().unwrap()),
                "last_resort": last_resort,
            },
            "claim_expires_at": timestamp(at() + Duration::from_secs(60)),
        });
        if let Some(one_time) = one_time {
            bundle["one_time_prekey"] = json!({
                "key_id": EC_ID,
                "public_key": STANDARD.encode(one_time.public_key().unwrap().serialize()),
            });
        }
        Ok(bundle)
    })
    .unwrap()
}

fn encode(value: &Value) -> String {
    STANDARD.encode(serde_json::to_vec(value).unwrap())
}

fn decode(payload: &[u8]) -> Value {
    serde_json::from_slice(&STANDARD.decode(payload).unwrap()).unwrap()
}

fn relay_event(sender: &str, id: &str, payload: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "event",
        "from_void_id": sender,
        "event_id": id,
        "payload_base64": std::str::from_utf8(payload).unwrap(),
        "created_at": timestamp(at()),
        "expires_at": timestamp(at() + Duration::from_secs(60)),
    }))
    .unwrap()
}

fn session(db: &Database, peer: &str) -> Option<Zeroizing<Vec<u8>>> {
    db.connection
        .query_row(
            "SELECT record FROM signal_sessions WHERE peer_void_id = ?1 AND device_id = 1",
            [peer],
            |row| row.get::<_, Vec<u8>>(0).map(Zeroizing::new),
        )
        .optional()
        .unwrap()
}

fn has_prekey(db: &Database, id: u32) -> bool {
    db.connection
        .prepare("SELECT 1 FROM signal_prekeys WHERE id = ?1")
        .unwrap()
        .exists([id])
        .unwrap()
}

fn assert_envelope(payload: &[u8], from: &str, to: &str, id: &str, kind: u32) -> Vec<u8> {
    let envelope = decode(payload);
    assert_eq!(envelope["v"], json!(2));
    assert_eq!(envelope["kind"], "signal");
    assert_eq!(envelope["client_message_id"], id);
    assert_eq!(envelope["from_void_id"], from);
    assert_eq!(envelope["to_void_id"], to);
    assert_eq!(envelope["device_id"], json!(1));
    assert_eq!(envelope["message_type"], json!(kind));
    assert_eq!(
        STANDARD
            .decode(envelope["identity_key"].as_str().unwrap())
            .unwrap()
            .len(),
        33
    );
    assert_eq!(
        STANDARD
            .decode(envelope["account_signature"].as_str().unwrap())
            .unwrap()
            .len(),
        64
    );
    STANDARD
        .decode(envelope["ciphertext_base64"].as_str().unwrap())
        .unwrap()
}

fn assert_received(db: &Database, peer: &str, id: &str, body: &str, created_at: i64) {
    let conversation = db
        .connection
        .query_row(
            "SELECT id FROM conversations WHERE peer_void_id = ?1",
            [peer],
            |row| row.get(0),
        )
        .unwrap();
    let messages = db.messages(conversation).unwrap();
    let stored_id = format!("{peer}:{id}");
    let message = messages.iter().find(|item| item.id == stored_id).unwrap();
    assert_eq!(message.direction, crate::storage::Direction::Incoming);
    assert!(message.body == body);
    assert_eq!(message.created_at, created_at);
}

#[test]
fn interrupted_receipt_preparation_rolls_back_ratchet_and_resumes_with_the_same_id() {
    let alice = TestAccount::new(ALICE, 71);
    let bob = TestAccount::new(BOB, 72);
    let mut a = alice.open();
    let mut b = bob.open();
    let peer_a = alice.trusted();
    let peer_b = bob.trusted();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    a.establish_signal_session(&peer_b, BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let (original, _) = a.queue_outgoing(chat, "Prepare a receipt", NOW_MS).unwrap();
    let payload = a
        .encrypt_signal_message(&peer_b, chat, &original, "Prepare a receipt", NOW_MS)
        .unwrap();
    b.decrypt_signal_event(&peer_a, &relay_event(ALICE, &original, &payload), at())
        .unwrap();

    let now = b.next_receipt_attempt(0).unwrap().unwrap();
    let first = b.claim_receipt(now).unwrap().unwrap();
    let receipt_id = first.event_id.clone();
    let before = session(&b, ALICE);
    b.connection
        .execute_batch(
            "CREATE TEMP TRIGGER interrupt_receipt BEFORE UPDATE OF payload ON receipt_outbox
         BEGIN SELECT RAISE(ABORT, 'interrupt receipt'); END;",
        )
        .unwrap();
    assert!(b.encrypt_signal_receipt(first).is_err());
    assert!(session(&b, ALICE) == before);
    b.close().unwrap();

    let mut b = bob.open();
    assert!(b.claim_receipt(now + 1).unwrap().is_none());
    let next = b.next_receipt_attempt(now).unwrap().unwrap();
    let attempt = b.claim_receipt(next).unwrap().unwrap();
    assert_eq!(attempt.event_id, receipt_id);
    assert!(attempt.payload.is_none());
    let payload = b.encrypt_signal_receipt(attempt).unwrap();
    a.decrypt_signal_event(&peer_b, &relay_event(BOB, &receipt_id, &payload), at())
        .unwrap();
    assert!(a.has_verified_delivery(&original).unwrap());

    for _ in 2..8 {
        let next = b.next_receipt_attempt(now).unwrap().unwrap();
        let retry = b.claim_receipt(next).unwrap().unwrap();
        assert_eq!(retry.event_id, receipt_id);
        assert_eq!(retry.payload.as_deref(), Some(payload.as_slice()));
    }
    assert!(b.next_receipt_attempt(now).unwrap().is_none());
}

#[test]
fn authenticated_receipts_cannot_claim_legacy_unrelated_or_unknown_messages() {
    use super::{
        crypto::encrypt_content,
        wire::{Routing, encode_receipt},
    };
    use crate::storage::{DeliveryState, Direction, Message};

    let alice = TestAccount::new(ALICE, 71);
    let bob = TestAccount::new(BOB, 72);
    let mut a = alice.open();
    let mut b = bob.open();
    let peer_a = alice.trusted();
    let peer_b = bob.trusted();
    a.establish_signal_session(&peer_b, BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let unrelated = a.create_conversation("303", NOW_MS).unwrap();
    let legacy = event_id(1);
    let payload = a
        .commit_outgoing(chat, &legacy, false, |tx| {
            let routing = Routing {
                event_id: &legacy,
                sender: ALICE,
                recipient: BOB,
            };
            let plaintext = json!({"v":2, "client_message_id":legacy, "from_void_id":ALICE,
            "to_void_id":BOB, "body":"Legacy message", "created_at":NOW_MS});
            let payload = encrypt_content(
                tx,
                &peer_b,
                routing,
                &serde_json::to_vec(&plaintext).unwrap(),
            )?;
            Ok((
                Message {
                    id: format!("{ALICE}:{legacy}"),
                    conversation_id: chat,
                    direction: Direction::Outgoing,
                    body: "Legacy message".into(),
                    created_at: NOW_MS,
                    delivery_state: DeliveryState::Pending,
                },
                payload,
            ))
        })
        .unwrap();
    b.decrypt_signal_event(&peer_a, &relay_event(ALICE, &legacy, &payload), at())
        .unwrap();
    assert!(b.next_receipt_attempt(NOW_MS).unwrap().is_none());

    let unrelated_id = event_id(2);
    a.commit_outgoing(unrelated, &unrelated_id, true, |_| {
        Ok((
            Message {
                id: format!("{ALICE}:{unrelated_id}"),
                conversation_id: unrelated,
                direction: Direction::Outgoing,
                body: "For another peer".into(),
                created_at: NOW_MS,
                delivery_state: DeliveryState::Pending,
            },
            b"fixture".to_vec(),
        ))
    })
    .unwrap();
    let (opted_in, _) = a
        .queue_outgoing(chat, "Receipt requested", NOW_MS + 1)
        .unwrap();
    a.encrypt_signal_message(&peer_b, chat, &opted_in, "Receipt requested", NOW_MS + 1)
        .unwrap();

    let before = session(&a, BOB);
    for (index, target) in [legacy.as_str(), unrelated_id.as_str(), event_id(3).as_str()]
        .into_iter()
        .enumerate()
    {
        let control_id = event_id(10 + index as u32);
        let routing = Routing {
            event_id: &control_id,
            sender: BOB,
            recipient: ALICE,
        };
        let plaintext = encode_receipt(routing, target).unwrap();
        let cipher = b
            .write(|tx| encrypt_content(tx, &peer_a, routing, &plaintext))
            .unwrap();
        let event = relay_event(BOB, &control_id, &cipher);
        assert!(matches!(
            a.decrypt_signal_event(&peer_b, &event, at()),
            Err(Error::SignalEnvelope)
        ));
        assert!(!a.has_verified_delivery(target).unwrap());
        assert!(session(&a, BOB) == before);
    }
    let control_id = event_id(20);
    let routing = Routing {
        event_id: &control_id,
        sender: BOB,
        recipient: ALICE,
    };
    let plaintext = encode_receipt(routing, &opted_in).unwrap();
    let cipher = b
        .write(|tx| encrypt_content(tx, &peer_a, routing, &plaintext))
        .unwrap();
    a.decrypt_signal_event(&peer_b, &relay_event(BOB, &control_id, &cipher), at())
        .unwrap();
    assert!(a.has_verified_delivery(&opted_in).unwrap());
    let verified = session(&a, BOB);
    let mut changed = decode(&cipher);
    changed["ciphertext_base64"] = json!("AQ==");
    let conflicting = relay_event(BOB, &control_id, encode(&changed).as_bytes());
    assert!(matches!(
        a.decrypt_signal_event(&peer_b, &conflicting, at()),
        Err(Error::EventConflict)
    ));
    assert!(session(&a, BOB) == verified);
    assert_eq!(
        a.messages(chat).unwrap()[1].delivery_state,
        DeliveryState::Delivered
    );
}

#[test]
fn real_signal_messages_and_sessions_survive_restarts() {
    let alice = TestAccount::new(ALICE, 71);
    let bob = TestAccount::new(BOB, 72);
    let mut a = alice.open();
    let mut b = bob.open();
    let a_identity = public_state(&mut a);
    let b_identity = public_state(&mut b);
    assert_ne!(a_identity.0, b_identity.0);
    a.close().unwrap();
    b.close().unwrap();

    let mut a = alice.open();
    let mut b = bob.open();
    assert!(public_state(&mut a) == a_identity);
    assert!(public_state(&mut b) == b_identity);
    let peer_a = alice.trusted();
    let peer_b = bob.trusted();
    a.establish_signal_session(&peer_b, BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let a_conversation = a.create_conversation(BOB, NOW_MS).unwrap();
    let first_id = event_id(1);
    let first_body = "Round trip: quotes \"quoted\", newline\n and Unicode ✓";
    let first = a
        .encrypt_signal_message(&peer_b, a_conversation, &first_id, first_body, NOW_MS)
        .unwrap();
    let first_cipher = assert_envelope(&first, ALICE, BOB, &first_id, 3);
    let prekey = PreKeySignalMessage::try_from(first_cipher.as_slice()).unwrap();
    assert_eq!(prekey.pre_key_id(), Some(EC_ID.into()));
    assert_eq!(prekey.signed_pre_key_id(), INITIAL_SIGNED_ID.into());
    assert_eq!(prekey.kyber_pre_key_id(), Some(KYBER_ID.into()));

    let first_event = relay_event(ALICE, &first_id, &first);
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &first_event, at()).unwrap(),
        IncomingOutcome::Stored
    );
    assert_received(&b, ALICE, &first_id, first_body, NOW_MS);
    assert!(!has_prekey(&b, EC_ID));
    assert!(!has_prekey(&b, KYBER_ID));
    assert!(has_prekey(&b, INITIAL_LAST_RESORT_ID));
    let b_after_first = session(&b, ALICE).unwrap();
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &first_event, at()).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(session(&b, ALICE).unwrap() == b_after_first);

    let b_conversation = b.create_conversation(ALICE, NOW_MS).unwrap();
    let reply_id = event_id(2);
    let reply = b
        .encrypt_signal_message(&peer_a, b_conversation, &reply_id, "Reply", NOW_MS + 1)
        .unwrap();
    let reply_cipher = assert_envelope(&reply, BOB, ALICE, &reply_id, 2);
    SignalMessage::try_from(reply_cipher.as_slice()).unwrap();
    assert_eq!(
        a.decrypt_signal_event(&peer_b, &relay_event(BOB, &reply_id, &reply), at())
            .unwrap(),
        IncomingOutcome::Stored
    );
    assert_received(&a, BOB, &reply_id, "Reply", NOW_MS + 1);

    let a_session = session(&a, BOB).unwrap();
    let b_session = session(&b, ALICE).unwrap();
    a.close().unwrap();
    b.close().unwrap();

    let mut a = alice.open();
    let mut b = bob.open();
    assert!(public_state(&mut a) == a_identity);
    assert!(public_state(&mut b) == b_identity);
    assert!(session(&a, BOB).unwrap() == a_session);
    assert!(session(&b, ALICE).unwrap() == b_session);
    assert!(!has_prekey(&b, EC_ID));
    assert!(!has_prekey(&b, KYBER_ID));
    for i in 3..=6 {
        let id = event_id(i);
        let when = NOW_MS + i64::from(i);
        let body = format!("Established message {i}");
        let (from_db, to_db, peer, sender, receiver, from_conversation, to_peer) = if i % 2 != 0 {
            (&mut a, &mut b, &peer_b, ALICE, BOB, a_conversation, &peer_a)
        } else {
            (&mut b, &mut a, &peer_a, BOB, ALICE, b_conversation, &peer_b)
        };
        let payload = from_db
            .encrypt_signal_message(peer, from_conversation, &id, &body, when)
            .unwrap();
        assert_envelope(&payload, sender, receiver, &id, 2);
        assert_eq!(
            to_db
                .decrypt_signal_event(to_peer, &relay_event(sender, &id, &payload), at())
                .unwrap(),
            IncomingOutcome::Stored
        );
        assert_received(to_db, sender, &id, &body, when);
    }
}

#[test]
fn malformed_claims_and_changed_identities_cannot_pin_or_replace_a_session() {
    let alice = TestAccount::new(ALICE, 73);
    let bob = TestAccount::new(BOB, 74);
    let mut a = alice.open();
    let mut b = bob.open();
    let claim_value = claim(&mut b, false);
    let peer_b = bob.trusted();
    let a_conversation = a.create_conversation(BOB, NOW_MS).unwrap();
    assert!(
        a.encrypt_signal_message(&peer_b, a_conversation, &event_id(1), "Unverified", NOW_MS)
            .is_err()
    );

    assert!(
        a.establish_signal_session(&peer_b, ALICE, &encode(&claim_value), at())
            .is_err()
    );
    for (field, invalid) in [
        ("v", json!(2)),
        ("device_id", json!(2)),
        ("registration_id", json!(0)),
        ("identity_key", json!("invalid%")),
        ("account_signature", json!(STANDARD.encode([0; 64]))),
        ("claim_expires_at", json!(timestamp(at()))),
    ] {
        let mut invalid_claim = claim_value.clone();
        invalid_claim[field] = invalid;
        assert!(
            a.establish_signal_session(&peer_b, BOB, &encode(&invalid_claim), at())
                .is_err()
        );
    }
    for key in ["signed_prekey", "kyber_prekey"] {
        let mut invalid_claim = claim_value.clone();
        invalid_claim[key]["signature"] = json!(STANDARD.encode([0; 64]));
        assert!(
            a.establish_signal_session(&peer_b, BOB, &encode(&invalid_claim), at())
                .is_err()
        );
    }
    assert!(session(&a, BOB).is_none());
    assert_eq!(
        a.connection
            .query_row("SELECT count(*) FROM signal_identities", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    a.establish_signal_session(&peer_b, BOB, &encode(&claim_value), at())
        .unwrap();
    let original = session(&a, BOB).unwrap();

    let bob_with_new_key = TestAccount::new(BOB, 75);
    let mut replaced_account = bob_with_new_key.open();
    assert!(
        a.establish_signal_session(
            &bob_with_new_key.trusted(),
            BOB,
            &encode(&claim(&mut replaced_account, false)),
            at(),
        )
        .is_err()
    );
    let bob_with_new_signal = TestAccount::new(BOB, 74);
    let mut replaced_signal = bob_with_new_signal.open();
    assert!(
        a.establish_signal_session(
            &peer_b,
            BOB,
            &encode(&claim(&mut replaced_signal, false)),
            at(),
        )
        .is_err()
    );
    assert!(
        a.encrypt_signal_message(
            &bob_with_new_key.trusted(),
            a_conversation,
            &event_id(2),
            "Wrong account key",
            NOW_MS,
        )
        .is_err()
    );
    assert!(session(&a, BOB).unwrap() == original);
}

// A deferred foreign key fails only when SQLite attempts COMMIT, after crypto and
// all event writes have run successfully inside the transaction.
fn fail_next_event_commit(db: &Database, table: &str) {
    assert!(matches!(table, "inbox" | "outbox"));
    db.connection
        .execute_batch(&format!(
            "CREATE TABLE test_deferred_fk (
            message_id TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED
        );
        CREATE TRIGGER test_fail_event_commit AFTER INSERT ON {table}
        BEGIN INSERT INTO test_deferred_fk VALUES ('missing-test-message'); END;",
        ))
        .unwrap();
}

fn allow_event_commit(db: &Database) {
    assert_eq!(
        db.connection
            .query_row("SELECT count(*) FROM test_deferred_fk", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    db.connection
        .execute_batch("DROP TRIGGER test_fail_event_commit; DROP TABLE test_deferred_fk;")
        .unwrap();
}

#[test]
fn commit_failures_and_duplicate_events_do_not_advance_signal_state() {
    let alice = TestAccount::new(ALICE, 76);
    let bob = TestAccount::new(BOB, 77);
    let peer_a = alice.trusted();
    let peer_b = bob.trusted();
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&peer_b, BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let conv = a.create_conversation(BOB, NOW_MS).unwrap();
    let before_encrypt = session(&a, BOB).unwrap();
    let id = event_id(1);

    fail_next_event_commit(&a, "outbox");
    assert!(matches!(
        a.encrypt_signal_message(&peer_b, conv, &id, "Retry exactly", NOW_MS),
        Err(Error::Database)
    ));
    assert!(session(&a, BOB).unwrap() == before_encrypt);
    assert!(a.messages(conv).unwrap().is_empty());
    assert_eq!(
        a.connection
            .query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    allow_event_commit(&a);
    let first = a
        .encrypt_signal_message(&peer_b, conv, &id, "Retry exactly", NOW_MS)
        .unwrap();
    let after_encrypt = session(&a, BOB).unwrap();
    a.close().unwrap();

    let mut a = alice.open();
    for (body, time) in [("Changed body", NOW_MS), ("Retry exactly", NOW_MS + 1)] {
        assert!(matches!(
            a.encrypt_signal_message(&peer_b, conv, &id, body, time),
            Err(Error::EventConflict)
        ));
    }
    let retried = a
        .encrypt_signal_message(&peer_b, conv, &id, "Retry exactly", NOW_MS)
        .unwrap();
    assert_eq!(retried, first);
    let wrong_bob = TestAccount::new(BOB, 81).trusted();
    assert!(matches!(
        a.encrypt_signal_message(&wrong_bob, conv, &id, "Retry exactly", NOW_MS),
        Err(Error::SignalPeer)
    ));
    assert!(session(&a, BOB).unwrap() == after_encrypt);
    assert_eq!(a.messages(conv).unwrap().len(), 1);
    let saved = a
        .connection
        .query_row(
            "SELECT payload FROM outbox WHERE event_id = ?1",
            [&id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .unwrap();
    assert_eq!(saved, first);

    let event = relay_event(ALICE, &id, &first);
    fail_next_event_commit(&b, "inbox");
    assert!(matches!(
        b.decrypt_signal_event(&peer_a, &event, at()),
        Err(Error::Database)
    ));
    assert!(session(&b, ALICE).is_none());
    assert!(has_prekey(&b, EC_ID));
    assert!(has_prekey(&b, KYBER_ID));
    assert_eq!(
        b.connection
            .query_row(
                "SELECT (SELECT count(*) FROM signal_identities),
                    (SELECT count(*) FROM signal_peer_bindings), (SELECT count(*) FROM messages),
                    (SELECT count(*) FROM inbox), (SELECT count(*) FROM conversations),
                    (SELECT count(*) FROM requests)",
                [],
                |row| Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?
                )),
            )
            .unwrap(),
        (0, 0, 0, 0, 0, 0),
    );
    allow_event_commit(&b);

    let wrong_peer = TestAccount::new(ALICE, 78).trusted();
    assert!(b.decrypt_signal_event(&wrong_peer, &event, at()).is_err());
    for (field, invalid) in [
        ("from_void_id", json!(BOB)),
        ("to_void_id", json!(ALICE)),
        ("device_id", json!(2)),
        ("message_type", json!(4)),
        ("account_signature", json!(STANDARD.encode([0; 64]))),
        ("ciphertext_base64", json!("$")),
    ] {
        let mut invalid_envelope = decode(&first);
        invalid_envelope[field] = invalid;
        assert!(
            b.decrypt_signal_event(
                &peer_a,
                &relay_event(ALICE, &id, encode(&invalid_envelope).as_bytes()),
                at()
            )
            .is_err()
        );
    }

    // The binding is valid, but it disagrees with the registration ID in the ciphertext.
    let mut wrong_registration = decode(&first);
    let registration =
        (wrong_registration["registration_id"].as_u64().unwrap() as u32 % 16_380) + 1;
    let identity = STANDARD
        .decode(wrong_registration["identity_key"].as_str().unwrap())
        .unwrap();
    let signature = sign_binding(&alice.key, ALICE, 1, registration, &identity).unwrap();
    wrong_registration["registration_id"] = json!(registration);
    wrong_registration["account_signature"] = json!(STANDARD.encode(signature));
    let invalid_event = relay_event(ALICE, &id, encode(&wrong_registration).as_bytes());
    assert!(matches!(
        b.decrypt_signal_event(&peer_a, &invalid_event, at()),
        Err(Error::SignalPeer)
    ));

    let mut corrupted = decode(&first);
    let mut ciphertext = STANDARD
        .decode(corrupted["ciphertext_base64"].as_str().unwrap())
        .unwrap();
    *ciphertext.last_mut().unwrap() ^= 1;
    corrupted["ciphertext_base64"] = json!(STANDARD.encode(&ciphertext));
    assert!(
        b.decrypt_signal_event(
            &peer_a,
            &relay_event(ALICE, &id, encode(&corrupted).as_bytes()),
            at()
        )
        .is_err()
    );

    let wrong_id = event_id(2);
    let mut altered_event_id = decode(&first);
    altered_event_id["client_message_id"] = json!(wrong_id);
    assert!(
        b.decrypt_signal_event(
            &peer_a,
            &relay_event(ALICE, &wrong_id, encode(&altered_event_id).as_bytes()),
            at(),
        )
        .is_err()
    );
    assert!(session(&b, ALICE).is_none());
    assert!(has_prekey(&b, EC_ID));
    assert!(has_prekey(&b, KYBER_ID));
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &event, at()).unwrap(),
        IncomingOutcome::Stored
    );
    assert_received(&b, ALICE, &id, "Retry exactly", NOW_MS);
    let after_decrypt = session(&b, ALICE).unwrap();
    b.close().unwrap();

    let mut b = bob.open();
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &event, at()).unwrap(),
        IncomingOutcome::AlreadyStored
    );
    assert!(matches!(
        b.decrypt_signal_event(
            &peer_a,
            &relay_event(ALICE, &id, encode(&corrupted).as_bytes()),
            at()
        ),
        Err(Error::EventConflict)
    ));
    assert!(session(&b, ALICE).unwrap() == after_decrypt);
    assert_eq!(
        b.connection
            .query_row("SELECT count(*) FROM messages", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn last_resort_kyber_without_one_time_ec_persists_replay_protection() {
    let alice = TestAccount::new(ALICE, 79);
    let bob = TestAccount::new(BOB, 80);
    let peer_a = alice.trusted();
    let peer_b = bob.trusted();
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&peer_b, BOB, &encode(&claim(&mut b, true)), at())
        .unwrap();
    let conv = a.create_conversation(BOB, NOW_MS).unwrap();
    let id = event_id(1);
    let payload = a
        .encrypt_signal_message(&peer_b, conv, &id, "Fallback keys", NOW_MS)
        .unwrap();
    let ciphertext = assert_envelope(&payload, ALICE, BOB, &id, 3);
    let prekey = PreKeySignalMessage::try_from(ciphertext.as_slice()).unwrap();
    assert_eq!(prekey.pre_key_id(), None);
    assert_eq!(
        prekey.kyber_pre_key_id(),
        Some(INITIAL_LAST_RESORT_ID.into())
    );
    let event = relay_event(ALICE, &id, &payload);
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &event, at()).unwrap(),
        IncomingOutcome::Stored
    );
    b.close().unwrap();
    let mut b = bob.open();
    assert!(has_prekey(&b, EC_ID));
    assert!(has_prekey(&b, KYBER_ID));
    assert!(has_prekey(&b, INITIAL_LAST_RESORT_ID));
    assert!(
        b.connection
            .prepare(
                "SELECT 1 FROM signal_kyber_uses
         WHERE kyber_prekey_id = ?1 AND signed_prekey_id = ?2 AND base_key = ?3",
            )
            .unwrap()
            .exists(params![
                INITIAL_LAST_RESORT_ID,
                INITIAL_SIGNED_ID,
                prekey.base_key().serialize().as_ref(),
            ])
            .unwrap()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    b.write(|tx| {
        let mut store = SignalStore::new(tx);
        assert!(
            runtime
                .block_on(store.mark_kyber_pre_key_used(
                    INITIAL_LAST_RESORT_ID.into(),
                    INITIAL_SIGNED_ID.into(),
                    prekey.base_key(),
                ))
                .is_err()
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(
        b.decrypt_signal_event(&peer_a, &event, at()).unwrap(),
        IncomingOutcome::AlreadyStored
    );
}
