use super::{Database, Owner, TestDir};
use crate::{
    account::RegisteredIdentity,
    messaging,
    relay::{
        Activity, Connection, Request, RequestError, Session, Worker,
        tests::{Socket, expiry, receive_json, send_json, verify_account_proof},
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::SigningKey;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{sync::mpsc, thread, time::Duration};
use tokio::{net::TcpListener, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const ALICE: &str = "101";
const BOB: &str = "202";
const WAIT: Duration = Duration::from_secs(12);
const BODY: &str = "Offline delivery using Rust Signal";
const REPLY: &str = "Reply using the established session";

fn account_key(id: &str) -> SigningKey {
    SigningKey::from_bytes(&[if id == ALICE { 41 } else { 42 }; 32])
}

fn open(dir: &TestDir, id: &str) -> Database {
    let key = account_key(id);
    let mut db = Database::open(
        &dir.0,
        Owner {
            void_id: id,
            public_key: key.verifying_key().as_bytes(),
        },
    )
    .unwrap();
    db.initialize_signal(id, &key).unwrap();
    db.recover_interrupted_sends().unwrap();
    db
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
        assert!(self.relay.connect_messaging(who.into()));
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

async fn publication(socket: &mut Socket) -> (Value, Value) {
    let frame = receive_json(socket).await;
    let bytes = STANDARD
        .decode(frame["payload_base64"].as_str().unwrap())
        .unwrap();
    let upload: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        frame,
        json!({"type":"prekeys_publish", "publication_id":frame["publication_id"],
        "payload_base64":STANDARD.encode(bytes)})
    );
    assert_eq!(upload.as_object().unwrap().len(), 9);
    assert_eq!(upload["v"], 1);
    assert_eq!(upload["device_id"], 1);
    for field in ["one_time_prekeys", "one_time_kyber_prekeys"] {
        assert_eq!(upload[field].as_array().unwrap().len(), 20);
    }
    for key in upload["one_time_prekeys"].as_array().unwrap() {
        assert_eq!(key.as_object().unwrap().len(), 2);
        assert_eq!(
            STANDARD
                .decode(key["public_key"].as_str().unwrap())
                .unwrap()
                .len(),
            33
        );
    }
    for key in upload["one_time_kyber_prekeys"]
        .as_array()
        .unwrap()
        .iter()
        .chain([&upload["signed_prekey"], &upload["kyber_last_resort"]])
    {
        assert_eq!(key.as_object().unwrap().len(), 3);
        assert_eq!(
            STANDARD
                .decode(key["signature"].as_str().unwrap())
                .unwrap()
                .len(),
            64
        );
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
    let (publication_frame, upload) = publication(&mut bob).await;
    drop(bob); // Inventory committed, response lost.

    let mut bob = authenticate(&listener, BOB).await;
    let (retry, _) = publication(&mut bob).await;
    assert_eq!(retry, publication_frame);
    published(&mut bob, &retry).await;
    bob.close(None).await.unwrap();
    drop(bob);

    let mut alice = authenticate(&listener, ALICE).await;
    let (alice_upload, _) = publication(&mut alice).await;
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
}

fn mock_relay() -> (String, thread::JoinHandle<()>) {
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
                    relay_script(TcpListener::from_std(listener).unwrap()),
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
fn encrypted_relay_round_trip_recovers_durable_operations_and_acks_only_committed_events() {
    let (endpoint, mock) = mock_relay();
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
    let event_id = alice.queue_outgoing(conversation, BODY, when).unwrap();
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
        Err(messaging::Error::Relay(RequestError::Rejected(code))) if code == "duplicate_event")
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
    let session = client.next_session();
    client.receive(&mut bob, &session).unwrap();
    assert_eq!(bob.messages(request.id).unwrap().len(), 1);
    assert_eq!(session_record(&bob, ALICE), bob_session);

    let reply_id = bob.queue_outgoing(request.id, REPLY, when + 1).unwrap();
    messaging::send(&mut bob, Some(&session), &reply_id).unwrap();
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
    mock.join().unwrap();
}
