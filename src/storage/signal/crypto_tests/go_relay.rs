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
const BACKEND_COMMIT: &str = "9eb131933a3c9e95cc8b5dba1bebae6400691419";

#[test]
fn outgoing_control_lost_acceptance_reconnect_reuses_ciphertext_without_automatic_send() {
    use crate::relay::tests::{receive_json, send_json, verify_account_proof};
    use futures_util::StreamExt;
    use tokio_tungstenite::accept_async;
    let alice = TestAccount::new(ALICE, 121);
    let bob = TestAccount::new(BOB, 122);
    let mut a = alice.open();
    let mut b = bob.open();
    a.establish_signal_session(&bob.trusted(), BOB, &encode(&claim(&mut b, false)), at())
        .unwrap();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let id = event_id(900);
    a.encrypt_signal_message(&bob.trusted(), chat, &id, "Original", NOW_MS)
        .unwrap();
    let target = MessageReference {
        author_void_id: ALICE.into(),
        event_id: id,
    };
    let control = event_id(901);
    a.queue_reaction(chat, &control, &target, "👍", true, NOW_MS)
        .unwrap();
    let (checked, check) = mpsc::channel();
    let (endpoint, mock) = crate::storage::tests::relay_messaging::mock_relay(
        move |listener| async move {
            let mut saved = None;
            for round in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(tcp).await.unwrap();
                assert_eq!(
                    receive_json(&mut socket).await,
                    json!({"type":"auth_begin","void_id":ALICE})
                );
                verify_account_proof(
                    &mut socket,
                    &SigningKey::from_bytes(&[121; 32]).verifying_key(),
                    ALICE,
                )
                .await;
                send_json(&mut socket, json!({"type":"auth_ok","void_id":ALICE})).await;
                assert_eq!(
                    receive_json(&mut socket).await,
                    json!({"type":"protocol","version":"1"})
                );
                send_json(&mut socket, json!({"type":"protocol_ok","version":"1"})).await;
                if round == 1 {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(350), socket.next())
                            .await
                            .is_err(),
                        "reconnect automatically sent an unfinished control"
                    );
                    checked.send(()).unwrap();
                }
                assert_eq!(
                    receive_json(&mut socket).await,
                    json!({"type":"lookup","void_id":BOB})
                );
                send_json(&mut socket,json!({"type":"person","void_id":BOB,"auth_public_key":STANDARD.encode(SigningKey::from_bytes(&[122;32]).verifying_key().as_bytes())})).await;
                let frame = receive_json(&mut socket).await;
                assert_eq!(frame["type"], "send");
                assert!(frame.get("min_protocol_version").is_none());
                if round == 0 {
                    saved = Some(frame);
                } else {
                    assert!(
                        saved.as_ref() == Some(&frame),
                        "transport retry changed the committed event or ciphertext"
                    );
                    send_json(
                        &mut socket,
                        json!({"type":"error","code":"duplicate_event"}),
                    )
                    .await;
                }
                // First socket closes after receiving the send, before acceptance.
            }
        },
    );
    let mut client = Client::start(
        &endpoint,
        "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz",
        ALICE,
        121,
        ApplicationProtocolVersion::V1,
    );
    assert!(crate::messaging::send_control(&mut a, &client.session, &control).is_err());
    assert_eq!(a.control(&control).unwrap().state, "unconfirmed");
    let payload = a.control(&control).unwrap().payload.unwrap();
    let ratchet = session(&a, BOB);
    a.close().unwrap();
    let mut a = alice.open();
    loop {
        if let Activity::Online(session) = client.events.recv_timeout(WAIT).unwrap() {
            client.session = session;
            break;
        }
    }
    check.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(crate::messaging::send_control(&mut a,&client.session,&control),Err(crate::messaging::Error::Relay(crate::relay::RequestError::Rejected{code,..})) if code=="duplicate_event")
    );
    assert!(ratchet == session(&a, BOB));
    assert!(payload == a.control(&control).unwrap().payload.unwrap());
    assert_eq!(a.control(&control).unwrap().state, "unconfirmed");
    drop(client);
    mock.join().unwrap();
}

