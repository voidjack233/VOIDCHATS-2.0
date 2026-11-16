//! Messaging runs on the existing SQLCipher worker. Relay requests block only
//! this worker, and no network operation runs inside a database transaction.

use crate::{
    account::valid_void_id,
    relay::{Reply, Request, RequestError, Session},
    storage::{Database, Error as StorageError, messaging::SendState, signal::peer::TrustedPeer},
};
use std::time::SystemTime;

#[derive(Debug)]
pub(crate) enum Error {
    Storage(StorageError),
    Relay(RequestError),
    Offline,
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<RequestError> for Error {
    fn from(error: RequestError) -> Self {
        Self::Relay(error)
    }
}

impl Error {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Offline => "Connect to the relay, then choose Resend.",
            Self::Relay(RequestError::Interrupted) => {
                "The relay response was interrupted. Check the connection and try again."
            }
            Self::Relay(RequestError::Rejected(code)) => match code.as_str() {
                "not_found" => "The peer’s VOID-ID was not found.",
                "duplicate_event" => "The relay already has this event. Acceptance is unconfirmed.",
                "prekeys_unavailable" => "The peer’s public Signal prekeys are unavailable.",
                "prekey_claim_expired" => {
                    "The prekey claim expired. Choose Resend to request a new claim."
                }
                "queue_full" => "The relay’s encrypted queue is full. Try again later.",
                "prekeys_capacity" | "rate_limited" | "overloaded" => {
                    "The relay is at capacity. Try again later."
                }
                _ => "The relay rejected the request.",
            },
            Self::Storage(
                StorageError::Database | StorageError::Unavailable | StorageError::Busy,
            ) => {
                "VOID could not confirm the local storage update. Check the message status before retrying."
            }
            Self::Storage(error) => error.open_message(),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

pub(crate) fn publish(db: &mut Database, session: &Session) -> Result<()> {
    if let Some(publication) = db.prepare_publication()? {
        let reply = session.request(Request::PrekeysPublish {
            publication_id: publication.id.clone(),
            payload_base64: publication.payload_base64,
        })?;
        if !matches!(reply, Reply::Published { publication_id } if publication_id == publication.id)
        {
            return Err(RequestError::Interrupted.into());
        }
        db.confirm_publication(&publication.id)?;
    }
    Ok(())
}

fn lookup(session: &Session, void_id: &str) -> Result<TrustedPeer> {
    if !valid_void_id(void_id) {
        return Err(StorageError::SignalPeer.into());
    }
    match session.request(Request::Lookup {
        void_id: void_id.into(),
    })? {
        Reply::Person {
            void_id: reported,
            public_key,
        } if reported == void_id => Ok(TrustedPeer::from_trusted_lookup(void_id, &public_key)?),
        _ => Err(RequestError::Interrupted.into()),
    }
}

pub(crate) fn send(db: &mut Database, session: Option<&Session>, event_id: &str) -> Result<()> {
    let operation = db.outgoing_operation(event_id)?;
    if operation.state == "sent" {
        return Ok(());
    }
    let mut outcome = if operation.state == "unconfirmed" {
        SendState::Unconfirmed
    } else {
        SendState::Failed
    };
    let attempt = (|| -> Result<()> {
        let session = session
            .filter(|session| session.is_active())
            .ok_or(Error::Offline)?;
        let peer = lookup(session, &operation.peer)?;
        let session_exists = db.has_signal_session(&peer)?;
        let payload = if let Some(committed) = db.committed_outgoing(event_id)? {
            if !session_exists
                || committed.conversation_id != operation.conversation_id
                || committed.body != operation.body
                || committed.created_at != operation.created_at
            {
                return Err(StorageError::EventConflict.into());
            }
            committed.payload
        } else {
            if !session_exists {
                let request_id = db.outgoing_claim(event_id)?;
                let bundle = session.request(Request::PrekeysClaim {
                    request_id: request_id.clone(),
                    void_id: operation.peer.clone(),
                });
                match bundle {
                    Ok(Reply::Bundle {
                        request_id: reply_id,
                        void_id,
                        payload_base64,
                    }) if reply_id == request_id && void_id == operation.peer => {
                        if !session.is_active() {
                            return Err(RequestError::Interrupted.into());
                        }
                        db.establish_signal_session(
                            &peer,
                            &void_id,
                            &payload_base64,
                            SystemTime::now(),
                        )?;
                    }
                    Err(error) => {
                        if matches!(&error, RequestError::Rejected(code) if code == "prekey_claim_expired")
                        {
                            db.clear_expired_claim(event_id)?;
                        }
                        return Err(error.into());
                    }
                    _ => return Err(RequestError::Interrupted.into()),
                }
            }
            if !session.is_active() {
                return Err(RequestError::Interrupted.into());
            }
            db.encrypt_signal_message(
                &peer,
                operation.conversation_id,
                event_id,
                &operation.body,
                operation.created_at,
            )?
        };
        let payload_base64 = String::from_utf8(payload).map_err(|_| StorageError::Unreadable)?;
        // Crash or timeout from this point forward means the send may have reached the relay.
        db.set_send_state(event_id, SendState::Unconfirmed)?;
        outcome = SendState::Unconfirmed;
        let reply = session.request(Request::Send {
            event_id: event_id.into(),
            to_void_id: operation.peer,
            payload_base64,
        });
        match reply {
            Ok(Reply::Accepted { event_id: accepted }) if accepted == event_id => {
                db.set_send_state(event_id, SendState::Sent)?;
                Ok(())
            }
            Err(error) => Err(error.into()),
            _ => Err(RequestError::Interrupted.into()),
        }
    })();
    if attempt.is_err() {
        // If this also fails, the previously committed state remains conservative.
        db.set_send_state(event_id, outcome)?;
    }
    attempt
}

pub(crate) fn receive(
    db: &mut Database,
    session: &Session,
    sender: &str,
    event_id: &str,
    json: &str,
) -> Result<()> {
    // Validate that the delivery handoff is the same event that will be committed.
    let peer = lookup(session, sender)?;
    db.decrypt_signal_delivery(&peer, json.as_bytes(), SystemTime::now(), Some(event_id))?;
    acknowledge(session, sender, event_id)
}

fn acknowledge(session: &Session, sender: &str, event_id: &str) -> Result<()> {
    match session.request(Request::Ack {
        event_id: event_id.into(),
        sender_void_id: sender.into(),
    })? {
        Reply::Acked {
            event_id: confirmed,
            sender_void_id,
        } if confirmed == event_id && sender_void_id == sender => Ok(()),
        _ => Err(RequestError::Interrupted.into()),
    }
}
