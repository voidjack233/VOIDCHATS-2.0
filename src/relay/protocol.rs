use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;

use crate::account::{timestamp_ms, valid_token, valid_void_id};

pub(super) const MAX_FRAME_SIZE: usize = 1_410_000;
const MAX_CHALLENGE_MESSAGE: usize = 4096;
const MAX_CHALLENGE_LIFETIME_MS: i64 = 30_000;
const MAX_DELIVERY_PAYLOAD: usize = 1_048_576;

pub(super) enum ServerFrame {
    Challenge(Challenge),
    AuthOk(String),
    Error,
    Delivery,
}

pub(super) struct Challenge {
    pub(super) id: String,
    encoded_message: String,
    expiry: String,
    expiry_ms: i64,
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

        let bytes = STANDARD.decode(&self.encoded_message).ok()?;
        if !bytes.is_ascii() {
            return None;
        }

        // Preserve the server's exact timestamp representation and the HTTPS API audience.
        let audience = STANDARD.encode(audience);
        let prefix = format!(
            "VOID-NOTHINGNESS:authenticate:v2\n{}\n{void_id}\n{public_key}\n{}\n{audience}\n",
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
    Error {
        code: String,
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
        WireFrame::AuthChallenge { challenge_id, message, expires_at }
            if valid_token(&challenge_id)
                && message.len() <= MAX_CHALLENGE_MESSAGE
                && valid_base64(&message, MAX_CHALLENGE_MESSAGE) =>
        {
            let expiry_ms = timestamp_ms(&expires_at)?;
            Some(ServerFrame::Challenge(Challenge {
                id: challenge_id,
                encoded_message: message,
                expiry: expires_at,
                expiry_ms,
            }))
        }
        WireFrame::AuthOk { void_id } if valid_void_id(&void_id) => {
            Some(ServerFrame::AuthOk(void_id))
        }
        WireFrame::Error { code }
            if (1..=64).contains(&code.len())
                && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            Some(ServerFrame::Error)
        }
        // Recognize unsolicited relay envelopes so the connection can discard them.
        // This phase never decodes a delivery payload or acknowledges it to the relay.
        WireFrame::Event { event_id, from_void_id, payload_base64, created_at, expires_at }
            if valid_event_id(&event_id)
                && valid_void_id(&from_void_id)
                && valid_base64(&payload_base64, MAX_DELIVERY_PAYLOAD)
                && timestamp_ms(&expires_at)? > timestamp_ms(&created_at)? =>
        {
            Some(ServerFrame::Delivery)
        }
        WireFrame::OnlineEvent { exchange_id, from_void_id, payload_base64 }
            if (1..=64).contains(&exchange_id.len())
                && exchange_id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
                && valid_void_id(&from_void_id)
                && valid_base64(&payload_base64, MAX_DELIVERY_PAYLOAD) =>
        {
            Some(ServerFrame::Delivery)
        }
        _ => None,
    }
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
    let padding = if encoded.ends_with("==") { 2 } else { usize::from(encoded.ends_with('=')) };
    if len / 4 * 3 - padding > max_decoded {
        return false;
    }
    let symbols = &encoded.as_bytes()[..len - padding];
    if !symbols.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/') {
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
    fn challenge_validates_exact_android_signing_bytes_and_lifetime() {
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
        let validate = |audience, void_id, key, now| challenge.validate(audience, void_id, key, now);
        let now = expiry_ms - MAX_CHALLENGE_LIFETIME_MS;
        assert_eq!(validate(audience, void_id, &public_key, now).unwrap(), message.as_bytes());
        assert!(validate(audience, void_id, &public_key, now - 1).is_none());
        assert!(validate(audience, void_id, &public_key, expiry_ms).is_none());
        assert!(validate("wss://example.invalid/void/ws", void_id, &public_key, now).is_none());
        assert!(validate(audience, "102", &public_key, now).is_none());
    }

    #[test]
    fn strict_decoder_distinguishes_deliveries_and_authentication_frames() {
        assert!(matches!(decode(r#"{"type":"auth_ok","void_id":"1"}"#), Some(ServerFrame::AuthOk(id)) if id == "1"));
        assert!(decode(r#"{"type":"auth_ok","void_id":"1","extra":"x"}"#).is_none());
        assert!(decode(r#"{"type":"auth_ok","void_id":1}"#).is_none());
        assert!(decode(r#"{"type":"auth_ok","void_id":"1","void_id":"2"}"#).is_none());
        assert!(matches!(decode(r#"{"type":"error","code":"authentication_failed"}"#), Some(ServerFrame::Error)));
        assert!(decode(r#"{"type":"error","code":"invalid-session"}"#).is_none());
        assert!(matches!(decode(r#"{"type":"online_event","exchange_id":"e","from_void_id":"2","payload_base64":"YQ=="}"#), Some(ServerFrame::Delivery)));
        assert!(decode(r#"{"type":"online_event","exchange_id":"e","from_void_id":"2","payload_base64":"YR=="}"#).is_none());
        let delivery = json!({
            "type": "event",
            "event_id": "00000000-0000-0000-0000-000000000001",
            "from_void_id": "2",
            "payload_base64": "YQ==",
            "created_at": "2026-11-14T04:05:00Z",
            "expires_at": "2026-11-14T04:05:01Z",
        });
        assert!(matches!(decode(&delivery.to_string()), Some(ServerFrame::Delivery)));
    }
}
