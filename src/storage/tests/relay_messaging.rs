use super::{Database, TestDir};
use crate::{
    account::RegisteredIdentity,
    messaging,
    relay::{
        Activity, Connection, Request, RequestError, Session, Worker,
        tests::{Socket, expiry, receive_json, send_json, verify_account_proof},
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, SigningKey};
use futures_util::StreamExt;
use libsignal_protocol::IdentityKey;
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, sync::mpsc, thread, time::Duration};
use tokio::{net::TcpListener, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const ALICE: &str = "101";
const BOB: &str = "202";
const WAIT: Duration = Duration::from_secs(12);
const BODY: &str = "Offline delivery using Rust Signal";
const REPLY: &str = "Reply using the established session";
const FOLLOW_UP: &str = "Later message in the promoted chat";

fn account_key(id: &str) -> SigningKey {
    SigningKey::from_bytes(&[if id == ALICE { 41 } else { 42 }; 32])
}

fn open(dir: &TestDir, id: &str) -> Database {
    crate::account::open_enrollment_fixture(&dir.0, id, &account_key(id))
        .unwrap()
        .0
}

struct Client {
    relay: Connection,
    _worker: Worker,
    events: mpsc::Receiver<Activity>,
}

impl Client {
    fn new(endpoint: &str) -> Self {
        let (events, receiver) = mpsc::channel();
        let (relay, worker) = Connection::spawn_with_activity(
            endpoint.into(),
            |id| {
                Ok(RegisteredIdentity {
                    void_id: id.into(),
                    key: account_key(id),
                })
            },
            |_| {},
            move |event| {
                let _ = events.send(event);
            },
        )
        .unwrap();
        Self {
            relay,
            _worker: worker,
            events: receiver,
        }
    }

    fn connect(&self, who: &str) -> Session {
        assert!(self.relay.connect(who.into()));
        self.next_session()
    }

    fn next_session(&self) -> Session {
        loop {
            match self.events.recv_timeout(WAIT).unwrap() {
                Activity::Online(session) => return session,
                Activity::Offline(_) => {}
                Activity::Delivery { .. } => panic!("delivery preceded authentication"),
            }
        }
    }

    fn receive(&self, db: &mut Database, session: &Session) -> Result<(), messaging::Error> {
        let Activity::Delivery {
            generation,
            sender,
            event_id,
            json,
        } = self.events.recv_timeout(WAIT).unwrap()
        else {
            panic!("expected queued delivery");
        };
        assert_eq!(generation, session.generation);
        messaging::receive(db, session, &sender, &event_id, &json)
    }
}

async fn authenticate(listener: &TcpListener, who: &'static str) -> Socket {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut socket = accept_async(tcp).await.unwrap();
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"auth_begin", "void_id":who})
    );
    verify_account_proof(&mut socket, &account_key(who).verifying_key(), who).await;
    send_json(&mut socket, json!({"type":"auth_ok", "void_id":who})).await;
    socket
}

async fn person(socket: &mut Socket, who: &str, substitute_key: bool) {
    assert_eq!(
        receive_json(socket).await,
        json!({"type":"lookup", "void_id":who})
    );
    let key = if substitute_key {
        SigningKey::from_bytes(&[97; 32])
    } else {
        account_key(who)
    };
    send_json(
        socket,
        json!({"type":"person", "void_id":who,
        "auth_public_key":STANDARD.encode(key.verifying_key().to_bytes())}),
    )
    .await;
}

fn canonical_base64(value: &Value) -> Vec<u8> {
    let value = value.as_str().unwrap();
    let bytes = STANDARD.decode(value).unwrap();
    assert!(STANDARD.encode(&bytes) == value);
    bytes
}

