use super::{
    Activity, Connection, ConnectionStatus, Request, RequestError, SessionIdentity,
    reconnect_delay, serve_once,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{cell::RefCell, future::Future, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

const AUDIENCE: &str = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz";
const VOID_ID: &str = "123";
const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn mandatory_update_ends_worker_without_online_commands_or_reconnects() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (status, mut statuses) = tokio::sync::mpsc::unbounded_channel();
    let (activity, mut activities) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint.clone(),
        |_| Ok(identity()),
        move |value| {
            let _ = status.send(value);
        },
        move |value| {
            let _ = activity.send(value);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    assert_status(&mut statuses, ConnectionStatus::Connecting).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    send_json(&mut socket, json!({"type":"auth_ok","void_id":VOID_ID})).await;
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"protocol","version":"1"})
    );
    send_json(
        &mut socket,
        json!({"type":"error","code":"update_required","min_protocol_version":"2"}),
    )
    .await;
    assert_status(
        &mut statuses,
        ConnectionStatus::UpdateRequired(super::ApplicationProtocolVersion::V2),
    )
    .await;
    assert!(matches!(
        activities.recv().await,
        Some(Activity::Offline(_))
    ));
    assert!(activities.recv().await.is_none());
    assert!(
        !relay.connect(VOID_ID.into()),
        "manual reconnect cannot revive incompatible worker"
    );
    expect_no_command(&mut socket).await;
    assert!(
        timeout(Duration::from_millis(2300), listener.accept())
            .await
            .is_err()
    );
    worker.finish();
    // A new process/worker does not persist the terminal state. Minimum V1
    // can authenticate and negotiate normally on the same endpoint.
    let (relay, worker) =
        Connection::spawn_with_activity(endpoint, |_| Ok(identity()), |_| {}, |_| {}).unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    finish_authentication(&mut socket, VOID_ID).await;
    worker.finish();
}

#[tokio::test]
async fn contradictory_update_minimum_is_not_trusted() {
    let (endpoint, server) = mock_relay(|mut socket| async move {
        verify_proof(&mut socket, &identity().key.verifying_key()).await;
        send_json(&mut socket, json!({"type":"auth_ok","void_id":VOID_ID})).await;
        assert_eq!(
            receive_json(&mut socket).await,
            json!({"type":"protocol","version":"1"})
        );
        send_json(
            &mut socket,
            json!({"type":"error","code":"update_required","min_protocol_version":"1"}),
        )
        .await;
        expect_no_command(&mut socket).await;
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity(), Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
}
pub(crate) type Socket = WebSocketStream<TcpStream>;

#[test]
fn version_two_override_is_test_only_and_rejects_remote_or_credentialed_urls() {
    assert_eq!(
        super::SUPPORTED_APPLICATION_PROTOCOL,
        super::ApplicationProtocolVersion::V1
    );
    for endpoint in [
        "wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws",
        "ws://example.invalid/ws",
        "wss://127.0.0.1/ws",
        "ws://user@127.0.0.1/ws",
    ] {
        assert!(
            Connection::spawn_with_test_protocol(
                endpoint.into(),
                AUDIENCE.into(),
                super::ApplicationProtocolVersion::V2,
                |_| Ok(identity()),
                |_| {},
                |_| {}
            )
            .is_err()
        );
    }
    assert!(
        Connection::spawn_with_test_protocol(
            "ws://127.0.0.1:1/ws".into(),
            AUDIENCE.into(),
            super::ApplicationProtocolVersion::parse("3").unwrap(),
            |_| Ok(identity()),
            |_| {},
            |_| {}
        )
        .is_err()
    );
}

#[tokio::test]
async fn isolated_version_two_negotiation_dispatches_versioned_delivery_without_implicit_ack() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (tx, mut activity) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_test_protocol(
        endpoint,
        AUDIENCE.into(),
        super::ApplicationProtocolVersion::V2,
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = tx.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    send_json(&mut socket, json!({"type":"auth_ok","void_id":VOID_ID})).await;
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"protocol","version":"2"})
    );
    assert!(activity.try_recv().is_err());
    send_json(&mut socket, json!({"type":"protocol_ok","version":"2"})).await;
    let Some(Activity::Online(session)) =
        tokio::time::timeout(WAIT, activity.recv()).await.unwrap()
    else {
        panic!("missing negotiated session");
    };
    let event = json!({"type":"event_versioned","event_id":"00000000-0000-4000-8000-000000000001",
        "from_void_id":"456","payload_base64":"AA==","created_at":expiry(-1),"expires_at":expiry(60),"min_protocol_version":"2"});
    send_json(&mut socket, event.clone()).await;
    let Some(Activity::Delivery {
        generation, json, ..
    }) = tokio::time::timeout(WAIT, activity.recv()).await.unwrap()
    else {
        panic!("missing versioned delivery");
    };
    assert_eq!(generation, session.generation);
    assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), event);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.next())
            .await
            .is_err(),
        "receiving a frame alone must not send ACK"
    );
    worker.finish();
}

