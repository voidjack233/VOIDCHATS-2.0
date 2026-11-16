//! Messaging runs on the existing SQLCipher worker. Relay requests block only
//! this worker, and no network operation runs inside a database transaction.

use crate::{
    account::valid_void_id,
    relay::{AccountState, Reply, Request, RequestError, Session},
    storage::{
        Database, Error as StorageError,
        account_status::{AccountStatus, VerifiedPeerStatus},
        messaging::SendState,
        signal::peer::TrustedPeer,
    },
};
use std::time::SystemTime;

#[derive(Debug)]
pub(crate) enum Error {
    Storage(StorageError),
    Relay(RequestError),
    Offline,
    UnconfirmedPeerStatus,
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
            Self::Offline => "The message is saved locally. Choose Resend when messaging is ready.",
            Self::UnconfirmedPeerStatus => {
                "The relay reported a deleted account. Reconnect to verify the peer’s status."
            }
            Self::Relay(RequestError::Interrupted) => {
                "The relay response was interrupted. Check the connection and try again."
            }
            Self::Relay(RequestError::Rejected { command, code }) => {
                match (*command, code.as_str()) {
                    (_, "authentication_failed" | "unauthorized") => {
                        "Relay authentication failed. Use Reconnect to try signing in again."
                    }
                    (_, "not_found") => "The peer’s VOID-ID was not found.",
                    ("send", "duplicate_event") => {
                        "The relay already has this event. Acceptance is unconfirmed."
                    }
                    ("prekeys_claim", "prekeys_invalid") => {
                        "The relay rejected the peer’s Signal prekey claim. Check the recipient’s VOID-ID."
                    }
                    ("prekeys_claim", "prekeys_conflict") => {
                        "The saved prekey claim conflicts with the relay’s record. Keep your VOID data."
                    }
                    ("prekeys_claim", "prekeys_unavailable") => {
                        "The peer’s public Signal prekeys are unavailable. Try again when they have set up messaging."
                    }
                    ("prekeys_claim", "prekey_claim_expired") => {
                        "The prekey claim expired. Choose Resend to request a new claim."
                    }
                    ("send", "queue_full") => {
                        "The relay’s encrypted queue is full. Choose Resend later."
                    }
                    (_, "prekeys_capacity" | "rate_limited" | "overloaded") => {
                        "The relay is at capacity. Try again later."
                    }
                    ("send", "invalid_frame") => {
                        "The relay rejected the encrypted message format. Check for a VOID update."
                    }
                    ("send", _) => {
                        "The relay could not accept the encrypted message. Choose Resend later."
                    }
                    ("lookup", _) => "The relay could not look up this VOID-ID. Try again later.",
                    ("prekeys_claim", _) => {
                        "The relay could not provide the peer’s Signal prekeys. Try again later."
                    }
                    ("ack", _) => {
                        "The relay could not confirm acknowledgement of the locally saved message."
                    }
                    _ => "The relay could not complete the operation.",
                }
            }
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

pub(crate) fn refresh_peer_status(
    db: &Database,
    session: &Session,
    void_id: &str,
) -> Result<AccountStatus> {
    let cached = db.peer_account_status(void_id)?;
    if cached == AccountStatus::Deleted {
        return Ok(cached);
    }
    let request_id = format!("status:{:016x}", rand::random::<u64>());
    let reply = session.request(Request::AccountStatus {
        request_id,
        void_id: void_id.into(),
    })?;
    // Session::request matches both the request ID and the peer's VOID-ID.
    let Reply::AccountStatus { state, .. } = reply else {
        return Err(RequestError::Interrupted.into());
    };
    let evidence = match state {
        AccountState::Active => VerifiedPeerStatus::Active,
        AccountState::Deleted => VerifiedPeerStatus::Deleted,
        AccountState::Unknown => return Ok(AccountStatus::Unknown),
    };
    db.record_verified_peer_status(void_id, evidence)?;
    Ok(db.peer_account_status(void_id)?)
}

fn peer_request(
    db: &Database,
    session: &Session,
    void_id: &str,
    request: Request,
) -> Result<Reply> {
    match session.request(request) {
        // A generic rejection can mean the requester itself was revoked. Require a
        // correlated peer snapshot before persisting a terminal status for the target.
        Err(RequestError::Rejected { code, .. }) if code == "account_deleted" => {
            match refresh_peer_status(db, session, void_id) {
                Ok(AccountStatus::Deleted) => Err(StorageError::PeerDeleted.into()),
                Err(error @ Error::Storage(_)) => Err(error),
                _ => Err(Error::UnconfirmedPeerStatus),
            }
        }
        result => Ok(result?),
    }
}

fn lookup(db: &Database, session: &Session, void_id: &str) -> Result<TrustedPeer> {
    if !valid_void_id(void_id) {
        return Err(StorageError::SignalPeer.into());
    }
    match peer_request(
        db,
        session,
        void_id,
        Request::Lookup {
            void_id: void_id.into(),
        },
    )? {
        Reply::Person {
            void_id: reported,
            public_key,
        } if reported == void_id => Ok(TrustedPeer::from_trusted_lookup(void_id, &public_key)?),
        _ => Err(RequestError::Interrupted.into()),
    }
}

pub(crate) fn send(db: &mut Database, session: Option<&Session>, event_id: &str) -> Result<()> {
    let operation = db.outgoing_operation(event_id)?;
    if operation.state == "sent" || db.has_verified_delivery(event_id)? {
        return Ok(());
    }
    if db.peer_account_status(&operation.peer)? == AccountStatus::Deleted {
        return Err(StorageError::PeerDeleted.into());
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
        let peer = lookup(db, session, &operation.peer)?;
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
                let bundle = peer_request(
                    db,
                    session,
                    &operation.peer,
                    Request::PrekeysClaim {
                        request_id: request_id.clone(),
                        void_id: operation.peer.clone(),
                    },
                );
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
                        if matches!(&error, Error::Relay(RequestError::Rejected { code, .. }) if code == "prekey_claim_expired")
                        {
                            db.clear_expired_claim(event_id)?;
                        }
                        return Err(error);
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
        let reply = peer_request(
            db,
            session,
            &operation.peer,
            Request::Send {
                event_id: event_id.into(),
                to_void_id: operation.peer.clone(),
                payload_base64,
            },
        );
        match reply {
            Ok(Reply::Accepted { event_id: accepted }) if accepted == event_id => {
                db.set_send_state(event_id, SendState::Sent)?;
                Ok(())
            }
            Err(error) => {
                if operation.state != "unconfirmed"
                    && matches!(&error, Error::Relay(RequestError::Rejected { code, .. })
                        if matches!(code.as_str(), "invalid_frame" | "not_found" | "queue_full" | "rate_limited"))
                {
                    outcome = SendState::Failed;
                }
                Err(error)
            }
            _ => Err(RequestError::Interrupted.into()),
        }
    })();
    if attempt.is_err() {
        // If this also fails, the previously committed state remains conservative.
        db.set_send_state(event_id, outcome)?;
    }
    attempt
}

