//! Authenticated relay sessions and serialized command dispatch.
pub(crate) mod deletion;
mod protocol;

#[cfg(test)]
pub(crate) mod tests;

use crate::account::{API_BASE, RegisteredIdentity as SessionIdentity, load_registered_identity};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
pub(crate) use protocol::{
    AccountState, ApplicationProtocolVersion, Reply, Request, SUPPORTED_APPLICATION_PROTOCOL,
    valid_utc_timestamp,
};
use protocol::{MAX_FRAME_SIZE, ServerFrame};
use serde_json::json;
use std::{
    io,
    sync::mpsc as blocking,
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::{Instant, interval_at, sleep, sleep_until, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Error as WireError, Message, protocol::WebSocketConfig},
};

const ENDPOINT: &str = "wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(9_500);
const MIN_RECONNECT_DELAY: Duration = Duration::from_secs(2);
const STABLE_CONNECTION: Duration = Duration::from_secs(60);
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
    Reconnecting,
    AuthenticationFailed,
    UpdateRequired(ApplicationProtocolVersion),
}

impl ConnectionStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
            Self::Reconnecting => "Reconnecting",
            Self::AuthenticationFailed => "Authentication failed",
            Self::UpdateRequired(_) => "Update required",
        }
    }
}

/// Requests are coalesced and queued without blocking the caller. Dropping all
/// handles stops the worker even while a socket is open.
#[derive(Clone)]
pub(crate) struct Connection {
    requests: mpsc::Sender<String>,
}

pub(crate) struct Worker {
    stop: oneshot::Sender<()>,
    thread: JoinHandle<()>,
}

impl Worker {
    pub(crate) fn finish(self) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RequestError {
    Interrupted,
    Rejected { command: &'static str, code: String },
}

struct PendingRequest {
    generation: u64,
    request: Request,
    reply: blocking::Sender<Result<Reply, RequestError>>,
}

impl PendingRequest {
    fn reject(self, code: String) {
        let command = self.request.name();
        report_rejection(command, &code);
        let _ = self
            .reply
            .send(Err(RequestError::Rejected { command, code }));
    }
}

fn report_rejection(command: &str, code: &str) {
    let code = match code {
        "authentication_failed"
        | "unauthorized"
        | "invalid_frame"
        | "not_found"
        | "duplicate_event"
        | "queue_full"
        | "rate_limited"
        | "overloaded"
        | "unavailable"
        | "prekeys_invalid"
        | "prekeys_conflict"
        | "prekeys_capacity"
        | "prekeys_unavailable"
        | "prekey_claim_expired"
        | "account_deleted"
        | "challenge_expired"
        | "challenge_clock_mismatch"
        | "challenge_binding_invalid"
        | "deletion_authorization_failed" => code,
        _ => "unknown_code",
    };
    eprintln!("command={command} code={code}");
}

#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) generation: u64,
    commands: mpsc::Sender<PendingRequest>,
}

impl Session {
    pub(crate) fn is_active(&self) -> bool {
        !self.commands.is_closed()
    }

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
    ) -> io::Result<(Self, Worker)> {
        Self::spawn_with_activity(ENDPOINT.into(), load_registered_identity, report, activity)
    }

    #[cfg(test)]
    pub(crate) fn spawn(
        endpoint: String,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
    ) -> io::Result<(Self, Worker)> {
        Self::spawn_with_activity(endpoint, load_identity, report, |_| {})
    }

    pub(crate) fn spawn_with_activity(
        endpoint: String,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
        activity: impl Fn(Activity) + Send + 'static,
    ) -> io::Result<(Self, Worker)> {
        Self::spawn_configured(
            endpoint,
            API_BASE.into(),
            SUPPORTED_APPLICATION_PROTOCOL,
            load_identity,
            report,
            activity,
        )
    }