fn identity() -> SessionIdentity {
    SessionIdentity {
        void_id: VOID_ID.into(),
        key: SigningKey::from_bytes(&[7; 32]),
    }
}

pub(crate) async fn receive_json(socket: &mut Socket) -> Value {
    let frame = socket.next().await.unwrap().unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

pub(crate) async fn send_json(socket: &mut Socket, body: Value) {
    socket
        .send(Message::Text(body.to_string().into()))
        .await
        .unwrap();
}

pub(crate) async fn finish_authentication(socket: &mut Socket, void_id: &str) {
    send_json(socket, json!({"type":"auth_ok", "void_id":void_id})).await;
    assert_eq!(
        receive_json(socket).await,
        json!({"type":"protocol", "version":"1"})
    );
    send_json(socket, json!({"type":"protocol_ok", "version":"1"})).await;
}

// Accept on loopback only and require the exact first client command.
async fn mock_relay<F>(
    script: impl FnOnce(Socket) -> F + Send + 'static,
) -> (String, tokio::task::JoinHandle<()>)
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        timeout(WAIT, async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(tcp).await.unwrap();
            assert_eq!(
                receive_json(&mut socket).await,
                json!({"type": "auth_begin", "void_id": VOID_ID})
            );
            script(socket).await;
        })
        .await
        .expect("mock relay did not finish");
    });
    (endpoint, server)
}

struct Challenge {
    id: String,
    nonce: String,
    expires_at: String,
    void_id: &'static str,
    public_key: String,
    audience: &'static str,
}

impl Challenge {
    fn new(public_key: VerifyingKey) -> Self {
        Self {
            id: URL_SAFE_NO_PAD.encode([17; 32]),
            nonce: URL_SAFE_NO_PAD.encode([29; 32]),
            expires_at: expiry(20),
            void_id: VOID_ID,
            public_key: STANDARD.encode(public_key.as_bytes()),
            audience: AUDIENCE,
        }
    }

    fn message(&self) -> String {
        format!(
            "VOID-NOTHINGNESS:authenticate:v2\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.id,
            self.void_id,
            self.public_key,
            self.expires_at,
            STANDARD.encode(self.audience),
            self.nonce
        )
    }

    fn frame(&self, message: String) -> Value {
        json!({
            "type": "auth_challenge", "challenge_id": self.id,
            "message": STANDARD.encode(message), "expires_at": self.expires_at
        })
    }
}