#[test]
#[ignore = "requires isolated Go backend 9eb1319 and disposable PostgreSQL fixture; never production"]
fn actual_go_outgoing_features_resume_offline_intents_and_preserve_legacy_receipts() {
    assert_eq!(std::env::var("VOID_TEST_DISPOSABLE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("VOID_TEST_BACKEND_COMMIT").as_deref(),
        Ok(BACKEND_COMMIT)
    );
    let endpoint = std::env::var("VOID_TEST_RELAY_URL").unwrap();
    let audience = std::env::var("VOID_TEST_RELAY_AUDIENCE").unwrap();
    let alice = TestAccount::new(ALICE, 121);
    let bob = TestAccount::new(BOB, 122);
    let mut a = alice.open();
    let mut b = bob.open();
    let a1 = Client::start(
        &endpoint,
        &audience,
        ALICE,
        121,
        ApplicationProtocolVersion::V1,
    );
    let mut bc = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V1,
    );
    crate::messaging::publish(&mut a, &a1.session).unwrap();
    crate::messaging::publish(&mut b, &bc.session).unwrap();
    let chat = a.create_conversation(BOB, NOW_MS).unwrap();
    let (original, _) = a
        .queue_outgoing(chat, "Original through Go", NOW_MS)
        .unwrap();
    crate::messaging::send(&mut a, Some(&a1.session), &original).unwrap();
    let json = bc.delivery(&original, false);
    process(&mut b, &bc, ALICE, &original, &json);
    let target = MessageReference {
        author_void_id: ALICE.into(),
        event_id: original.clone(),
    };
    let (reply, _) = a
        .queue_outgoing_with_reply(chat, "Quoted reply", NOW_MS + 1, Some(&target))
        .unwrap();
    let add = event_id(801);
    a.queue_reaction(chat, &add, &target, "👍", true, NOW_MS)
        .unwrap();
    let mut ac = a1;
    crate::messaging::send(&mut a, Some(&ac.session), &reply).unwrap();
    let json = bc.delivery(&reply, false);
    process(&mut b, &bc, ALICE, &reply, &json);
    let bchat = b.conversations(true).unwrap()[0].id;
    assert_eq!(
        b.timeline_before(bchat, None, 1).unwrap()[0]
            .presentation
            .quote,
        "Original through Go"
    );
    // Original and reply keep ordinary, encrypted delivery receipts.
    while let Some(due) = b.next_receipt_attempt(0).unwrap() {
        let rid = b.claim_receipt(due).unwrap().unwrap();
        let id = rid.event_id.clone();
        let payload = b.encrypt_signal_receipt(rid).unwrap();
        send(&bc, ALICE, &id, &payload, false);
        let json = ac.delivery(&id, false);
        process(&mut a, &ac, BOB, &id, &json);
        b.finish_receipt(&id).unwrap();
    }
    assert!(a.has_verified_delivery(&reply).unwrap());
    drop(bc); // Recipient offline, with its independent protected SQLCipher store.
    // Simulate losing acceptance after transport succeeds: retain the durable
    // unconfirmed intent rather than promoting it based on a guessed outcome.
    let payload = a.encrypt_signal_control(&bob.trusted(), &add).unwrap();
    a.set_control_state(&add, "unconfirmed").unwrap();
    send(&ac, BOB, &add, &payload, false);
    assert!(
        matches!(crate::messaging::send_control(&mut a,&ac.session,&add),Err(crate::messaging::Error::Relay(crate::relay::RequestError::Rejected{code,..})) if code=="duplicate_event")
    );
    assert_eq!(a.control(&add).unwrap().state, "unconfirmed");
    let remove = event_id(802);
    a.queue_reaction(chat, &remove, &target, "👍", false, NOW_MS)
        .unwrap();
    let deletion = event_id(803);
    a.queue_delete_everyone(chat, &deletion, &target, NOW_MS)
        .unwrap();
    assert!(crate::messaging::send(&mut a, Some(&ac.session), &original).is_err());
    let state = session(&a, BOB);
    a.close().unwrap();
    let mut a = alice.open();
    assert!(state == session(&a, BOB));
    assert!(payload == a.control(&add).unwrap().payload.unwrap());
    assert!(a.pending_controls().unwrap().contains(&deletion));
    crate::messaging::send_control(&mut a, &ac.session, &remove).unwrap();
    crate::messaging::send_control(&mut a, &ac.session, &deletion).unwrap();
    assert_eq!(a.control(&remove).unwrap().revision, Some(2));
    assert_eq!(a.control(&deletion).unwrap().state, "accepted");
    b.close().unwrap();
    let mut b = bob.open();
    let mut bc = Client::start(
        &endpoint,
        &audience,
        BOB,
        122,
        ApplicationProtocolVersion::V1,
    );
    for id in [&add, &remove, &deletion] {
        let json = bc.delivery(id, false);
        process(&mut b, &bc, ALICE, id, &json);
        // Duplicate encrypted delivery is safe without ratcheting again.
        assert_eq!(
            b.decrypt_signal_event(&alice.trusted(), json.as_bytes(), SystemTime::now())
                .unwrap(),
            IncomingOutcome::AlreadyStored
        );
    }
    assert!(b.message_deleted(ALICE, &target).unwrap());
    assert_eq!(count(&b, "SELECT count(*) FROM messages"), 2);
    let rows = b.timeline_before(bchat, None, 120).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].presentation.quote.is_empty());
    ac.reconnect();
    bc.reconnect();
    // Existing text uses legacy send after reconnect, without extra bubbles.
    let (plain, _) = a
        .queue_outgoing(chat, "Legacy after controls", NOW_MS + 2)
        .unwrap();
    crate::messaging::send(&mut a, Some(&ac.session), &plain).unwrap();
    let json = bc.delivery(&plain, false);
    process(&mut b, &bc, ALICE, &plain, &json);
    b.delete_for_me(
        bchat,
        &MessageReference {
            author_void_id: ALICE.into(),
            event_id: reply,
        },
    )
    .unwrap();
    assert_eq!(b.timeline_before(bchat, None, 120).unwrap().len(), 1);
    b.close().unwrap();
    let b = bob.open();
    assert_eq!(b.timeline_before(bchat, None, 120).unwrap().len(), 1);
    assert_eq!(
        crate::relay::SUPPORTED_APPLICATION_PROTOCOL,
        ApplicationProtocolVersion::V1
    );
}

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
#[ignore = "requires isolated Go backend 9eb1319 and disposable PostgreSQL fixture; never production"]
fn actual_go_v1_receiver_recovers_and_commits_before_ack() {
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

    // A failed durable reply commit must hold the queue head until reconnect.
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
            "Text behind reply",
            NOW_MS + 2,
        )
        .unwrap();
    send(&alice_client, BOB, &event_id(2), &reply, false);
    send(&alice_client, BOB, &event_id(3), &later, false);
    let json = bob_client.delivery(&event_id(2), false);
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
        ApplicationProtocolVersion::V1,
    );
    let repeated = bob_client.delivery(&event_id(2), false);
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
    send(&alice_client, BOB, &event_id(4), &payload, false);
    let json = bob_client.delivery(&event_id(4), false);
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
        ApplicationProtocolVersion::V1,
    );
    let json = bob_client.delivery(&event_id(4), false);
    process(&mut b, &bob_client, ALICE, &event_id(4), &json);
    assert!(session(&b, ALICE) == committed);
    assert_eq!(count(&b, "SELECT count(*) FROM message_reactions"), 1);

    // Ciphertext and logical revision orders both differ from relay enqueue order.
    let stale = encrypt_raw(&mut a, &bob, 5, &reaction(5, true, 2));
    let removal = encrypt_raw(&mut a, &bob, 6, &reaction(6, false, 4));
    let middle = encrypt_raw(&mut a, &bob, 7, &reaction(7, true, 3));
    for (index, payload) in [(6, &removal), (5, &stale), (7, &middle)] {
        send(&alice_client, BOB, &event_id(index), payload, false);
        let json = bob_client.delivery(&event_id(index), false);
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
    send(&alice_client, BOB, &event_id(8), &payload, false);
    let json = bob_client.delivery(&event_id(8), false);
    process(&mut b, &bob_client, ALICE, &event_id(8), &json);
    assert!(b.message_deleted(ALICE, &target(1)).unwrap());
    assert_received(&b, ALICE, &event_id(1), "", NOW_MS);
    bob_client.reconnect();

    // Model an older receiver durably deferring supported content, then recover
    // the exact saved plaintext after restart without decrypting twice.
    let mut recovered = content(9);
    recovered["body"] = json!("Recovered reply");
    recovered["created_at"] = json!(NOW_MS + 9);
    recovered["reply_to"] = json!(target(1));
    let payload = encrypt_raw(&mut a, &bob, 9, &recovered);
    send(&alice_client, BOB, &event_id(9), &payload, false);
    let json = bob_client.delivery(&event_id(9), false);
    b.decrypt_signal_event_with_decoder(
        &alice.trusted(),
        json.as_bytes(),
        SystemTime::now(),
        super::recovery::previous_decoder,
    )
    .unwrap();
    bob_client
        .session
        .request(Request::Ack {
            sender_void_id: ALICE.into(),
            event_id: event_id(9),
        })
        .unwrap();
    let mut future = content(10);
    future["type"] = json!("still_unknown");
    let payload = encrypt_raw(&mut a, &bob, 10, &future);
    send(&alice_client, BOB, &event_id(10), &payload, false);
    let json = bob_client.delivery(&event_id(10), false);
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
        ApplicationProtocolVersion::V1,
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