pub(crate) fn upload_due_receipt(db: &mut Database, session: &Session, now: i64) -> Result<()> {
    if !session.is_active() {
        return Ok(());
    }
    let Some(upload) = db.claim_receipt(now)? else {
        return Ok(());
    };
    let event_id = upload.event_id.clone();
    let peer = upload.peer.clone();
    if db.peer_account_status(&peer)? == AccountStatus::Deleted {
        return Ok(db.finish_receipt(&event_id)?);
    }
    let payload_base64 = String::from_utf8(db.encrypt_signal_receipt(upload)?)
        .map_err(|_| StorageError::Unreadable)?;
    let reply = peer_request(
        db,
        session,
        &peer,
        Request::Send {
            event_id: event_id.clone(),
            to_void_id: peer.clone(),
            payload_base64,
        },
    );
    match reply {
        Ok(Reply::Accepted { event_id: accepted }) if accepted == event_id => {
            Ok(db.finish_receipt(&event_id)?)
        }
        Err(Error::Relay(RequestError::Rejected { code, .. }))
            if matches!(code.as_str(), "invalid_frame" | "not_found") =>
        {
            Ok(db.finish_receipt(&event_id)?)
        }
        Err(Error::Storage(StorageError::PeerDeleted)) => Ok(db.finish_receipt(&event_id)?),
        Err(error) => Err(error),
        _ => Err(RequestError::Interrupted.into()),
    }
}

pub(crate) fn publication_failure(error: &Error) -> (&'static str, bool) {
    match error {
        Error::Relay(RequestError::Rejected { code, .. }) => match code.as_str() {
            "prekeys_conflict" => (
                "The relay’s Signal keys conflict with this device’s keys. Keep your VOID data. Messaging requires the original Signal store for this account.",
                false,
            ),
            "prekeys_invalid" | "invalid_frame" => (
                "The relay rejected this device’s public Signal keys as invalid. Keep your VOID data and check for a VOID update.",
                false,
            ),
            "prekeys_capacity" | "rate_limited" | "overloaded" => (
                "The relay is at capacity for public Signal key publication. Try again later.",
                true,
            ),
            "prekeys_unavailable" | "unavailable" => (
                "The relay’s public Signal key service is unavailable. Try again later.",
                true,
            ),
            "authentication_failed" | "unauthorized" => (
                "Relay authentication failed during Signal key publication. Use Reconnect to try again.",
                false,
            ),
            _ => (
                "The relay could not publish this device’s public Signal keys.",
                false,
            ),
        },
        Error::Relay(RequestError::Interrupted) | Error::Offline => (
            "Signal key publication was interrupted. VOID will retry when it reconnects.",
            false,
        ),
        Error::UnconfirmedPeerStatus => (error.message(), false),
        Error::Storage(StorageError::Database | StorageError::Unavailable | StorageError::Busy) => {
            (
                "VOID could not save the Signal key publication in local storage. Keep your VOID data and retry messaging.",
                true,
            )
        }
        Error::Storage(_) => (
            "VOID could not verify the local Signal keys for publication. Keep your VOID data and check your local storage.",
            false,
        ),
    }
}

pub(crate) fn receive(
    db: &mut Database,
    session: &Session,
    sender: &str,
    event_id: &str,
    json: &str,
) -> Result<()> {
    // Validate that the delivery handoff is the same event that will be committed.
    let peer = lookup(db, session, sender)?;
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