pub(crate) fn expiry(seconds: i64) -> String {
    let now: chrono::DateTime<Utc> = std::time::SystemTime::now().into();
    (now + chrono::Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

async fn verify_proof(socket: &mut Socket, key: &VerifyingKey) {
    verify_account_proof(socket, key, VOID_ID).await;
}

pub(crate) async fn verify_account_proof(
    socket: &mut Socket,
    key: &VerifyingKey,
    void_id: &'static str,
) {
    let challenge = Challenge {
        void_id,
        ..Challenge::new(*key)
    };
    let message = challenge.message();
    send_json(socket, challenge.frame(message.clone())).await;
    let proof = receive_json(socket).await;
    let signature = proof["signature"].as_str().unwrap();
    assert_eq!(
        proof,
        json!({
            "type": "auth_proof", "challenge_id": challenge.id, "signature": signature
        })
    );
    let signature = Signature::from_slice(&STANDARD.decode(signature).unwrap()).unwrap();
    key.verify_strict(message.as_bytes(), &signature).unwrap();
}

async fn expect_no_command(socket: &mut Socket) {
    while let Some(Ok(frame)) = socket.next().await {
        assert!(
            !matches!(frame, Message::Text(_) | Message::Binary(_)),
            "client sent an unexpected application frame"
        );
    }
}

async fn connect(
    endpoint: &str,
    identity: &SessionIdentity,
    handshake_timeout: Duration,
) -> (ConnectionStatus, Vec<ConnectionStatus>) {
    let status = RefCell::new(Vec::new());
    let end = timeout(
        WAIT,
        serve_once(endpoint, AUDIENCE, identity, handshake_timeout, |value| {
            status.borrow_mut().push(value);
        }),
    )
    .await
    .expect("client did not finish");
    (end, status.into_inner())
}

#[tokio::test]
async fn mismatched_prekey_error_interrupts_the_session() {
    let (endpoint, server) = mock_relay(|mut socket| async move {
        verify_proof(&mut socket, &identity().key.verifying_key()).await;
        finish_authentication(&mut socket, VOID_ID).await;
        let request = receive_json(&mut socket).await;
        assert_eq!(request["type"], "prekeys_claim");
        send_json(
            &mut socket,
            json!({"type":"prekeys_error",
            "request_id":"00000000-0000-4000-8000-000000000002", "code":"prekeys_unavailable"}),
        )
        .await;
        expect_no_command(&mut socket).await;
    })
    .await;
    let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint,
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = events.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let Some(Activity::Online(session)) = timeout(WAIT, rx.recv()).await.unwrap() else {
        panic!("missing authenticated session");
    };
    let result = tokio::task::spawn_blocking(move || {
        session.request(Request::PrekeysClaim {
            request_id: "00000000-0000-4000-8000-000000000001".into(),
            void_id: "456".into(),
        })
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(RequestError::Interrupted)));
    assert!(matches!(
        timeout(WAIT, rx.recv()).await.unwrap(),
        Some(Activity::Offline(_))
    ));
    worker.finish();
    server.await.unwrap();
}

pub(crate) async fn next_attempt(listener: &TcpListener) -> Socket {
    timeout(Duration::from_secs(8), async {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(tcp).await.unwrap();
        assert_eq!(
            receive_json(&mut socket).await,
            json!({"type":"auth_begin", "void_id":VOID_ID})
        );
        socket
    })
    .await
    .unwrap()
}

async fn assert_status(
    changes: &mut tokio::sync::mpsc::UnboundedReceiver<ConnectionStatus>,
    expected: ConnectionStatus,
) {
    assert_eq!(timeout(WAIT, changes.recv()).await.unwrap(), Some(expected));
}

#[tokio::test]
async fn manual_reconnect_severs_in_flight_requests_before_the_next_session() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (events, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint,
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = events.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut old_socket = next_attempt(&listener).await;
    verify_proof(&mut old_socket, &identity().key.verifying_key()).await;
    finish_authentication(&mut old_socket, VOID_ID).await;
    let Some(Activity::Online(old_session)) = timeout(WAIT, changes.recv()).await.unwrap() else {
        panic!("missing first authenticated session");
    };
    let pending_session = old_session.clone();
    let pending = tokio::task::spawn_blocking(move || {
        pending_session.request(Request::Lookup {
            void_id: "456".into(),
        })
    });
    assert_eq!(
        receive_json(&mut old_socket).await,
        json!({"type":"lookup", "void_id":"456"})
    );
    assert!(relay.connect(VOID_ID.into()));
    assert!(matches!(
        timeout(WAIT, pending).await.unwrap().unwrap(),
        Err(RequestError::Interrupted)
    ));
    assert!(matches!(timeout(WAIT, changes.recv()).await.unwrap(),
        Some(Activity::Offline(generation)) if generation == old_session.generation));
    assert!(!old_session.is_active());
    let late = json!({"type":"person", "void_id":"456",
        "auth_public_key":STANDARD.encode(identity().key.verifying_key().to_bytes())});
    let _ = old_socket
        .send(Message::Text(late.to_string().into()))
        .await;
    timeout(WAIT, expect_no_command(&mut old_socket))
        .await
        .unwrap();

    let mut new_socket = next_attempt(&listener).await;
    verify_proof(&mut new_socket, &identity().key.verifying_key()).await;
    finish_authentication(&mut new_socket, VOID_ID).await;
    let Some(Activity::Online(new_session)) = timeout(WAIT, changes.recv()).await.unwrap() else {
        panic!("missing replacement authenticated session");
    };
    assert_ne!(old_session.generation, new_session.generation);
    let pending = tokio::task::spawn_blocking(move || {
        new_session.request(Request::Lookup {
            void_id: "456".into(),
        })
    });
    assert_eq!(
        receive_json(&mut new_socket).await,
        json!({"type":"lookup", "void_id":"456"})
    );
    send_json(&mut new_socket, late).await;
    assert!(matches!(timeout(WAIT, pending).await.unwrap().unwrap(),
        Ok(super::Reply::Person { void_id, .. }) if void_id == "456"));
    worker.finish();
}

#[test]
fn exponential_reconnect_delay_includes_jitter_even_at_the_cap() {
    for (step, base_seconds) in [2, 4, 8, 16, 32, 48, 48].into_iter().enumerate() {
        let base = Duration::from_secs(base_seconds);
        assert_eq!(reconnect_delay(step as u32, 0), base);
        assert_eq!(reconnect_delay(step as u32, 25), base + base / 4);
    }
    assert_eq!(reconnect_delay(u32::MAX, 25), Duration::from_secs(60));
}

#[tokio::test]
async fn reauthenticates_after_disconnect_with_backoff_and_stops_during_the_next_wait() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (status, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn(
        endpoint,
        |_| Ok(identity()),
        move |value| {
            let _ = status.send(value);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));

    let mut disconnected_at = tokio::time::Instant::now();
    for minimum in [0, 2, 4] {
        let mut socket = next_attempt(&listener).await;
        assert!(disconnected_at.elapsed() >= Duration::from_secs(minimum));
        assert_status(&mut changes, ConnectionStatus::Connecting).await;
        verify_proof(&mut socket, &identity().key.verifying_key()).await;
        finish_authentication(&mut socket, VOID_ID).await;
        assert_status(&mut changes, ConnectionStatus::Connected).await;
        disconnected_at = tokio::time::Instant::now();
        drop(socket);
        assert_status(&mut changes, ConnectionStatus::Reconnecting).await;
    }

    timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || worker.finish()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!relay.connect(VOID_ID.into()));
    assert_eq!(changes.recv().await, None);
}

#[tokio::test]
async fn transient_auth_unavailability_retries_but_rejected_proof_waits_for_manual_reconnect() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (status, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn(
        endpoint,
        |_| Ok(identity()),
        move |value| {
            let _ = status.send(value);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut first = next_attempt(&listener).await;
    assert_status(&mut changes, ConnectionStatus::Connecting).await;
    send_json(&mut first, json!({"type":"error", "code":"unavailable"})).await;
    assert_status(&mut changes, ConnectionStatus::Reconnecting).await;
    timeout(WAIT, expect_no_command(&mut first)).await.unwrap();

    let mut second = next_attempt(&listener).await;
    assert_status(&mut changes, ConnectionStatus::Connecting).await;
    verify_proof(&mut second, &identity().key.verifying_key()).await;
    send_json(
        &mut second,
        json!({"type":"error", "code":"authentication_failed"}),
    )
    .await;
    assert_status(&mut changes, ConnectionStatus::AuthenticationFailed).await;
    timeout(WAIT, expect_no_command(&mut second)).await.unwrap();
    assert!(
        timeout(Duration::from_secs(6), listener.accept())
            .await
            .is_err()
    );

    assert!(relay.connect(VOID_ID.into()));
    let mut manual = next_attempt(&listener).await;
    assert_status(&mut changes, ConnectionStatus::Connecting).await;
    verify_proof(&mut manual, &identity().key.verifying_key()).await;
    finish_authentication(&mut manual, VOID_ID).await;
    assert_status(&mut changes, ConnectionStatus::Connected).await;
    worker.finish();
    timeout(WAIT, expect_no_command(&mut manual)).await.unwrap();
}

#[tokio::test]
async fn relay_worker_stays_idle_until_given_a_registered_account() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (status, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn(
        endpoint,
        |_| Ok(identity()),
        move |value| {
            let _ = status.send(value);
        },
    )
    .unwrap();
    assert!(
        timeout(Duration::from_millis(150), listener.accept())
            .await
            .is_err()
    );
    assert!(changes.try_recv().is_err());
    worker.finish();
    assert!(!relay.connect(VOID_ID.into()));
}

#[tokio::test]
async fn authenticates_and_keeps_the_connection_without_handling_messages() {
    let identity = identity();
    let verifying_key = identity.key.verifying_key();
    let (endpoint, server) = mock_relay(move |mut socket| async move {
        verify_proof(&mut socket, &verifying_key).await;
        finish_authentication(&mut socket, VOID_ID).await;
        // The relay may send queued data immediately. Only the Pong should come back.
        send_json(
            &mut socket,
            json!({
                "type": "event", "event_id": "12345678-1234-1234-1234-123456789abc",
                "from_void_id": "456", "payload_base64": "AA==",
                "created_at": "2026-11-14T00:00:00Z", "expires_at": "2026-11-14T00:10:00Z"
            }),
        )
        .await;
        let payload = b"relay probe".to_vec();
        socket
            .send(Message::Ping(payload.clone().into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Pong(payload.into())
        );
        socket.close(None).await.unwrap();
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert_eq!(seen, vec![ConnectionStatus::Connected]);
}

#[tokio::test]
async fn legacy_delivery_waits_for_protocol_agreement_and_retains_ack_tracking() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (status, mut statuses) = tokio::sync::mpsc::unbounded_channel();
    let (activity, mut activities) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint,
        |_| Ok(identity()),
        move |value| {
            let _ = status.send(value);
        },
        move |event| {
            let _ = activity.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    assert_status(&mut statuses, ConnectionStatus::Connecting).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    send_json(&mut socket, json!({"type":"auth_ok", "void_id":VOID_ID})).await;
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"protocol", "version":"1"})
    );
    let event_id = "12345678-1234-1234-1234-123456789abc";
    let event = json!({
        "type":"event", "event_id":event_id, "from_void_id":"456",
        "payload_base64":"AA==", "created_at":expiry(-1), "expires_at":expiry(60)
    });
    // A version-1 queue head may arrive immediately after authentication, before
    // the relay's protocol_ok. It must not be lost or exposed as an online session.
    send_json(&mut socket, event.clone()).await;
    assert!(
        timeout(Duration::from_millis(100), statuses.recv())
            .await
            .is_err()
    );
    assert!(activities.try_recv().is_err());
    assert!(
        timeout(Duration::from_millis(50), socket.next())
            .await
            .is_err(),
        "queued ciphertext must not be ACKed before it has been processed"
    );
    send_json(&mut socket, json!({"type":"protocol_ok", "version":"1"})).await;
    assert_status(&mut statuses, ConnectionStatus::Connected).await;
    let Some(Activity::Online(session)) = timeout(WAIT, activities.recv()).await.unwrap() else {
        panic!("missing protocol-ready session");
    };
    let Some(Activity::Delivery {
        generation,
        sender,
        event_id: delivered_id,
        json,
    }) = timeout(WAIT, activities.recv()).await.unwrap()
    else {
        panic!("the pre-negotiation delivery was not preserved");
    };
    assert_eq!(generation, session.generation);
    assert_eq!(sender, "456");
    assert_eq!(delivered_id, event_id);
    assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), event);
    let ack = tokio::task::spawn_blocking(move || {
        session.request(Request::Ack {
            event_id: event_id.into(),
            sender_void_id: "456".into(),
        })
    });
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"ack", "event_id":event_id, "sender_void_id":"456"})
    );
    send_json(
        &mut socket,
        json!({"type":"ack_ok", "event_id":event_id, "sender_void_id":"456"}),
    )
    .await;
    assert!(matches!(
        timeout(WAIT, ack).await.unwrap().unwrap(),
        Ok(super::Reply::Acked { .. })
    ));
    worker.finish();
}