    /// Only compiled into tests, and deliberately unusable with a remote relay.
    #[cfg(test)]
    pub(crate) fn spawn_with_test_protocol(
        endpoint: String,
        audience: String,
        version: ApplicationProtocolVersion,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
        activity: impl Fn(Activity) + Send + 'static,
    ) -> io::Result<(Self, Worker)> {
        let url = reqwest::Url::parse(&endpoint).map_err(|_| io::ErrorKind::InvalidInput)?;
        if url.scheme() != "ws"
            || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(
                version,
                ApplicationProtocolVersion::V1 | ApplicationProtocolVersion::V2
            )
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Self::spawn_configured(endpoint, audience, version, load_identity, report, activity)
    }

    fn spawn_configured(
        endpoint: String,
        audience: String,
        application_version: ApplicationProtocolVersion,
        load_identity: impl Fn(&str) -> io::Result<SessionIdentity> + Send + 'static,
        report: impl Fn(ConnectionStatus) + Send + 'static,
        activity: impl Fn(Activity) + Send + 'static,
    ) -> io::Result<(Self, Worker)> {
        let (requests, receiver) = mpsc::channel(1);
        let (stop, stopped) = oneshot::channel();
        let thread = thread::Builder::new()
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
                runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = stopped => {}
                        _ = run_worker(receiver, &endpoint, &audience, application_version, load_identity, &report, &activity) => {}
                    }
                });
                // DNS can use Tokio's blocking pool. Cancelling the connection
                // future must not make application shutdown wait on system DNS.
                runtime.shutdown_background();
            })?;
        Ok((Self { requests }, Worker { stop, thread }))
    }

    pub(crate) fn connect(&self, void_id: String) -> bool {
        self.requests.try_send(void_id).is_ok()
    }
}

async fn run_worker(
    mut requests: mpsc::Receiver<String>,
    endpoint: &str,
    audience: &str,
    application_version: ApplicationProtocolVersion,
    load_identity: impl Fn(&str) -> io::Result<SessionIdentity>,
    report: &impl Fn(ConnectionStatus),
    activity: &impl Fn(Activity),
) {
    let mut next_attempt = Instant::now();
    let mut manual_attempt = next_attempt;
    let mut retry_step = 0_u32;
    let mut generation = 0;
    let mut next = requests.recv().await;
    while let Some(void_id) = &next {
        tokio::select! {
            biased;
            request = requests.recv() => {
                // A manual reconnect can shorten the wait, with the same minimum
                // cooldown across clicks and failed attempts.
                next = request;
                next_attempt = manual_attempt;
                continue;
            }
            _ = sleep_until(next_attempt) => {}
        }
        generation += 1;
        // Each connection has a fresh command queue. An old handle cannot target a new socket.
        let (commands, mut command_receiver) = mpsc::channel(2);
        let session = Session {
            generation,
            commands,
        };
        let mut connected_at = None;
        let attempt = async {
            report(ConnectionStatus::Connecting);
            let identity = match load_identity(void_id) {
                Ok(identity) => identity,
                Err(_) => return ConnectionStatus::AuthenticationFailed,
            };
            let result =
                connect_and_authenticate(endpoint, audience, &identity, HANDSHAKE_TIMEOUT).await;
            // Keep the private key out of the idle session.
            drop(identity);
            if result.is_ok() {
                connected_at = Some(Instant::now());
            }
            let messaging = Some(Messaging {
                session,
                commands: &mut command_receiver,
                activity,
            });
            monitor_authenticated(result, report, messaging, application_version).await
        };
        let (terminal, replacement) = tokio::select! {
            biased;
            request = requests.recv() => {
                let Some(request) = request else { return; };
                (None, Some(request))
            },
            status = attempt => (Some(status), None),
        };
        drop(command_receiver);
        activity(Activity::Offline(generation));
        let ended = Instant::now();
        manual_attempt = ended + MIN_RECONNECT_DELAY;
        next_attempt = manual_attempt;
        // A brief successful handshake is part of the same failure streak.
        if connected_at.is_some_and(|since| ended - since >= STABLE_CONNECTION) {
            retry_step = 0;
        }
        match terminal {
            Some(status @ ConnectionStatus::UpdateRequired(_)) => {
                report(status);
                return;
            }
            Some(ConnectionStatus::Disconnected) => {
                report(ConnectionStatus::Reconnecting);
                let delay = reconnect_delay(retry_step, rand::random_range(0..=25));
                retry_step = retry_step.saturating_add(1);
                next_attempt = ended + delay;
            }
            Some(status) => {
                report(status);
                retry_step = 0;
                next = requests.recv().await;
            }
            None => {
                report(ConnectionStatus::Reconnecting);
                next = replacement;
            }
        }
    }
}

