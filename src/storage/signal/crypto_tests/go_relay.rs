//! Explicit opt-in integration with an unmodified, disposable Go relay.
//! All account keys and SQLCipher histories belong to synthetic test identities.
use super::*;
use crate::{
    account::RegisteredIdentity,
    relay::{Activity, ApplicationProtocolVersion, Connection, Reply, Request, Session, Worker},
    storage::message_events::MessageReference,
    storage::signal::{crypto::encrypt_content, wire::Routing},
};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};

const WAIT: Duration = Duration::from_secs(20);
const BACKEND_COMMIT: &str = "2faffd27f12d1d02ec28e4be298610d4fee50856";

struct Client {
    connection: Connection,
    worker: Option<Worker>,
    events: Receiver<Activity>,
    session: Session,
    id: &'static str,
}

impl Client {
    fn start(
        endpoint: &str,
        audience: &str,
        id: &'static str,
        seed: u8,
        version: ApplicationProtocolVersion,
    ) -> Self {
        let (send, events) = mpsc::channel();
        let (connection, worker) = Connection::spawn_with_test_protocol(
            endpoint.into(),
            audience.into(),
            version,
            move |requested| {
                assert_eq!(requested, id);
                Ok(RegisteredIdentity {
                    void_id: id.into(),
                    key: SigningKey::from_bytes(&[seed; 32]),
                })
            },
            |_| {},
            move |event| {
                let _ = send.send(event);
            },
        )
        .unwrap();
        assert!(connection.connect(id.into()));
        let session = loop {
            match events
                .recv_timeout(WAIT)
                .expect("Go relay did not authenticate and negotiate")
            {
                Activity::Online(session) => break session,
                Activity::Offline(_) => {
                    panic!("Go relay rejected isolated authentication/negotiation")
                }
                Activity::Delivery { .. } => panic!("delivery arrived before protocol agreement"),
            }
        };
        Self {
            connection,
            worker: Some(worker),
            events,
            session,
            id,
        }
    }

    fn delivery(&mut self, id: &str, versioned: bool) -> String {
        let Activity::Delivery {
            generation,
            event_id,
            json,
            ..
        } = self
            .events
            .recv_timeout(WAIT)
            .expect("Go queued delivery missing")
        else {
            panic!("relay disconnected before queued delivery");
        };
        assert_eq!(generation, self.session.generation);
        assert_eq!(event_id, id);
        let frame: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            frame["type"],
            if versioned {
                "event_versioned"
            } else {
                "event"
            }
        );
        if versioned {
            assert_eq!(frame["min_protocol_version"], "2");
        } else {
            assert!(frame.get("min_protocol_version").is_none());
        }
        json
    }

    fn no_delivery(&self) {
        assert!(
            matches!(
                self.events.recv_timeout(Duration::from_millis(1_250)),
                Err(RecvTimeoutError::Timeout)
            ),
            "an incompatible or unacknowledged queue head must block later events"
        );
    }

    fn reconnect(&mut self) {
        let old = self.session.generation;
        assert!(self.connection.connect(self.id.into()));
        loop {
            match self
                .events
                .recv_timeout(WAIT)
                .expect("reconnect did not negotiate again")
            {
                Activity::Online(session) => {
                    assert_ne!(session.generation, old);
                    self.session = session;
                    break;
                }
                Activity::Offline(generation) => assert_eq!(generation, old),
                Activity::Delivery { .. } => panic!("unexpected queued delivery during reconnect"),
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.finish();
        }
    }
}

fn target(index: u32) -> MessageReference {
    MessageReference {
        author_void_id: ALICE.into(),
        event_id: event_id(index),
    }
}

fn content(index: u32) -> Value {
    json!({"v":2,"client_message_id":event_id(index),"from_void_id":ALICE,"to_void_id":BOB})
}

fn reaction(index: u32, active: bool, revision: i64) -> Value {
    let mut value = content(index);
    value["type"] = json!("reaction");
    value["target"] = json!(target(1));
    value["emoji"] = json!("👍");
    value["active"] = json!(active);
    value["revision"] = json!(revision);
    value
}

fn encrypt_raw(a: &mut Database, bob: &TestAccount, index: u32, value: &Value) -> Vec<u8> {
    let id = event_id(index);
    let routing = Routing {
        event_id: &id,
        sender: ALICE,
        recipient: BOB,
    };
    let bytes = Zeroizing::new(serde_json::to_vec(value).unwrap());
    a.write(|tx| encrypt_content(tx, &bob.trusted(), routing, &bytes))
        .unwrap()
}

