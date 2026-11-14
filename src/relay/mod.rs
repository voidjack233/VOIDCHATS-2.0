//! Authenticated relay sessions. Application delivery frames are dropped in this phase.
mod protocol;

#[cfg(test)]
mod tests;

use crate::account::{API_BASE, RegisteredIdentity as SessionIdentity, load_registered_identity};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use protocol::{MAX_FRAME_SIZE, ServerFrame};
use serde_json::json;
use std::{
    io, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    sync::mpsc,
    time::{Instant, interval_at, sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Error as WireError, Message, protocol::WebSocketConfig},
};

const ENDPOINT: &str = "wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(9_500);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    AuthenticationFailed,
}

impl ConnectionStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
            Self::AuthenticationFailed => "Authentication failed",
        }
    }
}

/// Requests are coalesced and queued without blocking the caller. Dropping all
/// handles stops the worker even while a socket is open.
#[derive(Clone)]
pub(crate) struct Connection {
    requests: mpsc::Sender<String>,
}

impl Connection {
    pub(crate) fn start(report: impl Fn(ConnectionStatus) + Send + 'static) -> io::Result<Self> {
        Self::spawn(ENDPOINT.into(), load_registered_identity, report)
    }

    pub(crate) fn spawn(
        endpoint: String,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, receiver) = mpsc::channel(1);
        thread::Builder::new().name("void-relay".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(_) => {
                    drop(receiver);
                    report(ConnectionStatus::Disconnected);
                    return;
                }
            };
            runtime.block_on(run_worker(receiver, &endpoint, load_identity, &report));
        })?;
        Ok(Self { requests })
    }

    pub(crate) fn connect(&self, void_id: String) -> bool {
        self.requests.try_send(void_id).is_ok()
    }
}

async fn run_worker(
    mut requests: mpsc::Receiver<String>,
    endpoint: &str,
    load_identity: impl Fn(&str) -> io::Result<SessionIdentity>,
    report: &impl Fn(ConnectionStatus),
) {
    let mut next_attempt = Instant::now();
    while let Some(void_id) = requests.recv().await {
        let attempt = async {
            tokio::time::sleep_until(next_attempt).await;
            report(ConnectionStatus::Connecting);
            let identity = match load_identity(&void_id) {
                Ok(identity) => identity,
                Err(_) => return ConnectionStatus::AuthenticationFailed,
            };
            let result = connect_and_authenticate(endpoint, API_BASE, &identity, HANDSHAKE_TIMEOUT).await;
            // Keep the private key out of the idle session.
            drop(identity);
            monitor_authenticated(result, report).await
        };
        let terminal = tokio::select! {
            status = attempt => status,
            () = async { while requests.recv().await.is_some() {} } => return,
        };
        next_attempt = Instant::now() + RECONNECT_DELAY;
        report(terminal);
    }
}

#[cfg(test)]
async fn serve_once(
    endpoint: &str,
    audience: &str,
    identity: &SessionIdentity,
    handshake_timeout: Duration,
    report: impl Fn(ConnectionStatus),
) -> ConnectionStatus {
    let result = connect_and_authenticate(endpoint, audience, identity, handshake_timeout).await;
    monitor_authenticated(result, &report).await
}

async fn connect_and_authenticate(
    endpoint: &str,
    audience: &str,
    identity: &SessionIdentity,
    handshake_timeout: Duration,
) -> Result<Socket, ConnectionStatus> {
    timeout(handshake_timeout, async {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_SIZE))
            .max_frame_size(Some(MAX_FRAME_SIZE));
        let (mut socket, _) = connect_async_with_config(endpoint, Some(config), true)
            .await
            .map_err(|_| ConnectionStatus::Disconnected)?;
        authenticate(&mut socket, audience, identity).await?;
        Ok(socket)
    })
    .await
    .unwrap_or(Err(ConnectionStatus::Disconnected))
}