#[tokio::test]
async fn higher_version_delivery_is_neither_processed_nor_acknowledged_at_version_one() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (activity, mut activities) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint,
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = activity.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    finish_authentication(&mut socket, VOID_ID).await;
    let Some(Activity::Online(session)) = timeout(WAIT, activities.recv()).await.unwrap() else {
        panic!("missing protocol-ready session");
    };
    send_json(
        &mut socket,
        json!({
            "type":"event_versioned", "min_protocol_version":"2",
            "event_id":"12345678-1234-1234-1234-123456789abc", "from_void_id":"456",
            "payload_base64":"AA==", "created_at":expiry(-1), "expires_at":expiry(60)
        }),
    )
    .await;
    assert!(matches!(
        timeout(WAIT, activities.recv()).await.unwrap(),
        Some(Activity::Offline(generation)) if generation == session.generation
    ));
    timeout(WAIT, expect_no_command(&mut socket)).await.unwrap();
    assert!(activities.try_recv().is_err());
    worker.finish();
}

#[tokio::test]
async fn rejects_mismatched_or_malformed_protocol_agreement_without_connected_status() {
    for frame in [
        json!({"type":"protocol_ok", "version":"2"}),
        json!({"type":"protocol_ok", "version":"01"}),
        json!({"type":"protocol_ok", "version":1}),
        json!({"type":"protocol_ok", "version":"1", "extra":true}),
    ] {
        let (endpoint, server) = mock_relay(move |mut socket| async move {
            verify_proof(&mut socket, &identity().key.verifying_key()).await;
            send_json(&mut socket, json!({"type":"auth_ok", "void_id":VOID_ID})).await;
            assert_eq!(
                receive_json(&mut socket).await,
                json!({"type":"protocol", "version":"1"})
            );
            send_json(&mut socket, frame).await;
            expect_no_command(&mut socket).await;
        })
        .await;
        let (end, seen) = connect(&endpoint, &identity(), Duration::from_secs(2)).await;
        server.await.unwrap();
        assert_eq!(end, ConnectionStatus::Disconnected);
        assert!(seen.is_empty());
    }
}