fn send(client: &Client, recipient: &str, id: &str, payload: &[u8], versioned: bool) {
    let payload_base64 = std::str::from_utf8(payload).unwrap().into();
    let request = if versioned {
        Request::SendVersioned {
            event_id: id.into(),
            to_void_id: recipient.into(),
            payload_base64,
            min_protocol_version: ApplicationProtocolVersion::V2,
        }
    } else {
        Request::Send {
            event_id: id.into(),
            to_void_id: recipient.into(),
            payload_base64,
        }
    };
    assert!(
        matches!(client.session.request(request),Ok(Reply::Accepted{event_id}) if event_id==id)
    );
}

fn process(db: &mut Database, client: &Client, sender: &str, id: &str, json: &str) {
    crate::messaging::receive(db, &client.session, sender, id, json).unwrap();
}

fn count(db: &Database, query: &str) -> i64 {
    db.connection
        .query_row(query, [], |row| row.get(0))
        .unwrap()
}

#[test]
#[ignore = "requires isolated Go backend 2faffd2 and disposable PostgreSQL fixture; never production"]
fn actual_go_v2_receiver_recovers_blocked_queue_and_commits_before_ack() {
    assert_eq!(std::env::var("VOID_TEST_DISPOSABLE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("VOID_TEST_BACKEND_COMMIT").as_deref(),
        Ok(BACKEND_COMMIT)
    );
    let endpoint =
        std::env::var("VOID_TEST_RELAY_URL").expect("disposable loopback relay URL required");
    let audience =
        std::env::var("VOID_TEST_RELAY_AUDIENCE").expect("disposable relay audience required");
    let alice = TestAccount::new(ALICE, 121);
    let bob = TestAccount::new(BOB, 122);
    let mut a = alice.open();
    let mut b = bob.open();
    let alice_client = Client::start(
        &endpoint,
        &audience,
        ALICE,
        121,
        ApplicationProtocolVersion::V1,
    );
    let mut bob_client = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V1,
    );
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();

    // Real Go authentication, legacy delivery, and the existing encrypted receipt.
    let original = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(1),
            "Original via actual Go",
            NOW_MS,
        )
        .unwrap();
    send(&alice_client, BOB, &event_id(1), &original, false);
    let json = bob_client.delivery(&event_id(1), false);
    process(&mut b, &bob_client, ALICE, &event_id(1), &json);
    let when = b.next_receipt_attempt(0).unwrap().unwrap();
    let upload = b.claim_receipt(when).unwrap().unwrap();
    let receipt_id = upload.event_id.clone();
    let receipt = b.encrypt_signal_receipt(upload).unwrap();
    send(&bob_client, ALICE, &receipt_id, &receipt, false);
    let json = alice_client.events.recv_timeout(WAIT).unwrap();
    let Activity::Delivery {
        event_id: received,
        json,
        ..
    } = json
    else {
        panic!("legacy receipt missing");
    };
    assert_eq!(received, receipt_id);
    process(&mut a, &alice_client, BOB, &receipt_id, &json);
    b.finish_receipt(&receipt_id).unwrap();
    assert!(a.has_verified_delivery(&event_id(1)).unwrap());

    // A version-2 head blocks all later legacy events on a version-1 connection.
    let mut reply = content(2);
    reply["body"] = json!("Reply via actual Go");
    reply["created_at"] = json!(NOW_MS + 1);
    reply["reply_to"] = json!(target(1));
    let reply = encrypt_raw(&mut a, &bob, 2, &reply);
    let later = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(3),
            "Legacy behind v2",
            NOW_MS + 2,
        )
        .unwrap();
    send(&alice_client, BOB, &event_id(2), &reply, true);
    send(&alice_client, BOB, &event_id(3), &later, false);
    bob_client.no_delivery();
    drop(bob_client);
    b.close().unwrap();
    b = bob.open();
    bob_client = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V2,
    );
    let json = bob_client.delivery(&event_id(2), true);
    let before = session(&b, ALICE);
    fail_next_event_commit(&b, "inbox");
    assert!(
        crate::messaging::receive(&mut b, &bob_client.session, ALICE, &event_id(2), &json).is_err()
    );
    assert!(session(&b, ALICE) == before);
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 1);
    bob_client.no_delivery();
    // No ACK on failed COMMIT; actual Go offers exactly the same queue head again.
    drop(bob_client);
    b.close().unwrap();
    b = bob.open();
    allow_event_commit(&b);
    bob_client = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V2,
    );
    let repeated = bob_client.delivery(&event_id(2), true);
    assert_eq!(
        serde_json::from_str::<Value>(&repeated).unwrap()["payload_base64"],
        serde_json::from_str::<Value>(&json).unwrap()["payload_base64"]
    );
    process(&mut b, &bob_client, ALICE, &event_id(2), &repeated);
    assert_eq!(
        b.reply_reference(&format!("{ALICE}:{}", event_id(2)))
            .unwrap(),
        Some(target(1))
    );
    let json = bob_client.delivery(&event_id(3), false);
    process(&mut b, &bob_client, ALICE, &event_id(3), &json);

    // Commit a reaction but lose its ACK, then deduplicate after restart.
    let payload = encrypt_raw(&mut a, &bob, 4, &reaction(4, true, 1));
    send(&alice_client, BOB, &event_id(4), &payload, true);
    let json = bob_client.delivery(&event_id(4), true);
    assert_eq!(
        b.decrypt_signal_delivery(
            &alice.trusted(),
            json.as_bytes(),
            SystemTime::now(),
            Some(&event_id(4))
        )
        .unwrap(),
        IncomingOutcome::Stored
    );
    let committed = session(&b, ALICE);
    drop(bob_client);
    b.close().unwrap();
    b = bob.open();
    bob_client = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V2,
    );
    let json = bob_client.delivery(&event_id(4), true);
    process(&mut b, &bob_client, ALICE, &event_id(4), &json);
    assert!(session(&b, ALICE) == committed);
    assert_eq!(count(&b, "SELECT count(*) FROM message_reactions"), 1);

    // Ciphertext and logical revision orders both differ from relay enqueue order.
    let stale = encrypt_raw(&mut a, &bob, 5, &reaction(5, true, 2));
    let removal = encrypt_raw(&mut a, &bob, 6, &reaction(6, false, 4));
    let middle = encrypt_raw(&mut a, &bob, 7, &reaction(7, true, 3));
    for (index, payload) in [(6, &removal), (5, &stale), (7, &middle)] {
        send(&alice_client, BOB, &event_id(index), payload, true);
        let json = bob_client.delivery(&event_id(index), true);
        process(&mut b, &bob_client, ALICE, &event_id(index), &json);
    }
    assert_eq!(
        b.reaction_states(ALICE, &target(1)).unwrap(),
        vec![(ALICE.into(), "👍".into(), false, 4)]
    );
    let mut delete = content(8);
    delete["type"] = json!("delete_for_everyone");
    delete["target"] = json!(target(1));
    let payload = encrypt_raw(&mut a, &bob, 8, &delete);
    send(&alice_client, BOB, &event_id(8), &payload, true);
    let json = bob_client.delivery(&event_id(8), true);
    process(&mut b, &bob_client, ALICE, &event_id(8), &json);
    assert!(b.message_deleted(ALICE, &target(1)).unwrap());
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    bob_client.reconnect();

    // Mis-versioned but authenticated known content becomes recoverable locally.
    let mut recovered = content(9);
    recovered["body"] = json!("Recovered reply");
    recovered["created_at"] = json!(NOW_MS + 9);
    recovered["reply_to"] = json!(target(1));
    let payload = encrypt_raw(&mut a, &bob, 9, &recovered);
    send(&alice_client, BOB, &event_id(9), &payload, false);
    let json = bob_client.delivery(&event_id(9), false);
    process(&mut b, &bob_client, ALICE, &event_id(9), &json);
    let mut future = content(10);
    future["type"] = json!("still_unknown");
    let payload = encrypt_raw(&mut a, &bob, 10, &future);
    send(&alice_client, BOB, &event_id(10), &payload, true);
    let json = bob_client.delivery(&event_id(10), true);
    process(&mut b, &bob_client, ALICE, &event_id(10), &json);
    assert_eq!(count(&b, "SELECT count(*) FROM deferred_signal_events"), 2);
    let before = session(&b, ALICE);
    drop(bob_client);
    b.close().unwrap();
    b = bob.open();
    let recovery = b.recover_deferred_events().unwrap();
    assert_eq!(recovery.recovered, 1);
    assert_eq!(recovery.deferred, 1);
    assert!(
        session(&b, ALICE) == before,
        "saved plaintext must not advance the Signal ratchet again"
    );
    assert_eq!(b.recover_deferred_events().unwrap().recovered, 0);
    assert_eq!(
        b.reply_reference(&format!("{ALICE}:{}", event_id(9)))
            .unwrap(),
        Some(target(1))
    );
    assert_received(&b, ALICE, &event_id(9), "Recovered reply", NOW_MS + 9);
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    bob_client = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V2,
    );
    let payload = a
        .encrypt_signal_message(
            &bob.trusted(),
            chat,
            &event_id(11),
            "Text after recovery",
            NOW_MS + 11,
        )
        .unwrap();
    send(&alice_client, BOB, &event_id(11), &payload, false);
    let json = bob_client.delivery(&event_id(11), false);
    process(&mut b, &bob_client, ALICE, &event_id(11), &json);
    assert_received(&b, ALICE, &event_id(11), "Text after recovery", NOW_MS + 11);
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 5);
    assert_eq!(
        crate::relay::SUPPORTED_APPLICATION_PROTOCOL,
        ApplicationProtocolVersion::V1
    );
}
