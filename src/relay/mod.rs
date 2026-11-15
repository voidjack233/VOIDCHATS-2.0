//! Authenticated relay sessions and serialized command dispatch.
mod protocol;

#[cfg(test)]
pub(crate) mod tests;

use crate::account::{API_BASE, RegisteredIdentity as SessionIdentity, load_registered_identity};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use protocol::{MAX_FRAME_SIZE, ServerFrame};
pub(crate) use protocol::{Reply, Request};
use serde_json::json;
use std::{
    io,
    sync::mpsc as blocking,
    thread,
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
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);

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
    requests: mpsc::Sender<ConnectRequest>,
}

struct ConnectRequest {
    void_id: String,
    messaging: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RequestError {
    Interrupted,
    Rejected(String),
}

struct PendingRequest {
    generation: u64,
    request: Request,
    reply: blocking::Sender<Result<Reply, RequestError>>,
}

#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) generation: u64,
    commands: mpsc::Sender<PendingRequest>,
}

impl Session {
    // Called on the SQLCipher worker. The socket continues to receive events during this wait.
    pub(crate) fn request(&self, request: Request) -> Result<Reply, RequestError> {
        let (reply, response) = blocking::channel();
        self.commands
            .try_send(PendingRequest {
                generation: self.generation,
                request,
                reply,
            })
            .map_err(|_| RequestError::Interrupted)?;
        response
            .recv_timeout(WRITE_TIMEOUT + RESPONSE_TIMEOUT + Duration::from_secs(2))
            .unwrap_or(Err(RequestError::Interrupted))
    }
}

pub(crate) enum Activity {
    Online(Session),
    Offline(u64),
    Delivery {
        generation: u64,
        sender: String,
        event_id: String,
        json: String,
    },
}

impl Connection {
    pub(crate) fn start(
        report: impl Fn(ConnectionStatus) + Send + 'static,
        activity: impl Fn(Activity) + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_activity(ENDPOINT.into(), load_registered_identity, report, activity)
    }

    #[cfg(test)]
    pub(crate) fn spawn(
        endpoint: String,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_activity(endpoint, load_identity, report, |_| {})
    }

    pub(crate) fn spawn_with_activity(
        endpoint: String,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
        activity: impl Fn(Activity) + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, receiver) = mpsc::channel(1);
        thread::Builder::new()
            .name("void-relay".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        drop(receiver);
                        report(ConnectionStatus::Disconnected);
                        return;
                    }
                };
                runtime.block_on(run_worker(
                    receiver,
                    &endpoint,
                    load_identity,
                    &report,
                    &activity,
                ));
            })?;
        Ok(Self { requests })
    }

    pub(crate) fn connect(&self, void_id: String) -> bool {
        self.requests
            .try_send(ConnectRequest {
                void_id,
                messaging: false,
            })
            .is_ok()
    }

    pub(crate) fn connect_messaging(&self, void_id: String) -> bool {
        self.requests
            .try_send(ConnectRequest {
                void_id,
                messaging: true,
            })
            .is_ok()
    }
}

async fn run_worker(
    mut requests: mpsc::Receiver<ConnectRequest>,
    endpoint: &str,
    load_identity: impl Fn(&str) -> io::Result<SessionIdentity>,
    report: &impl Fn(ConnectionStatus),
    activity: &impl Fn(Activity),
) {
    let mut next_attempt = Instant::now();
    let mut generation = 0;
    let mut next = requests.recv().await;
    while let Some(ConnectRequest { void_id, messaging }) = next {
        generation += 1;
        // Each connection has a fresh command queue. An old handle cannot target a new socket.
        let (commands, mut command_receiver) = mpsc::channel(2);
        let session = Session {
            generation,
            commands,
        };
        let attempt = async {
            tokio::time::sleep_until(next_attempt).await;
            report(ConnectionStatus::Connecting);
            let identity = match load_identity(&void_id) {
                Ok(identity) => identity,
                Err(_) => return ConnectionStatus::AuthenticationFailed,
            };
            let result =
                connect_and_authenticate(endpoint, API_BASE, &identity, HANDSHAKE_TIMEOUT).await;
            // Keep the private key out of the idle session.
            drop(identity);
            let messaging = messaging.then_some(Messaging {
                session,
                commands: &mut command_receiver,
                activity,
            });
            monitor_authenticated(result, report, messaging).await
        };
        let (terminal, replacement) = tokio::select! {
            status = attempt => (Some(status), None),
            request = requests.recv() => {
                let Some(request) = request else { return; };
                (None, Some(request))
            },
        };
        drop(command_receiver);
        if messaging {
            activity(Activity::Offline(generation));
        }
        next_attempt = Instant::now() + RECONNECT_DELAY;
        next = if let Some(status) = terminal {
            report(status);
            requests.recv().await
        } else {
            replacement
        };
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
    monitor_authenticated::<fn(Activity)>(result, &report, None).await
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
    send_json(
        socket,
        json!({"type": "auth_begin", "void_id": identity.void_id}),
    )
    .await?;
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
    send_json(
        socket,
        json!({
            "type": "auth_proof",
            "challenge_id": challenge.id,
            "signature": signature,
        }),
    )
    .await?;

    match read_auth_frame(socket).await? {
        ServerFrame::AuthOk(id) if id == identity.void_id => Ok(()),
        _ => Err(ConnectionStatus::AuthenticationFailed),
    }
}

async fn send_json(socket: &mut Socket, frame: serde_json::Value) -> Result<(), ConnectionStatus> {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .map_err(|_| ConnectionStatus::Disconnected)
}

async fn read_auth_frame(socket: &mut Socket) -> Result<ServerFrame, ConnectionStatus> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) if text.len() <= 8 * 1024 => {
                return protocol::decode(&text).ok_or(ConnectionStatus::AuthenticationFailed);
            }
            Some(Ok(Message::Ping(_))) => socket
                .flush()
                .await
                .map_err(|_| ConnectionStatus::Disconnected)?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) => {
                let _ = socket.flush().await;
                return Err(ConnectionStatus::Disconnected);
            }
            None
            | Some(Err(
                WireError::ConnectionClosed | WireError::AlreadyClosed | WireError::Io(_),
            )) => return Err(ConnectionStatus::Disconnected),
            Some(Err(WireError::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            ))) => return Err(ConnectionStatus::Disconnected),
            _ => return Err(ConnectionStatus::AuthenticationFailed),
        }
    }
}

