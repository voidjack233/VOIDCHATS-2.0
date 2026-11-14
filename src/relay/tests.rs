use super::{ConnectionStatus, SessionIdentity, serve_once};
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
type Socket = WebSocketStream<TcpStream>;

fn identity() -> SessionIdentity {
    SessionIdentity {
        void_id: VOID_ID.into(),
        key: SigningKey::from_bytes(&[7; 32]),
    }
}

async fn receive_json(socket: &mut Socket) -> Value {
    let frame = socket.next().await.unwrap().unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

async fn send_json(socket: &mut Socket, body: Value) {
    socket.send(Message::Text(body.to_string().into())).await.unwrap();
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
            self.id, self.void_id, self.public_key, self.expires_at,
            STANDARD.encode(self.audience), self.nonce
        )
    }

    fn frame(&self, message: String) -> Value {
        json!({
            "type": "auth_challenge", "challenge_id": self.id,
            "message": STANDARD.encode(message), "expires_at": self.expires_at
        })
    }
}

fn expiry(seconds: i64) -> String {
    let now: chrono::DateTime<Utc> = std::time::SystemTime::now().into();
    (now + chrono::Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

async fn verify_proof(socket: &mut Socket, key: &VerifyingKey) {
    let challenge = Challenge::new(*key);
    let message = challenge.message();
    send_json(socket, challenge.frame(message.clone())).await;
    let proof = receive_json(socket).await;
    let signature = proof["signature"].as_str().unwrap();
    assert_eq!(proof, json!({
        "type": "auth_proof", "challenge_id": challenge.id, "signature": signature
    }));
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
async fn authenticates_and_keeps_the_connection_without_handling_messages() {
    let identity = identity();
    let verifying_key = identity.key.verifying_key();
    let (endpoint, server) = mock_relay(move |mut socket| async move {
        verify_proof(&mut socket, &verifying_key).await;
        send_json(&mut socket, json!({"type": "auth_ok", "void_id": VOID_ID})).await;
        // The relay may send queued data immediately. Only the Pong should come back.
        send_json(&mut socket, json!({
            "type": "event", "event_id": "12345678-1234-1234-1234-123456789abc",
            "from_void_id": "456", "payload_base64": "AA==",
            "created_at": "2026-11-14T00:00:00Z", "expires_at": "2026-11-14T00:10:00Z"
        })).await;
        let payload = b"relay probe".to_vec();
        socket.send(Message::Ping(payload.clone().into())).await.unwrap();
        assert_eq!(socket.next().await.unwrap().unwrap(), Message::Pong(payload.into()));
        socket.close(None).await.unwrap();
    }).await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert_eq!(seen, vec![ConnectionStatus::Connected]);
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
        }).await;
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
        }).await;
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
    }).await;
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
    }).await;
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
    }).await;
    let (end, seen) = connect(&endpoint, &identity, Duration::from_secs(2)).await;
    server.await.unwrap();
    assert_eq!(end, ConnectionStatus::Disconnected);
    assert!(seen.is_empty());
}
