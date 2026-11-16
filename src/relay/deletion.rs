//! A dedicated deletion socket never starts a normal messaging session.
use super::{
    API_BASE, ENDPOINT, Message, ServerFrame, SessionIdentity, Socket, WireError, now_ms, protocol,
    report_rejection,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{io, time::Duration};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async_with_config, tungstenite::protocol::WebSocketConfig};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(35);
const MAX_FRAME: usize = 8 * 1024;

#[derive(Debug)]
pub(crate) enum Error {
    Interrupted,
    InvalidResponse,
    Rejected(String),
    IntentNotSaved,
}

impl Error {
    pub(crate) fn message(&self, pending: bool) -> &'static str {
        match self {
            Self::IntentNotSaved => {
                "VOID could not save the deletion request on this device. Check that its account files are writable, then retry."
            }
            Self::Rejected(code) if matches!(code.as_str(), "rate_limited" | "overloaded") => {
                "The relay is limiting deletion requests. Wait a minute, then retry."
            }
            Self::Rejected(code)
                if matches!(
                    code.as_str(),
                    "deletion_authorization_failed" | "unauthorized"
                ) =>
            {
                "The relay could not authorize deletion with this account key. Keep this device’s account data and retry."
            }
            Self::Rejected(code) if code == "invalid_frame" => {
                "The relay rejected the deletion request format. Check for a VOID update before retrying."
            }
            Self::InvalidResponse if !pending => {
                "VOID could not verify the relay’s deletion challenge. Check for a VOID update, then retry."
            }
            _ if pending => {
                "VOID has no verified confirmation of deletion yet. Check your connection and retry to confirm the result with the relay."
            }
            _ => "The relay could not start account deletion. Check your connection and retry.",
        }
    }
}

pub(crate) fn delete(
    identity: &SessionIdentity,
    before_confirm: impl FnOnce() -> io::Result<()>,
) -> Result<String, Error> {
    delete_at(ENDPOINT, identity, before_confirm)
}

pub(crate) fn delete_at(
    endpoint: &str,
    identity: &SessionIdentity,
    before_confirm: impl FnOnce() -> io::Result<()>,
) -> Result<String, Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::Interrupted)?;
    let result = runtime.block_on(async {
        timeout(
            EXCHANGE_TIMEOUT,
            exchange(endpoint, identity, before_confirm),
        )
        .await
        .unwrap_or(Err(Error::Interrupted))
    });
    runtime.shutdown_background();
    result
}

async fn exchange(
    endpoint: &str,
    identity: &SessionIdentity,
    before_confirm: impl FnOnce() -> io::Result<()>,
) -> Result<String, Error> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let (mut socket, _) = connect_async_with_config(endpoint, Some(config), true)
        .await
        .map_err(|_| Error::Interrupted)?;
    send(
        &mut socket,
        json!({"type": "account_delete_begin", "void_id": identity.void_id}),
    )
    .await?;
    let ServerFrame::DeleteChallenge(challenge) =
        receive(&mut socket, "account_delete_begin").await?
    else {
        return Err(Error::InvalidResponse);
    };
    let public_key = STANDARD.encode(identity.key.verifying_key().as_bytes());
    let message = challenge
        .validate_deletion(API_BASE, &identity.void_id, &public_key, now_ms())
        .ok_or(Error::InvalidResponse)?;

    // A proof can reach the relay even when a write fails or its response is lost.
    before_confirm().map_err(|_| Error::IntentNotSaved)?;
    let signature = STANDARD.encode(identity.key.sign(&message).to_bytes());
    if challenge.is_expired(now_ms()) {
        return Err(Error::Interrupted);
    }
    send(
        &mut socket,
        json!({
            "type": "account_delete_confirm", "challenge_id": challenge.id,
            "confirmation": "DELETE", "signature": signature,
        }),
    )
    .await?;
    match receive(&mut socket, "account_delete_confirm").await? {
        ServerFrame::AccountDeleted {
            void_id,
            deleted_at,
        } if void_id == identity.void_id => Ok(deleted_at),
        _ => Err(Error::InvalidResponse),
    }
}

async fn send(socket: &mut Socket, frame: Value) -> Result<(), Error> {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .map_err(|_| Error::Interrupted)
}

async fn receive(socket: &mut Socket, command: &str) -> Result<ServerFrame, Error> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) if text.len() <= MAX_FRAME => {
                return match protocol::decode(&text) {
                    Some(ServerFrame::Error(code)) => {
                        report_rejection(command, &code);
                        Err(Error::Rejected(code))
                    }
                    Some(frame) => Ok(frame),
                    None => Err(Error::InvalidResponse),
                };
            }
            Some(Ok(Message::Ping(_))) => socket.flush().await.map_err(|_| Error::Interrupted)?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | Some(Err(WireError::Io(_))) | None => {
                return Err(Error::Interrupted);
            }
            Some(Err(_)) => return Err(Error::Interrupted),
            _ => return Err(Error::InvalidResponse),
        }
    }
}