async fn publication(socket: &mut Socket, who: &str) -> (Value, Value) {
    let frame = receive_json(socket).await;
    let bytes = canonical_base64(&frame["payload_base64"]);
    assert!(!bytes.is_empty() && bytes.len() <= 256 * 1024);
    let upload: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        frame
            == json!({"type":"prekeys_publish", "publication_id":frame["publication_id"],
        "payload_base64":STANDARD.encode(bytes)})
    );
    assert_eq!(upload.as_object().unwrap().len(), 9);
    assert_eq!(upload["v"], 1);
    assert_eq!(upload["device_id"], 1);
    let registration_id = upload["registration_id"].as_u64().unwrap();
    assert!((1..=16_380).contains(&registration_id));
    let identity = canonical_base64(&upload["identity_key"]);
    assert_eq!(identity.len(), 33);
    assert_eq!(identity[0], 5);
    let signer = IdentityKey::decode(&identity).unwrap();
    // The pinned Go service's IdentityMessage transcript, independent of the Rust signer.
    let transcript = format!(
        "VOID:signal-identity:v1\nhttps://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz\n{who}\n1\n{registration_id}\n{}\n",
        upload["identity_key"].as_str().unwrap(),
    );
    let signature = Signature::from_slice(&canonical_base64(&upload["account_signature"])).unwrap();
    assert!(
        account_key(who)
            .verifying_key()
            .verify_strict(transcript.as_bytes(), &signature)
            .is_ok()
    );
    for field in ["one_time_prekeys", "one_time_kyber_prekeys"] {
        assert_eq!(upload[field].as_array().unwrap().len(), 20);
    }
    let key_id = |key: &Value| {
        let id = key["key_id"].as_u64().unwrap();
        assert!((1..=16_777_214).contains(&id));
        id
    };
    let mut ec_ids = HashSet::new();
    for key in upload["one_time_prekeys"].as_array().unwrap() {
        assert_eq!(key.as_object().unwrap().len(), 2);
        assert!(ec_ids.insert(key_id(key)));
        let public_key = canonical_base64(&key["public_key"]);
        assert_eq!(public_key.len(), 33);
        assert_eq!(public_key[0], 5);
    }
    let verify_signed = |key: &Value| {
        assert_eq!(key.as_object().unwrap().len(), 3);
        let public_key = canonical_base64(&key["public_key"]);
        let signature = canonical_base64(&key["signature"]);
        assert_eq!(signature.len(), 64);
        assert!(
            signer
                .public_key()
                .verify_signature(&public_key, &signature)
        );
        public_key
    };
    let signed = &upload["signed_prekey"];
    key_id(signed);
    let signed_ec = verify_signed(signed);
    assert_eq!(signed_ec.len(), 33);
    assert_eq!(signed_ec[0], 5);
    let mut kyber_ids = HashSet::new();
    for key in upload["one_time_kyber_prekeys"]
        .as_array()
        .unwrap()
        .iter()
        .chain([&upload["kyber_last_resort"]])
    {
        assert!(kyber_ids.insert(key_id(key)));
        assert!((1..=2048).contains(&verify_signed(key).len()));
    }
    (frame, upload)
}

async fn published(socket: &mut Socket, frame: &Value) {
    send_json(
        socket,
        json!({"type":"prekeys_published", "publication_id":frame["publication_id"],
        "ec_remaining":"20", "kyber_remaining":"20"}),
    )
    .await;
}

async fn claimed(socket: &mut Socket, request: &Value, upload: &Value) {
    let mut kyber = upload["one_time_kyber_prekeys"][0].clone();
    kyber["last_resort"] = json!(false);
    let bundle = json!({
        "v": 1, "device_id": upload["device_id"], "registration_id": upload["registration_id"],
        "identity_key": upload["identity_key"], "account_signature": upload["account_signature"],
        "signed_prekey": upload["signed_prekey"], "one_time_prekey": upload["one_time_prekeys"][0],
        "kyber_prekey": kyber, "claim_expires_at": expiry(120),
    });
    send_json(
        socket,
        json!({"type":"prekey_bundle", "request_id":request["request_id"], "void_id":BOB,
        "payload_base64":STANDARD.encode(serde_json::to_vec(&bundle).unwrap())}),
    )
    .await;
}

