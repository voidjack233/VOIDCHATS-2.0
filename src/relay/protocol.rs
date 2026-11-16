use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::account::{timestamp_ms, valid_token, valid_void_id};

pub(super) const MAX_FRAME_SIZE: usize = 1_410_000;
const MAX_CHALLENGE_MESSAGE: usize = 4096;
const MAX_CHALLENGE_LIFETIME_MS: i64 = 30_000;
pub(super) const DELETION_CHALLENGE_LIFETIME: std::time::Duration =
    std::time::Duration::from_secs(30);
const DELETION_CLOCK_SKEW_MS: i64 = 10_000;
const MAX_DELIVERY_PAYLOAD: usize = 1_048_576;

pub(super) enum ServerFrame {
    Challenge(Challenge),
    AuthOk(String),
    DeleteChallenge(Challenge),
    AccountDeleted {
        void_id: String,
        deleted_at: String,
    },
    Error(String),
    Delivery {
        sender: String,
        event_id: String,
        expiry_ms: i64,
    },
    OnlineDelivery,
    Reply(Reply),
    PrekeyError {
        request_id: String,
        code: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Request {
    AccountStatus {
        request_id: String,
        void_id: String,
    },
    Lookup {
        void_id: String,
    },
    PrekeysPublish {
        publication_id: String,
        payload_base64: String,
    },
    PrekeysClaim {
        request_id: String,
        void_id: String,
    },
    Send {
        event_id: String,
        to_void_id: String,
        payload_base64: String,
    },
    Ack {
        event_id: String,
        sender_void_id: String,
    },
}

#[derive(Debug)]
pub(crate) enum Reply {
    AccountStatus {
        request_id: String,
        void_id: String,
        state: AccountState,
    },
    Person {
        void_id: String,
        public_key: [u8; 32],
    },
    Published {
        publication_id: String,
    },
    Bundle {
        request_id: String,
        void_id: String,
        payload_base64: String,
    },
    Accepted {
        event_id: String,
    },
    Acked {
        event_id: String,
        sender_void_id: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountState {
    Active,
    Deleted,
    Unknown,
}

impl Request {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::AccountStatus { .. } => "account_status",
            Self::Lookup { .. } => "lookup",
            Self::PrekeysPublish { .. } => "prekeys_publish",
            Self::PrekeysClaim { .. } => "prekeys_claim",
            Self::Send { .. } => "send",
            Self::Ack { .. } => "ack",
        }
    }

    pub(super) fn matches(&self, reply: &Reply) -> bool {
        match (self, reply) {
            (
                Self::AccountStatus {
                    request_id,
                    void_id,
                },
                Reply::AccountStatus {
                    request_id: id,
                    void_id: peer,
                    ..
                },
            ) => request_id == id && void_id == peer,
            (Self::Lookup { void_id }, Reply::Person { void_id: id, .. }) => void_id == id,
            (
                Self::PrekeysPublish { publication_id, .. },
                Reply::Published { publication_id: id },
            ) => publication_id == id,
            (
                Self::PrekeysClaim {
                    request_id,
                    void_id,
                },
                Reply::Bundle {
                    request_id: id,
                    void_id: peer,
                    ..
                },
            ) => request_id == id && void_id == peer,
            (Self::Send { event_id, .. }, Reply::Accepted { event_id: id }) => event_id == id,
            (
                Self::Ack {
                    event_id,
                    sender_void_id,
                },
                Reply::Acked {
                    event_id: id,
                    sender_void_id: sender,
                },
            ) => event_id == id && sender_void_id == sender,
            _ => false,
        }
    }

    pub(super) fn prekey_id(&self) -> Option<&str> {
        match self {
            Self::PrekeysPublish { publication_id, .. } => Some(publication_id),
            Self::PrekeysClaim { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}

pub(super) struct Challenge {
    pub(super) id: String,
    encoded_message: String,
    expiry: String,
    expiry_ms: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DeletionChallengeError {
    Expired,
    ClockMismatch,
    Transcript,
}

impl Challenge {
    pub(super) fn validate(
        &self,
        audience: &str,
        void_id: &str,
        public_key: &str,
        now_ms: i64,
    ) -> Option<Vec<u8>> {
        if self.is_expired(now_ms)
            || self.expiry_ms > now_ms.saturating_add(MAX_CHALLENGE_LIFETIME_MS)
        {
            return None;
        }
        self.validate_transcript(
            "VOID-NOTHINGNESS:authenticate:v2",
            "",
            audience,
            void_id,
            public_key,
        )
    }

    pub(super) fn validate_deletion(
        &self,
        audience: &str,
        void_id: &str,
        public_key: &str,
        now_ms: i64,
    ) -> Result<Vec<u8>, DeletionChallengeError> {
        if self.is_expired(now_ms) {
            return Err(DeletionChallengeError::Expired);
        }
        let maximum_ms = DELETION_CHALLENGE_LIFETIME.as_millis() as i64 + DELETION_CLOCK_SKEW_MS;
        if self.expiry_ms > now_ms.saturating_add(maximum_ms) {
            return Err(DeletionChallengeError::ClockMismatch);
        }
        self.validate_transcript(
            "VOID:delete-account:v1",
            "DELETE\n",
            audience,
            void_id,
            public_key,
        )
        .ok_or(DeletionChallengeError::Transcript)
    }

    fn validate_transcript(
        &self,
        domain: &str,
        confirmation: &str,
        audience: &str,
        void_id: &str,
        public_key: &str,
    ) -> Option<Vec<u8>> {
        let bytes = STANDARD.decode(&self.encoded_message).ok()?;
        if !bytes.is_ascii() {
            return None;
        }

        // Preserve the server's exact timestamp representation and the HTTPS API audience.
        let audience = STANDARD.encode(audience);
        let prefix = format!(
            "{domain}\n{}\n{void_id}\n{public_key}\n{}\n{audience}\n{confirmation}",
            self.id, self.expiry,
        );
        let nonce = bytes.strip_prefix(prefix.as_bytes())?.strip_suffix(b"\n")?;
        if !valid_token(str::from_utf8(nonce).ok()?) {
            return None;
        }
        Some(bytes)
    }

    pub(super) fn is_expired(&self, now_ms: i64) -> bool {
        self.expiry_ms <= now_ms
    }
}

fn parse_challenge(id: String, message: String, expiry: String) -> Option<Challenge> {
    if !valid_token(&id)
        || message.len() > MAX_CHALLENGE_MESSAGE
        || !valid_base64(&message, MAX_CHALLENGE_MESSAGE)
    {
        return None;
    }
    Some(Challenge {
        id,
        encoded_message: message,
        expiry_ms: timestamp_ms(&expiry)?,
        expiry,
    })
}

pub(crate) fn valid_utc_timestamp(value: &str) -> bool {
    value.ends_with('Z') && timestamp_ms(value).is_some()
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireFrame {
    AuthChallenge {
        challenge_id: String,
        message: String,
        expires_at: String,
    },
    AuthOk {
        void_id: String,
    },
    AccountDeleteChallenge {
        challenge_id: String,
        message: String,
        expires_at: String,
    },
    AccountDeleted {
        void_id: String,
        deleted_at: String,
    },
    AccountStatus {
        request_id: String,
        void_id: String,
        state: AccountState,
        #[serde(default, deserialize_with = "present_string")]
        deleted_at: Option<String>,
    },
    Error {
        code: String,
    },
    Person {
        void_id: String,
        auth_public_key: String,
    },
    PrekeysPublished {
        publication_id: String,
        ec_remaining: String,
        kyber_remaining: String,
    },
    PrekeyBundle {
        request_id: String,
        void_id: String,
        payload_base64: String,
    },
    PrekeysError {
        request_id: String,
        code: String,
    },
    Accepted {
        event_id: String,
        created_at: String,
        expires_at: String,
    },
    AckOk {
        event_id: String,
        sender_void_id: String,
    },
    Event {
        event_id: String,
        from_void_id: String,
        payload_base64: String,
        created_at: String,
        expires_at: String,
    },
    OnlineEvent {
        exchange_id: String,
        from_void_id: String,
        payload_base64: String,
    },
}

pub(super) fn decode(text: &str) -> Option<ServerFrame> {
    if text.len() > MAX_FRAME_SIZE {
        return None;
    }

    match serde_json::from_str(text).ok()? {
        WireFrame::AuthChallenge {
            challenge_id,
            message,
            expires_at,
        } => parse_challenge(challenge_id, message, expires_at).map(ServerFrame::Challenge),
        WireFrame::AccountDeleteChallenge {
            challenge_id,
            message,
            expires_at,
        } if valid_utc_timestamp(&expires_at) => {
            parse_challenge(challenge_id, message, expires_at).map(ServerFrame::DeleteChallenge)
        }
        WireFrame::AccountDeleted {
            void_id,
            deleted_at,
        } if valid_void_id(&void_id) && valid_utc_timestamp(&deleted_at) => {
            Some(ServerFrame::AccountDeleted {
                void_id,
                deleted_at,
            })
        }
        WireFrame::AccountStatus {
            request_id,
            void_id,
            state,
            deleted_at,
        } if valid_request_id(&request_id)
            && valid_void_id(&void_id)
            && match state {
                AccountState::Deleted => deleted_at.as_deref().is_some_and(valid_utc_timestamp),
                AccountState::Active | AccountState::Unknown => deleted_at.is_none(),
            } =>
        {
            Some(ServerFrame::Reply(Reply::AccountStatus {
                request_id,
                void_id,
                state,
            }))
        }
        WireFrame::AuthOk { void_id } if valid_void_id(&void_id) => {
            Some(ServerFrame::AuthOk(void_id))
        }
        WireFrame::Error { code }
            if (1..=64).contains(&code.len())
                && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            Some(ServerFrame::Error(code))
        }
        WireFrame::Person {
            void_id,
            auth_public_key,
        } if valid_void_id(&void_id) && valid_base64(&auth_public_key, 32) => {
            let public_key = STANDARD.decode(auth_public_key).ok()?.try_into().ok()?;
            Some(ServerFrame::Reply(Reply::Person {
                void_id,
                public_key,
            }))
        }
        WireFrame::PrekeysPublished {
            publication_id,
            ec_remaining,
            kyber_remaining,
        } if valid_event_id(&publication_id)
            && valid_count(&ec_remaining)
            && valid_count(&kyber_remaining) =>
        {
            Some(ServerFrame::Reply(Reply::Published { publication_id }))
        }
        WireFrame::PrekeyBundle {
            request_id,
            void_id,
            payload_base64,
        } if valid_event_id(&request_id)
            && valid_void_id(&void_id)
            && valid_base64(&payload_base64, 32 * 1024) =>
        {
            Some(ServerFrame::Reply(Reply::Bundle {
                request_id,
                void_id,
                payload_base64,
            }))
        }
        WireFrame::PrekeysError { request_id, code }
            if valid_event_id(&request_id)
                && matches!(
                    code.as_str(),
                    "prekeys_invalid"
                        | "prekeys_conflict"
                        | "prekeys_capacity"
                        | "prekey_claim_expired"
                        | "prekeys_unavailable"
                        | "account_deleted"
                ) =>
        {
            Some(ServerFrame::PrekeyError { request_id, code })
        }
        WireFrame::Accepted {
            event_id,
            created_at,
            expires_at,
        } if valid_event_id(&event_id)
            && timestamp_ms(&expires_at)? > timestamp_ms(&created_at)? =>
        {
            Some(ServerFrame::Reply(Reply::Accepted { event_id }))
        }
        WireFrame::AckOk {
            event_id,
            sender_void_id,
        } if valid_event_id(&event_id) && valid_void_id(&sender_void_id) => {
            Some(ServerFrame::Reply(Reply::Acked {
                event_id,
                sender_void_id,
            }))
        }
        WireFrame::Event {
            event_id,
            from_void_id,
            payload_base64,
            created_at,
            expires_at,
        } if valid_event_id(&event_id)
            && valid_void_id(&from_void_id)
            && valid_base64(&payload_base64, MAX_DELIVERY_PAYLOAD)
            && timestamp_ms(&expires_at)? > timestamp_ms(&created_at)? =>
        {
            Some(ServerFrame::Delivery {
                sender: from_void_id,
                event_id,
                expiry_ms: timestamp_ms(&expires_at)?,
            })
        }
        WireFrame::OnlineEvent {
            exchange_id,
            from_void_id,
            payload_base64,
        } if (1..=64).contains(&exchange_id.len())
            && exchange_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            && valid_void_id(&from_void_id)
            && valid_base64(&payload_base64, MAX_DELIVERY_PAYLOAD) =>
        {
            Some(ServerFrame::OnlineDelivery)
        }
        _ => None,
    }
}

fn valid_request_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
}

fn valid_count(value: &str) -> bool {
    value
        .parse::<u8>()
        .is_ok_and(|count| count <= 64 && count.to_string() == value)
}

fn valid_event_id(id: &str) -> bool {
    id.len() == 36
        && id != "00000000-0000-0000-0000-000000000000"
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

fn valid_base64(encoded: &str, max_decoded: usize) -> bool {
    let len = encoded.len();
    if len == 0 || !len.is_multiple_of(4) || len > max_decoded.div_ceil(3) * 4 {
        return false;
    }
    let padding = if encoded.ends_with("==") {
        2
    } else {
        usize::from(encoded.ends_with('='))
    };
    if len / 4 * 3 - padding > max_decoded {
        return false;
    }
    let symbols = &encoded.as_bytes()[..len - padding];
    if !symbols
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
    {
        return false;
    }
    let last = symbols[symbols.len() - 1];
    match padding {
        2 => b"AQgw".contains(&last),
        1 => b"AEIMQUYcgkosw048".contains(&last),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    #[test]
    fn challenge_validates_exact_relay_signing_bytes_and_lifetime() {
        let audience = "https://example.invalid/void";
        let void_id = "101";
        let expiry = "2026-11-14T12:05:00.123456789+08:00";
        let expiry_ms = timestamp_ms(expiry).unwrap();
        let id = URL_SAFE_NO_PAD.encode([1; 32]);
        let nonce = URL_SAFE_NO_PAD.encode([2; 32]);
        let public_key = STANDARD.encode([3; 32]);
        let message = format!(
            "VOID-NOTHINGNESS:authenticate:v2\n{id}\n{void_id}\n{public_key}\n{expiry}\n{}\n{nonce}\n",
            STANDARD.encode(audience),
        );
        let frame = json!({
            "type": "auth_challenge",
            "challenge_id": id,
            "message": STANDARD.encode(&message),
            "expires_at": expiry,
        });
        let Some(ServerFrame::Challenge(challenge)) = decode(&frame.to_string()) else {
            panic!("expected a challenge");
        };
        let validate =
            |audience, void_id, key, now| challenge.validate(audience, void_id, key, now);
        let now = expiry_ms - MAX_CHALLENGE_LIFETIME_MS;
        assert_eq!(
            validate(audience, void_id, &public_key, now).unwrap(),
            message.as_bytes()
        );
        assert!(validate(audience, void_id, &public_key, now - 1).is_none());
        assert!(validate(audience, void_id, &public_key, expiry_ms).is_none());
        assert!(validate("wss://example.invalid/void/ws", void_id, &public_key, now).is_none());
        assert!(validate(audience, "102", &public_key, now).is_none());
    }

    #[test]
    fn deletion_validates_the_signed_confirmation_and_correlates_status() {
        let audience = "https://example.invalid/void";
        let id = URL_SAFE_NO_PAD.encode([1; 32]);
        let nonce = URL_SAFE_NO_PAD.encode([2; 32]);
        let key = STANDARD.encode([3; 32]);
        let expires_at = "2026-11-16T00:00:30.123456789Z";
        let expires_ms = timestamp_ms(expires_at).unwrap();
        let fields = [
            "VOID:delete-account:v1".into(),
            id.clone(),
            "123".into(),
            key.clone(),
            expires_at.into(),
            STANDARD.encode(audience),
            "DELETE".into(),
            nonce,
        ];
        let decode_challenge = |message: &str| {
            let frame = json!({"type": "account_delete_challenge", "challenge_id": id,
                "message": STANDARD.encode(message), "expires_at": expires_at});
            let Some(ServerFrame::DeleteChallenge(challenge)) = decode(&frame.to_string()) else {
                panic!("expected a deletion challenge");
            };
            challenge
        };
        let message = fields.join("\n") + "\n";
        let challenge = decode_challenge(&message);
        let lifetime = DELETION_CHALLENGE_LIFETIME.as_millis() as i64;
        let now = expires_ms - lifetime;
        assert_eq!(
            challenge
                .validate_deletion(audience, "123", &key, now)
                .unwrap(),
            message.as_bytes()
        );
        assert_eq!(
            challenge.validate_deletion(audience, "123", &key, now - DELETION_CLOCK_SKEW_MS),
            Ok(message.as_bytes().to_vec())
        );
        assert_eq!(
            challenge.validate_deletion(audience, "123", &key, now - DELETION_CLOCK_SKEW_MS - 1),
            Err(DeletionChallengeError::ClockMismatch)
        );
        assert_eq!(
            challenge.validate_deletion(audience, "123", &key, expires_ms),
            Err(DeletionChallengeError::Expired)
        );
        for index in 0..fields.len() {
            let mut tampered = fields.clone();
            tampered[index].push('x');
            let challenge = decode_challenge(&(tampered.join("\n") + "\n"));
            assert_eq!(
                challenge.validate_deletion(audience, "123", &key, now),
                Err(DeletionChallengeError::Transcript)
            );
        }
        assert_eq!(
            decode_challenge(message.trim_end()).validate_deletion(audience, "123", &key, now),
            Err(DeletionChallengeError::Transcript)
        );

        let request = Request::AccountStatus {
            request_id: "status:1".into(),
            void_id: "456".into(),
        };
        let response = |id: &str, peer: &str, state: &str| json!({"type": "account_status", "request_id": id, "void_id": peer, "state": state});
        let mut frame = response("status:1", "456", "deleted");
        assert!(decode(&frame.to_string()).is_none());
        frame["deleted_at"] = json!(expires_at);
        assert!(matches!(
            decode(&frame.to_string()),
            Some(ServerFrame::Reply(reply @ Reply::AccountStatus { state: AccountState::Deleted, .. }))
                if request.matches(&reply)
        ));
        for (id, peer) in [("status:2", "456"), ("status:1", "457")] {
            assert!(matches!(decode(&response(id, peer, "active").to_string()),
                Some(ServerFrame::Reply(reply)) if !request.matches(&reply)));
        }
        frame["state"] = json!("active");
        assert!(decode(&frame.to_string()).is_none());
        frame["deleted_at"] = json!(null);
        assert!(decode(&frame.to_string()).is_none());
    }

    #[test]
    fn strict_decoder_distinguishes_deliveries_and_authentication_frames() {
        assert!(
            matches!(decode(r#"{"type":"auth_ok","void_id":"1"}"#), Some(ServerFrame::AuthOk(id)) if id == "1")
        );
        assert!(decode(r#"{"type":"auth_ok","void_id":"1","extra":"x"}"#).is_none());
        assert!(decode(r#"{"type":"auth_ok","void_id":1}"#).is_none());
        assert!(decode(r#"{"type":"auth_ok","void_id":"1","void_id":"2"}"#).is_none());
        assert!(matches!(
            decode(r#"{"type":"error","code":"authentication_failed"}"#),
            Some(ServerFrame::Error(_))
        ));
        assert!(decode(r#"{"type":"error","code":"invalid-session"}"#).is_none());
        assert!(matches!(
            decode(
                r#"{"type":"online_event","exchange_id":"e","from_void_id":"2","payload_base64":"YQ=="}"#
            ),
            Some(ServerFrame::OnlineDelivery)
        ));
        assert!(decode(r#"{"type":"online_event","exchange_id":"e","from_void_id":"2","payload_base64":"YR=="}"#).is_none());
        let delivery = json!({
            "type": "event",
            "event_id": "00000000-0000-0000-0000-000000000001",
            "from_void_id": "2",
            "payload_base64": "YQ==",
            "created_at": "2026-11-14T04:05:00Z",
            "expires_at": "2026-11-14T04:05:01Z",
        });
        assert!(matches!(
            decode(&delivery.to_string()),
            Some(ServerFrame::Delivery { .. })
        ));
    }
}