struct Messaging<'a, F> {
    session: Session,
    commands: &'a mut mpsc::Receiver<PendingRequest>,
    activity: &'a F,
}

async fn monitor_authenticated<F: Fn(Activity)>(
    result: Result<Socket, ConnectionStatus>,
    report: &impl Fn(ConnectionStatus),
    mut messaging: Option<Messaging<'_, F>>,
) -> ConnectionStatus {
    let mut socket = match result {
        Ok(socket) => socket,
        Err(status) => return status,
    };
    report(ConnectionStatus::Connected);
    if let Some(mode) = &messaging {
        (mode.activity)(Activity::Online(mode.session.clone()));
    }
    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let deadline = sleep(PONG_TIMEOUT);
    tokio::pin!(deadline);
    let mut probe = 0_u64;
    let mut awaiting_pong = false;
    let mut pending: Option<PendingRequest> = None;
    let command_deadline = sleep(RESPONSE_TIMEOUT);
    tokio::pin!(command_deadline);
    let mut delivered: Option<(String, String, i64)> = None;

    loop {
        tokio::select! {
            _ = &mut command_deadline, if pending.is_some() => break,
            command = async {
                match &mut messaging {
                    Some(mode) => mode.commands.recv().await,
                    None => std::future::pending().await,
                }
            }, if pending.is_none() => {
                let Some(command) = command else { break; };
                let Some(mode) = &messaging else { break; };
                if command.generation != mode.session.generation {
                    let _ = command.reply.send(Err(RequestError::Interrupted));
                    continue;
                }
                if let Request::Ack { event_id, sender_void_id } = &command.request {
                    if !delivered.as_ref().is_some_and(|(sender, id, expiry)|
                        sender == sender_void_id && id == event_id && *expiry > now_ms())
                    {
                        let _ = command.reply.send(Err(RequestError::Interrupted));
                        continue;
                    }
                    // The relay can offer the next delivery as soon as it commits this ACK.
                    delivered = None;
                }
                let Ok(frame) = serde_json::to_string(&command.request) else { break; };
                if frame.len() > MAX_FRAME_SIZE || !matches!(timeout(WRITE_TIMEOUT, socket.send(Message::Text(frame.into()))).await, Ok(Ok(()))) {
                    break;
                }
                command_deadline.as_mut().reset(Instant::now() + RESPONSE_TIMEOUT);
                pending = Some(command);
            }
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
                Some(Ok(Message::Text(text))) => match protocol::decode(&text) {
                    Some(ServerFrame::Delivery { sender, event_id, expiry_ms }) => {
                        if let Some(mode) = &messaging {
                            if expiry_ms <= now_ms() { continue; }
                            if delivered.as_ref().is_some_and(|(_, _, expiry)| *expiry > now_ms()) { break; }
                            delivered = Some((sender.clone(), event_id.clone(), expiry_ms));
                            (mode.activity)(Activity::Delivery { generation: mode.session.generation, sender, event_id, json: text.to_string() });
                        }
                    }
                    // Online setup frames from earlier protocols are outside queued Signal messaging.
                    Some(ServerFrame::OnlineDelivery) => {}
                    Some(ServerFrame::Reply(reply)) if pending.as_ref().is_some_and(|request| request.request.matches(&reply)) => {
                        let _ = pending.take().unwrap().reply.send(Ok(reply));
                    }
                    Some(ServerFrame::PrekeyError { request_id, code })
                        if pending.as_ref().and_then(|request| request.request.prekey_id()) == Some(request_id.as_str()) => {
                        let _ = pending.take().unwrap().reply.send(Err(RequestError::Rejected(code)));
                    }
                    Some(ServerFrame::Error(code)) if pending.is_some() => {
                        let _ = pending.take().unwrap().reply.send(Err(RequestError::Rejected(code)));
                    }
                    _ => break,
                },
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