fn delivery(send: &Value, from: &str) -> Value {
    json!({"type":"event", "event_id":send["event_id"], "from_void_id":from,
        "payload_base64":send["payload_base64"], "created_at":expiry(-1), "expires_at":expiry(900)})
}

async fn ack(socket: &mut Socket, event: &Value, from: &str, reply: bool) {
    assert_eq!(
        receive_json(socket).await,
        json!({"type":"ack", "event_id":event["event_id"], "sender_void_id":from})
    );
    if reply {
        send_json(
            socket,
            json!({"type":"ack_ok", "sender_void_id":from, "event_id":event["event_id"]}),
        )
        .await;
    }
}

async fn no_ack_until_close(socket: &mut Socket) {
    while let Some(Ok(frame)) = socket.next().await {
        assert!(
            !matches!(frame, Message::Text(_) | Message::Binary(_)),
            "unexpected application command"
        );
    }
}

async fn relay_script(listener: TcpListener) {
    let mut bob = authenticate(&listener, BOB).await;
    let (publication_frame, upload) = publication(&mut bob, BOB).await;
    drop(bob); // Inventory committed, response lost.

    let mut bob = authenticate(&listener, BOB).await;
    let (retry, _) = publication(&mut bob, BOB).await;
    assert!(retry == publication_frame);
    published(&mut bob, &retry).await;
    bob.close(None).await.unwrap();
    drop(bob);

    let mut alice = authenticate(&listener, ALICE).await;
    let (alice_upload, _) = publication(&mut alice, ALICE).await;
    published(&mut alice, &alice_upload).await;
    person(&mut alice, BOB, false).await;
    let claim = receive_json(&mut alice).await;
    assert_eq!(
        claim,
        json!({"type":"prekeys_claim", "request_id":claim["request_id"], "void_id":BOB})
    );
    drop(alice); // The claim was recorded, with the response lost.

    let mut alice = authenticate(&listener, ALICE).await;
    person(&mut alice, BOB, false).await;
    assert_eq!(receive_json(&mut alice).await, claim);
    claimed(&mut alice, &claim, &upload).await;
    let send = receive_json(&mut alice).await;
    assert_eq!(
        send,
        json!({"type":"send", "event_id":send["event_id"], "to_void_id":BOB,
        "payload_base64":send["payload_base64"]})
    );
    let queued = delivery(&send, ALICE);
    drop(alice); // Queue write committed for offline Bob, acceptance lost for Alice.

    let mut alice = authenticate(&listener, ALICE).await;
    person(&mut alice, BOB, false).await;
    assert_eq!(receive_json(&mut alice).await, send);
    send_json(
        &mut alice,
        json!({"type":"error", "code":"duplicate_event"}),
    )
    .await;
    alice.close(None).await.unwrap();
    drop(alice);

    let mut bob = authenticate(&listener, BOB).await;
    let mut malformed = queued.clone();
    malformed["payload_base64"] = json!("AA==");
    send_json(&mut bob, malformed).await;
    person(&mut bob, ALICE, false).await;
    no_ack_until_close(&mut bob).await;

    let mut bob = authenticate(&listener, BOB).await;
    send_json(&mut bob, queued.clone()).await;
    person(&mut bob, ALICE, true).await;
    no_ack_until_close(&mut bob).await;

    let mut bob = authenticate(&listener, BOB).await;
    send_json(&mut bob, queued.clone()).await;
    person(&mut bob, ALICE, false).await;
    no_ack_until_close(&mut bob).await; // The recipient transaction fails at COMMIT.

    let mut bob = authenticate(&listener, BOB).await;
    send_json(&mut bob, queued.clone()).await;
    person(&mut bob, ALICE, false).await;
    ack(&mut bob, &queued, ALICE, false).await;
    drop(bob); // ACK is lost before the relay durably removes the event.

    let mut bob = authenticate(&listener, BOB).await;
    send_json(&mut bob, queued.clone()).await;
    person(&mut bob, ALICE, false).await;
    ack(&mut bob, &queued, ALICE, true).await;
    person(&mut bob, ALICE, false).await;
    let reply = receive_json(&mut bob).await;
    assert_eq!(reply["type"], "send");
    assert_eq!(reply["to_void_id"], ALICE);
    let reply_event = delivery(&reply, BOB);
    send_json(
        &mut bob,
        json!({"type":"accepted", "event_id":reply["event_id"],
        "created_at":reply_event["created_at"], "expires_at":reply_event["expires_at"]}),
    )
    .await;

    let mut alice = authenticate(&listener, ALICE).await;
    assert_eq!(
        receive_json(&mut alice).await,
        json!({"type":"lookup", "void_id":BOB})
    );
    send_json(&mut alice, reply_event.clone()).await; // Event interleaved with an outstanding lookup.
    send_json(
        &mut alice,
        json!({"type":"person", "void_id":BOB,
        "auth_public_key":STANDARD.encode(account_key(BOB).verifying_key().to_bytes())}),
    )
    .await;
    person(&mut alice, BOB, false).await;
    ack(&mut alice, &reply_event, BOB, true).await;

    person(&mut alice, BOB, false).await;
    let follow_up = receive_json(&mut alice).await;
    assert_eq!(follow_up["type"], "send");
    assert_eq!(follow_up["to_void_id"], BOB);
    let next_event = delivery(&follow_up, ALICE);
    send_json(
        &mut alice,
        json!({"type":"accepted", "event_id":follow_up["event_id"],
            "created_at":next_event["created_at"], "expires_at":next_event["expires_at"]}),
    )
    .await;
    send_json(&mut bob, next_event.clone()).await;
    person(&mut bob, ALICE, false).await;
    ack(&mut bob, &next_event, ALICE, true).await;
}