fn reconnect_delay(step: u32, jitter_percent: u32) -> Duration {
    // Cap the base below one minute so capped retries still spread out.
    let base = (MIN_RECONNECT_DELAY * (1 << step.min(5))).min(Duration::from_secs(48));
    base + base * jitter_percent / 100
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
    monitor_authenticated::<fn(Activity)>(result, &report, None, SUPPORTED_APPLICATION_PROTOCOL)
        .await
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
            .map_err(|error| match error {
                WireError::Http(response) if matches!(response.status().as_u16(), 401 | 403) => {
                    ConnectionStatus::AuthenticationFailed
                }
                _ => ConnectionStatus::Disconnected,
            })?;
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
    let ServerFrame::Challenge(challenge) = read_auth_frame(socket, "auth_begin").await? else {
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

    match read_auth_frame(socket, "auth_proof").await? {
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

async fn read_auth_frame(
    socket: &mut Socket,
    command: &str,
) -> Result<ServerFrame, ConnectionStatus> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) if text.len() <= 8 * 1024 => {
                return match protocol::decode(&text) {
                    Some(ServerFrame::Error(code)) => {
                        report_rejection(command, &code);
                        if code == "unavailable" {
                            Err(ConnectionStatus::Disconnected)
                        } else {
                            Err(ConnectionStatus::AuthenticationFailed)
                        }
                    }
                    Some(frame) => Ok(frame),
                    None => Err(ConnectionStatus::AuthenticationFailed),
                };
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

struct BufferedDelivery {
    sender: String,
    event_id: String,
    expiry_ms: i64,
    json: String,
}

async fn negotiate_protocol(
    socket: &mut Socket,
    application_version: ApplicationProtocolVersion,
) -> Result<Option<BufferedDelivery>, ConnectionStatus> {
    send_json(
        socket,
        json!({ "type": "protocol", "version": application_version }),
    )
    .await?;
    let mut buffered = None;
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => match protocol::decode(&text) {
                Some(ServerFrame::ProtocolOk(version)) if version == application_version => {
                    return Ok(buffered);
                }
                // Each socket starts at version 1. A legacy queued event may be
                // offered immediately after auth_ok, before protocol_ok. Hold at
                // most the relay's single in-flight event until the worker is online.
                Some(ServerFrame::Delivery {
                    sender,
                    event_id,
                    expiry_ms,
                    min_protocol_version,
                }) if min_protocol_version == ApplicationProtocolVersion::V1 => {
                    if expiry_ms <= now_ms() {
                        continue;
                    }
                    if buffered
                        .as_ref()
                        .is_some_and(|event: &BufferedDelivery| event.expiry_ms > now_ms())
                    {
                        return Err(ConnectionStatus::Disconnected);
                    }
                    buffered = Some(BufferedDelivery {
                        sender,
                        event_id,
                        expiry_ms,
                        json: text.to_string(),
                    });
                }
                Some(ServerFrame::UpdateRequired(version)) if version > application_version => {
                    return Err(ConnectionStatus::UpdateRequired(version));
                }
                Some(ServerFrame::OnlineDelivery) => {}
                Some(ServerFrame::Error(code)) => {
                    report_rejection("protocol", &code);
                    return Err(
                        if matches!(code.as_str(), "authentication_failed" | "unauthorized") {
                            ConnectionStatus::AuthenticationFailed
                        } else {
                            ConnectionStatus::Disconnected
                        },
                    );
                }
                _ => return Err(ConnectionStatus::Disconnected),
            },
            Some(Ok(Message::Ping(_))) => socket
                .flush()
                .await
                .map_err(|_| ConnectionStatus::Disconnected)?,
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(ConnectionStatus::Disconnected),
        }
    }
}

async fn monitor_authenticated<F: Fn(Activity)>(
    result: Result<Socket, ConnectionStatus>,
    report: &impl Fn(ConnectionStatus),
    mut messaging: Option<Messaging<'_, F>>,
    application_version: ApplicationProtocolVersion,
) -> ConnectionStatus {
    let mut socket = match result {
        Ok(socket) => socket,
        Err(status) => return status,
    };
    // Negotiation belongs to each authenticated socket, including reconnects.
    // No prekey/send/ACK command is exposed before the exact protocol_ok arrives.
    let buffered = match timeout(
        RESPONSE_TIMEOUT,
        negotiate_protocol(&mut socket, application_version),
    )
    .await
    {
        Ok(Ok(buffered)) => buffered,
        Ok(Err(status)) => return status,
        Err(_) => return ConnectionStatus::Disconnected,
    };
    report(ConnectionStatus::Connected);
    let mut delivered: Option<(String, String, i64)> = None;
    if let Some(mode) = &messaging {
        (mode.activity)(Activity::Online(mode.session.clone()));
        if let Some(event) = buffered.filter(|event| event.expiry_ms > now_ms()) {
            delivered = Some((
                event.sender.clone(),
                event.event_id.clone(),
                event.expiry_ms,
            ));
            (mode.activity)(Activity::Delivery {
                generation: mode.session.generation,
                sender: event.sender,
                event_id: event.event_id,
                json: event.json,
            });
        }
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
                    Some(ServerFrame::Delivery { sender, event_id, expiry_ms, min_protocol_version }) => {
                        // Do not decrypt or ACK unsupported events, even when the
                        // same event ID was previously processed as a legacy event.
                        if min_protocol_version > application_version { break; }
                        if let Some(mode) = &messaging {
                            if expiry_ms <= now_ms() { continue; }
                            if delivered.as_ref().is_some_and(|(_, _, expiry)| *expiry > now_ms()) { break; }
                            delivered = Some((sender.clone(), event_id.clone(), expiry_ms));
                            (mode.activity)(Activity::Delivery { generation: mode.session.generation, sender, event_id, json: text.to_string() });
                        }
                    }
                    Some(ServerFrame::UpdateRequired(version)) if version > application_version => return ConnectionStatus::UpdateRequired(version),
                    // Online setup frames from earlier protocols are outside queued Signal messaging.
                    Some(ServerFrame::OnlineDelivery) => {}
                    Some(ServerFrame::Reply(reply)) if pending.as_ref().is_some_and(|request| request.request.matches(&reply)) => {
                        let _ = pending.take().unwrap().reply.send(Ok(reply));
                    }
                    Some(ServerFrame::PrekeyError { request_id, code })
                        if pending.as_ref().and_then(|request| request.request.prekey_id()) == Some(request_id.as_str()) => {
                        pending.take().unwrap().reject(code);
                    }
                    Some(ServerFrame::Error(code)) if matches!(code.as_str(), "authentication_failed" | "unauthorized") => {
                        if let Some(request) = pending.take() {
                            request.reject(code);
                        }
                        return ConnectionStatus::AuthenticationFailed;
                    }
                    Some(ServerFrame::Error(code)) if pending.is_some() => {
                        pending.take().unwrap().reject(code);
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