async fn authenticate(
    socket: &mut Socket,
    audience: &str,
    identity: &SessionIdentity,
) -> Result<(), ConnectionStatus> {
    send_json(socket, json!({"type": "auth_begin", "void_id": identity.void_id})).await?;
    let ServerFrame::Challenge(challenge) = read_auth_frame(socket).await? else {
        return Err(ConnectionStatus::AuthenticationFailed);
    };
    let public_key = STANDARD.encode(identity.key.verifying_key().as_bytes());
    let message = challenge
        .validate(audience, &identity.void_id, &public_key, now_ms())
        .ok_or(ConnectionStatus::AuthenticationFailed)?;
    let signature = STANDARD.encode(identity.key.sign(&message).to_bytes());
    if challenge.is_expired(now_ms()) {
        return Err(ConnectionStatus::AuthenticationFailed);
    }
    send_json(socket, json!({
        "type": "auth_proof",
        "challenge_id": challenge.id,
        "signature": signature,
    })).await?;

    match read_auth_frame(socket).await? {
        ServerFrame::AuthOk(id) if id == identity.void_id => Ok(()),
        _ => Err(ConnectionStatus::AuthenticationFailed),
    }
}

async fn send_json(socket: &mut Socket, frame: serde_json::Value) -> Result<(), ConnectionStatus> {
    socket.send(Message::Text(frame.to_string().into())).await
        .map_err(|_| ConnectionStatus::Disconnected)
}

async fn read_auth_frame(socket: &mut Socket) -> Result<ServerFrame, ConnectionStatus> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return protocol::decode(&text).ok_or(ConnectionStatus::AuthenticationFailed);
            }
            Some(Ok(Message::Ping(_))) => socket.flush().await
                .map_err(|_| ConnectionStatus::Disconnected)?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) => {
                let _ = socket.flush().await;
                return Err(ConnectionStatus::Disconnected);
            }
            None | Some(Err(WireError::ConnectionClosed | WireError::AlreadyClosed | WireError::Io(_))) =>
                return Err(ConnectionStatus::Disconnected),
            Some(Err(WireError::Protocol(tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake))) =>
                return Err(ConnectionStatus::Disconnected),
            _ => return Err(ConnectionStatus::AuthenticationFailed),
        }
    }
}

async fn monitor_authenticated(
    result: Result<Socket, ConnectionStatus>,
    report: &impl Fn(ConnectionStatus),
) -> ConnectionStatus {
    let mut socket = match result {
        Ok(socket) => socket,
        Err(status) => return status,
    };
    report(ConnectionStatus::Connected);
    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let deadline = sleep(PONG_TIMEOUT);
    tokio::pin!(deadline);
    let mut probe = 0_u64;
    let mut awaiting_pong = false;

    loop {
        tokio::select! {
            _ = &mut deadline, if awaiting_pong => break,
            _ = heartbeat.tick() => {
                if awaiting_pong {
                    break;
                }
                probe = probe.wrapping_add(1);
                match timeout(WRITE_TIMEOUT, socket.send(Message::Ping(probe.to_be_bytes().to_vec().into()))).await {
                    Ok(Ok(())) => {
                        awaiting_pong = true;
                        deadline.as_mut().reset(Instant::now() + PONG_TIMEOUT);
                    }
                    _ => break,
                }
            }
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Ping(_))) => {
                    if !matches!(timeout(WRITE_TIMEOUT, socket.flush()).await, Ok(Ok(()))) {
                        break;
                    }
                }
                Some(Ok(Message::Pong(payload))) => {
                    if payload.as_ref() == probe.to_be_bytes() {
                        awaiting_pong = false;
                    }
                }
                Some(Ok(Message::Text(text))) if matches!(protocol::decode(&text), Some(ServerFrame::Delivery)) => {
                    // The Go relay may offer queued frames on connect. Never process or acknowledge them.
                }
                Some(Ok(Message::Close(_))) => {
                    let _ = timeout(WRITE_TIMEOUT, socket.flush()).await;
                    break;
                }
                _ => break,
            },
        }
    }
    ConnectionStatus::Disconnected
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