fn mock_relay<F: Future<Output = ()> + 'static>(
    script: impl FnOnce(TcpListener) -> F + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                timeout(
                    Duration::from_secs(45),
                    script(TcpListener::from_std(listener).unwrap()),
                )
                .await
                .expect("mock messaging relay timed out");
            });
    });
    (format!("ws://{address}/ws"), worker)
}

fn count(db: &Database, sql: &str) -> i64 {
    db.connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn session_record(db: &Database, peer: &str) -> Vec<u8> {
    db.connection
        .query_row(
            "SELECT record FROM signal_sessions WHERE peer_void_id = ?1",
            [peer],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn replacing_the_signal_store_for_an_existing_account_reproduces_a_publication_conflict() {
    let (endpoint, mock) = mock_relay(|listener| async move {
        let mut first = authenticate(&listener, BOB).await;
        let (frame, original) = publication(&mut first, BOB).await;
        published(&mut first, &frame).await;

        let mut replacement = authenticate(&listener, BOB).await;
        let (frame, new_upload) = publication(&mut replacement, BOB).await;
        // The pinned Go inventory rejects a changed immutable identity, even with
        // a valid account signature and a fresh publication UUID.
        assert!(original["identity_key"] != new_upload["identity_key"]);
        send_json(&mut replacement, json!({
            "type":"prekeys_error", "request_id":frame["publication_id"], "code":"prekeys_conflict"
        })).await;
    });
    let original = TestDir::new();
    let mut db = open(&original, BOB);
    let client = Client::new(&endpoint);
    messaging::publish(&mut db, &client.connect(BOB)).unwrap();
    drop(client);
    db.close().unwrap();
    let mut db = open(&original, BOB);
    assert!(db.prepare_publication().unwrap().is_none());
    assert_eq!(
        count(&db, "SELECT count(*) FROM signal_prekeys WHERE published"),
        42
    );

    let replacement = TestDir::new();
    let mut db = open(&replacement, BOB);
    let saved = db.prepare_publication().unwrap().unwrap();
    let client = Client::new(&endpoint);
    let error = messaging::publish(&mut db, &client.connect(BOB)).unwrap_err();
    assert!(
        matches!(&error, messaging::Error::Relay(RequestError::Rejected {
        command: "prekeys_publish", code
    }) if code == "prekeys_conflict")
    );
    let (notice, retryable) = messaging::publication_failure(&error);
    assert!(notice.contains("original Signal store"));
    assert!(!retryable);
    db.close().unwrap();
    let mut db = open(&replacement, BOB);
    let persisted = db.prepare_publication().unwrap().unwrap();
    assert!(saved.id == persisted.id && saved.payload_base64 == persisted.payload_base64);
    assert_eq!(
        count(&db, "SELECT count(*) FROM signal_prekeys WHERE published"),
        0
    );
    mock.join().unwrap();
}

#[test]
fn encrypted_relay_round_trip_recovers_durable_operations_and_acks_only_committed_events() {
    let (endpoint, mock) = mock_relay(relay_script);
    let alice_dir = TestDir::new();
    let bob_dir = TestDir::new();
    let mut bob = open(&bob_dir, BOB);

    let client = Client::new(&endpoint);
    let session = client.connect(BOB);
    assert!(messaging::publish(&mut bob, &session).is_err());
    assert_eq!(
        count(&bob, "SELECT count(*) FROM signal_prekeys WHERE published"),
        0
    );
    bob.close().unwrap();
    let mut bob = open(&bob_dir, BOB);
    let replacement = client.next_session();
    assert_ne!(session.generation, replacement.generation);
    assert!(matches!(
        session.request(Request::Lookup {
            void_id: ALICE.into()
        }),
        Err(RequestError::Interrupted)
    ));
    messaging::publish(&mut bob, &replacement).unwrap();
    assert_eq!(
        count(&bob, "SELECT count(*) FROM signal_prekeys WHERE published"),
        42
    );
    drop(client);

    let mut alice = open(&alice_dir, ALICE);
    let client = Client::new(&endpoint);
    let when = chrono::Utc::now().timestamp_millis();
    let conversation = alice.create_conversation(BOB, when).unwrap();
    alice.save_draft(conversation, BODY).unwrap();
    let (event_id, promoted) = alice.queue_outgoing(conversation, BODY, when).unwrap();
    assert!(!promoted);
    assert_eq!(
        alice.conversation_timeline(conversation).unwrap()[0].status,
        "Pending"
    );
    let session = client.connect(ALICE);
    messaging::publish(&mut alice, &session).unwrap();
    assert!(messaging::send(&mut alice, Some(&session), &event_id).is_err());
    assert!(alice.committed_outgoing(&event_id).unwrap().is_none());
    assert_eq!(alice.outgoing_operation(&event_id).unwrap().body, BODY);
    alice.close().unwrap();
    let mut alice = open(&alice_dir, ALICE);

    let session = client.next_session();
    assert!(messaging::send(&mut alice, Some(&session), &event_id).is_err());
    let payload = alice
        .committed_outgoing(&event_id)
        .unwrap()
        .unwrap()
        .payload;
    assert_ne!(payload, BODY.as_bytes());
    assert_eq!(
        alice.outgoing_operation(&event_id).unwrap().state,
        "unconfirmed"
    );
    let alice_session = session_record(&alice, BOB);
    alice.close().unwrap();
    let mut alice = open(&alice_dir, ALICE);
    let replacement = client.next_session();
    assert_eq!(
        alice.outgoing_operation(&event_id).unwrap().state,
        "unconfirmed"
    );
    assert!(
        matches!(messaging::send(&mut alice, Some(&replacement), &event_id),
        Err(messaging::Error::Relay(RequestError::Rejected { command: "send", code })) if code == "duplicate_event")
    );
    assert_eq!(
        alice.outgoing_operation(&event_id).unwrap().state,
        "unconfirmed"
    );
    assert_eq!(
        alice
            .committed_outgoing(&event_id)
            .unwrap()
            .unwrap()
            .payload,
        payload
    );
    assert_eq!(session_record(&alice, BOB), alice_session);
    drop(client);

    for _ in 0..2 {
        let client = Client::new(&endpoint);
        let session = client.connect(BOB);
        assert!(client.receive(&mut bob, &session).is_err());
        assert_eq!(count(&bob, "SELECT count(*) FROM inbox"), 0);
    }

    bob.connection.execute_batch(
        "CREATE TABLE test_ack_commit (missing TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER test_inbox_commit AFTER INSERT ON inbox
         BEGIN INSERT INTO test_ack_commit VALUES ('missing-test-message'); END;",
    ).unwrap();
    let client = Client::new(&endpoint);
    let session = client.connect(BOB);
    assert!(matches!(
        client.receive(&mut bob, &session),
        Err(messaging::Error::Storage(_))
    ));
    assert_eq!(count(&bob, "SELECT count(*) FROM inbox"), 0);
    assert_eq!(count(&bob, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&bob, "SELECT count(*) FROM signal_prekeys"), 42);
    bob.connection
        .execute_batch("DROP TRIGGER test_inbox_commit; DROP TABLE test_ack_commit;")
        .unwrap();
    drop(client);

    let client = Client::new(&endpoint);
    let session = client.connect(BOB);
    assert!(matches!(
        client.receive(&mut bob, &session),
        Err(messaging::Error::Relay(RequestError::Interrupted))
    ));
    let request = bob.conversations(true).unwrap().pop().unwrap();
    assert!(request.is_request);
    assert_eq!(bob.messages(request.id).unwrap()[0].body, BODY);
    let bob_session = session_record(&bob, ALICE);
    bob.close().unwrap();
    let mut bob = open(&bob_dir, BOB);
    let bob_relay = client.next_session();
    client.receive(&mut bob, &bob_relay).unwrap();
    assert_eq!(bob.messages(request.id).unwrap().len(), 1);
    assert_eq!(session_record(&bob, ALICE), bob_session);

    assert!(bob.conversation(request.id).unwrap().is_request);
    let (reply_id, promoted) = bob.queue_outgoing(request.id, REPLY, when + 1).unwrap();
    assert!(promoted);
    assert!(bob.conversations(true).unwrap().is_empty());
    assert_eq!(bob.conversations(false).unwrap()[0].id, request.id);
    assert_eq!(session_record(&bob, ALICE), bob_session);
    assert_eq!(bob.outgoing_operation(&reply_id).unwrap().state, "pending");
    messaging::send(&mut bob, Some(&bob_relay), &reply_id).unwrap();
    assert_eq!(bob.outgoing_operation(&reply_id).unwrap().state, "sent");
    assert_eq!(
        bob.conversation_timeline(request.id).unwrap()[1].status,
        "Sent to relay"
    );
    let alice_client = Client::new(&endpoint);
    let session = alice_client.connect(ALICE);
    assert!(
        session
            .request(Request::Lookup {
                void_id: BOB.into()
            })
            .is_ok()
    );
    alice_client.receive(&mut alice, &session).unwrap();
    assert_eq!(alice.messages(conversation).unwrap()[1].body, REPLY);
    assert_eq!(
        alice.outgoing_operation(&event_id).unwrap().state,
        "unconfirmed"
    );
    bob.close().unwrap();
    let mut bob = open(&bob_dir, BOB);
    assert!(!bob.conversation(request.id).unwrap().is_request);
    let (follow_up, promoted) = alice
        .queue_outgoing(conversation, FOLLOW_UP, when + 2)
        .unwrap();
    assert!(!promoted);
    messaging::send(&mut alice, Some(&session), &follow_up).unwrap();
    client.receive(&mut bob, &bob_relay).unwrap();
    assert!(bob.conversations(true).unwrap().is_empty());
    let chats = bob.conversations(false).unwrap();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].id, request.id);
    let messages = bob.messages(request.id).unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].body, FOLLOW_UP);
    mock.join().unwrap();
}