#[tokio::test]
async fn rejects_unsolicited_protocol_agreement_after_negotiation() {
    let (endpoint, server) = mock_relay(|mut socket| async move {
        verify_proof(&mut socket, &identity().key.verifying_key()).await;
        finish_authentication(&mut socket, VOID_ID).await;
        send_json(&mut socket, json!({"type":"protocol_ok", "version":"1"})).await;
        expect_no_command(&mut socket).await;
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity(), Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert_eq!(seen, vec![ConnectionStatus::Connected]);
}

#[tokio::test]
async fn prepared_versioned_send_preserves_the_accepted_reply_contract() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (activity, mut activities) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_with_activity(
        endpoint,
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = activity.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    finish_authentication(&mut socket, VOID_ID).await;
    let Some(Activity::Online(session)) = timeout(WAIT, activities.recv()).await.unwrap() else {
        panic!("missing protocol-ready session");
    };
    let event_id = "12345678-1234-1234-1234-123456789abc";
    let result = tokio::task::spawn_blocking(move || {
        session.request(Request::SendVersioned {
            event_id: event_id.into(),
            to_void_id: "456".into(),
            payload_base64: "AA==".into(),
            min_protocol_version: super::ApplicationProtocolVersion::V1,
        })
    });
    assert_eq!(
        receive_json(&mut socket).await,
        json!({
            "type":"send_versioned", "event_id":event_id, "to_void_id":"456",
            "payload_base64":"AA==", "min_protocol_version":"1"
        })
    );
    send_json(
        &mut socket,
        json!({"type":"accepted", "event_id":event_id,
        "created_at":expiry(0), "expires_at":expiry(60)}),
    )
    .await;
    assert!(matches!(
        timeout(WAIT, result).await.unwrap().unwrap(),
        Ok(super::Reply::Accepted { event_id: accepted_id }) if accepted_id == event_id
    ));
    worker.finish();
}

#[tokio::test]
async fn protocol_negotiation_times_out_without_reporting_connected() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let client = tokio::spawn(async move {
        let statuses = std::sync::Mutex::new(Vec::new());
        let end = timeout(
            Duration::from_secs(12),
            serve_once(
                &endpoint,
                AUDIENCE,
                &identity(),
                Duration::from_secs(2),
                |value| statuses.lock().unwrap().push(value),
            ),
        )
        .await
        .expect("protocol negotiation exceeded its response deadline");
        (end, statuses.into_inner().unwrap())
    });
    let mut socket = next_attempt(&listener).await;
    verify_proof(&mut socket, &identity().key.verifying_key()).await;
    send_json(&mut socket, json!({"type":"auth_ok", "void_id":VOID_ID})).await;
    assert_eq!(
        receive_json(&mut socket).await,
        json!({"type":"protocol", "version":"1"})
    );
    let waiting_since = tokio::time::Instant::now();
    let (end, seen) = client.await.unwrap();
    assert!(waiting_since.elapsed() >= Duration::from_secs(7));
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
    timeout(WAIT, expect_no_command(&mut socket)).await.unwrap();
}

#[tokio::test]
async fn disconnect_during_protocol_negotiation_does_not_report_connected() {
    let (endpoint, server) = mock_relay(|mut socket| async move {
        verify_proof(&mut socket, &identity().key.verifying_key()).await;
        send_json(&mut socket, json!({"type":"auth_ok", "void_id":VOID_ID})).await;
        assert_eq!(
            receive_json(&mut socket).await,
            json!({"type":"protocol", "version":"1"})
        );
        socket.close(None).await.unwrap();
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity(), Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
}

#[tokio::test]
async fn rejects_invalid_or_mismatched_challenges_without_a_proof() {
    let identity = identity();
    let verifying_key = identity.key.verifying_key();
    for variant in [
        "audience",
        "void_id",
        "public_key",
        "expired",
        "future",
        "challenge_id",
        "domain",
        "base64",
        "nonce",
        "extra_field",
    ] {
        let (endpoint, server) = mock_relay(move |mut socket| async move {
            let mut challenge = Challenge::new(verifying_key);
            match variant {
                "audience" => challenge.audience = "https://other.example",
                "void_id" => challenge.void_id = "456",
                "public_key" => challenge.public_key = STANDARD.encode([3; 32]),
                "expired" => challenge.expires_at = expiry(-1),
                "future" => challenge.expires_at = expiry(45),
                "nonce" => challenge.nonce = "not-a-canonical-nonce".into(),
                _ => {}
            }
            let mut message = challenge.message();
            if variant == "domain" {
                message = message.replace(":authenticate:v2", ":authenticate:v1");
            }
            let mut frame = challenge.frame(message);
            match variant {
                "challenge_id" => frame["challenge_id"] = URL_SAFE_NO_PAD.encode([31; 32]).into(),
                "base64" => frame["message"] = "not standard base64".into(),
                "extra_field" => frame["unexpected"] = "extra".into(),
                _ => {}
            }
            send_json(&mut socket, frame).await;
            expect_no_command(&mut socket).await;
        })
        .await;
        let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
        server.await.unwrap();
        assert_eq!(end, ConnectionStatus::AuthenticationFailed, "{variant}");
        assert!(seen.is_empty(), "{variant}");
    }
}

#[tokio::test]
async fn rejects_early_auth_ok_and_delivery() {
    let identity = identity();
    for frame in [
        json!({"type": "auth_ok", "void_id": VOID_ID}),
        json!({"type":"online_event","exchange_id":"queued","from_void_id":"456","payload_base64":"AA=="}),
        json!({"type":"error","code":"invalid_session"}),
    ] {
        let (endpoint, server) = mock_relay(move |mut socket| async move {
            send_json(&mut socket, frame).await;
            expect_no_command(&mut socket).await;
        })
        .await;
        let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
        server.await.unwrap();
        assert_eq!(end, ConnectionStatus::AuthenticationFailed);
        assert!(seen.is_empty());
    }
}

#[tokio::test]
async fn refuses_auth_ok_for_another_registered_void_id() {
    let identity = identity();
    let verifying_key = identity.key.verifying_key();
    let (endpoint, server) = mock_relay(move |mut socket| async move {
        verify_proof(&mut socket, &verifying_key).await;
        send_json(&mut socket, json!({"type": "auth_ok", "void_id": "456"})).await;
        expect_no_command(&mut socket).await;
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::AuthenticationFailed);
    assert!(seen.is_empty());
}

#[tokio::test]
async fn times_out_when_the_relay_does_not_send_a_challenge() {
    let identity = identity();
    let (endpoint, server) = mock_relay(|mut socket| async move {
        expect_no_command(&mut socket).await;
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_millis(500)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
}

#[tokio::test]
async fn reports_disconnection_if_the_relay_closes_before_proof() {
    let identity = identity();
    let (endpoint, server) = mock_relay(|mut socket| async move {
        socket.close(None).await.unwrap();
    })
    .await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
}
