//! VOID's version 2 Signal wire format. The relay carries a Base64-encoded UTF-8
//! envelope that correlates the event ID and sender with the relay frame and
//! identifies the recipient. It contains the sender's signed account binding and
//! Base64-encoded ciphertext serialized by libsignal.

use super::{DEVICE_ID, Error, Result};
use crate::{
    account::{timestamp_ms, valid_void_id},
    relay::ApplicationProtocolVersion,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use libsignal_protocol::{CiphertextMessage, CiphertextMessageType};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use zeroize::Zeroizing;

const PROTOCOL_VERSION: u8 = 2;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_EVENT_BYTES: usize = 1_410_000;
const MAX_BODY_BYTES: usize = 60_000;
const MAX_CONTENT_BYTES: usize = 6 * MAX_BODY_BYTES + 512;
const MAX_SAFE_TIMESTAMP: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SignalBinding {
    pub(super) device_id: u32,
    pub(super) registration_id: u32,
    pub(super) identity_key: [u8; 33],
    pub(super) account_signature: [u8; 64],
}

impl SignalBinding {
    fn valid_shape(&self) -> bool {
        self.device_id == DEVICE_ID
            && (1..=16_380).contains(&self.registration_id)
            && self.identity_key[0] == 0x05
    }
}

#[derive(Clone, Copy)]
pub(super) struct Routing<'a> {
    pub(super) event_id: &'a str,
    pub(super) sender: &'a str,
    pub(super) recipient: &'a str,
}

impl Routing<'_> {
    fn valid(self) -> bool {
        valid_event_id(self.event_id)
            && valid_void_id(self.sender)
            && valid_void_id(self.recipient)
            && self.sender != self.recipient
    }

    fn matches(self, event_id: &str, sender: &str, recipient: &str) -> bool {
        self.event_id == event_id && self.sender == sender && self.recipient == recipient
    }
}

pub(super) struct ParsedEvent {
    pub(super) min_protocol_version: ApplicationProtocolVersion,
    pub(super) event_id: String,
    pub(super) sender: String,
    pub(super) payload_base64: String,
    pub(super) binding: SignalBinding,
    pub(super) message_type: CiphertextMessageType,
    pub(super) ciphertext: Vec<u8>,
    pub(super) expires_at_ms: i64,
}

pub(super) struct ParsedContent {
    pub(super) body: String,
    pub(super) created_at: i64,
    pub(super) requests_receipt: bool,
}

pub(super) enum Content {
    Message(ParsedContent),
    Receipt(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayEvent {
    #[serde(rename = "type")]
    kind: String,
    event_id: String,
    from_void_id: String,
    payload_base64: String,
    created_at: String,
    expires_at: String,
    #[serde(default, deserialize_with = "present_protocol_version")]
    min_protocol_version: Option<ApplicationProtocolVersion>,
}

// An absent legacy field is valid; an explicitly present null is not a version.
fn present_protocol_version<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<ApplicationProtocolVersion>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    ApplicationProtocolVersion::deserialize(deserializer).map(Some)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EncryptedPayload<'a> {
    device_id: u32,
    registration_id: u32,
    identity_key: String,
    account_signature: String,
    v: u8,
    #[serde(borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow)]
    client_message_id: Cow<'a, str>,
    #[serde(borrow)]
    from_void_id: Cow<'a, str>,
    #[serde(borrow)]
    to_void_id: Cow<'a, str>,
    message_type: u8,
    #[serde(borrow)]
    ciphertext_base64: Cow<'a, str>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageContent<'a> {
    v: u8,
    #[serde(borrow)]
    client_message_id: Cow<'a, str>,
    #[serde(borrow)]
    from_void_id: Cow<'a, str>,
    #[serde(borrow)]
    to_void_id: Cow<'a, str>,
    #[serde(borrow)]
    body: Cow<'a, str>,
    created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_delivery_receipt: Option<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptContent<'a> {
    v: u8,
    #[serde(rename = "type")]
    kind: Cow<'a, str>,
    client_message_id: Cow<'a, str>,
    original_message_id: Cow<'a, str>,
    from_void_id: Cow<'a, str>,
    to_void_id: Cow<'a, str>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Plaintext<'a> {
    #[serde(borrow)]
    Message(MessageContent<'a>),
    Receipt(ReceiptContent<'a>),
}

pub(super) fn valid_event_id(id: &str) -> bool {
    id.len() == 36
        && id != "00000000-0000-0000-0000-000000000000"
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

pub(super) fn decode_base64(encoded: &str, max_decoded: usize) -> Result<Vec<u8>> {
    if encoded.is_empty() || encoded.len() > max_decoded.div_ceil(3) * 4 {
        return Err(Error::SignalEnvelope);
    }
    // VOID uses standard Base64 with canonical padding and zero trailing bits.
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| Error::SignalEnvelope)?;
    if bytes.is_empty() || bytes.len() > max_decoded {
        return Err(Error::SignalEnvelope);
    }
    Ok(bytes)
}

pub(super) fn decode_exact_base64<const N: usize>(encoded: &str) -> Result<[u8; N]> {
    decode_base64(encoded, N)?
        .try_into()
        .map_err(|_| Error::SignalEnvelope)
}

pub(super) fn decode_event(event_json: &[u8], expected_recipient: &str) -> Result<ParsedEvent> {
    if event_json.len() > MAX_EVENT_BYTES {
        return Err(Error::SignalEnvelope);
    }
    let event: RelayEvent =
        serde_json::from_slice(event_json).map_err(|_| Error::SignalEnvelope)?;
    let min_protocol_version = match (event.kind.as_str(), event.min_protocol_version) {
        ("event", None) => ApplicationProtocolVersion::V1,
        ("event_versioned", Some(version)) if version > ApplicationProtocolVersion::V1 => version,
        _ => return Err(Error::SignalEnvelope),
    };
    let routing = Routing {
        event_id: &event.event_id,
        sender: &event.from_void_id,
        recipient: expected_recipient,
    };
    let created_at_ms = timestamp_ms(&event.created_at).ok_or(Error::SignalEnvelope)?;
    let expires_at_ms = timestamp_ms(&event.expires_at).ok_or(Error::SignalEnvelope)?;
    if !routing.valid() || expires_at_ms <= created_at_ms {
        return Err(Error::SignalEnvelope);
    }

    let bytes = decode_base64(&event.payload_base64, MAX_PAYLOAD_BYTES)?;
    let payload: EncryptedPayload =
        serde_json::from_slice(&bytes).map_err(|_| Error::SignalEnvelope)?;
    if payload.v != PROTOCOL_VERSION
        || payload.kind != "signal"
        || !routing.matches(
            &payload.client_message_id,
            &payload.from_void_id,
            &payload.to_void_id,
        )
    {
        return Err(Error::SignalEnvelope);
    }
    let binding = SignalBinding {
        device_id: payload.device_id,
        registration_id: payload.registration_id,
        identity_key: decode_exact_base64(&payload.identity_key)?,
        account_signature: decode_exact_base64(&payload.account_signature)?,
    };
    if !binding.valid_shape() {
        return Err(Error::SignalEnvelope);
    }
    let message_type = match CiphertextMessageType::try_from(payload.message_type) {
        Ok(kind @ (CiphertextMessageType::Whisper | CiphertextMessageType::PreKey)) => kind,
        _ => return Err(Error::SignalEnvelope),
    };
    let ciphertext = decode_base64(&payload.ciphertext_base64, MAX_PAYLOAD_BYTES)?;
    Ok(ParsedEvent {
        min_protocol_version,
        event_id: event.event_id,
        sender: event.from_void_id,
        payload_base64: event.payload_base64,
        binding,
        message_type,
        ciphertext,
        expires_at_ms,
    })
}

pub(super) fn encode_payload(
    routing: Routing<'_>,
    binding: &SignalBinding,
    ciphertext: &CiphertextMessage,
) -> Result<Vec<u8>> {
    let message_type = ciphertext.message_type();
    let ciphertext = ciphertext.serialize();
    if !routing.valid()
        || !binding.valid_shape()
        || !matches!(
            message_type,
            CiphertextMessageType::Whisper | CiphertextMessageType::PreKey
        )
        || !(1..=MAX_PAYLOAD_BYTES).contains(&ciphertext.len())
    {
        return Err(Error::SignalEnvelope);
    }
    let payload = EncryptedPayload {
        device_id: binding.device_id,
        registration_id: binding.registration_id,
        identity_key: STANDARD.encode(binding.identity_key),
        account_signature: STANDARD.encode(binding.account_signature),
        v: PROTOCOL_VERSION,
        kind: "signal".into(),
        client_message_id: routing.event_id.into(),
        from_void_id: routing.sender.into(),
        to_void_id: routing.recipient.into(),
        message_type: message_type as u8,
        ciphertext_base64: STANDARD.encode(ciphertext).into(),
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| Error::SignalEnvelope)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::SignalEnvelope);
    }
    Ok(STANDARD.encode(bytes).into_bytes())
}

pub(crate) fn valid_content(body: &str, created_at: i64) -> bool {
    (0..=MAX_SAFE_TIMESTAMP).contains(&created_at)
        && body.len() <= MAX_BODY_BYTES
        && !body.contains('\0')
        && !body.trim().is_empty()
}

pub(super) fn encode_content(
    routing: Routing<'_>,
    body: &str,
    created_at: i64,
) -> Result<Zeroizing<Vec<u8>>> {
    if !routing.valid() || !valid_content(body, created_at) {
        return Err(Error::SignalEnvelope);
    }
    let content = MessageContent {
        v: PROTOCOL_VERSION,
        client_message_id: routing.event_id.into(),
        from_void_id: routing.sender.into(),
        to_void_id: routing.recipient.into(),
        body: body.into(),
        created_at,
        request_delivery_receipt: Some(1),
    };
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &content).map_err(|_| Error::SignalEnvelope)?;
    Ok(bytes)
}

pub(super) fn encode_receipt(routing: Routing<'_>, original: &str) -> Result<Zeroizing<Vec<u8>>> {
    if !routing.valid() || !valid_event_id(original) || routing.event_id == original {
        return Err(Error::SignalEnvelope);
    }
    let content = ReceiptContent {
        v: 1,
        kind: "delivery_receipt".into(),
        client_message_id: routing.event_id.into(),
        original_message_id: original.into(),
        from_void_id: routing.sender.into(),
        to_void_id: routing.recipient.into(),
    };
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &content).map_err(|_| Error::SignalEnvelope)?;
    Ok(bytes)
}

pub(super) fn decode_content(routing: Routing<'_>, bytes: &[u8]) -> Result<Content> {
    if bytes.len() > MAX_CONTENT_BYTES || !routing.valid() {
        return Err(Error::SignalEnvelope);
    }
    let content = match serde_json::from_slice(bytes).map_err(|_| Error::SignalEnvelope)? {
        Plaintext::Message(message) => message,
        Plaintext::Receipt(receipt) => {
            if receipt.v != 1
                || receipt.kind != "delivery_receipt"
                || !routing.matches(
                    &receipt.client_message_id,
                    &receipt.from_void_id,
                    &receipt.to_void_id,
                )
                || !valid_event_id(&receipt.original_message_id)
                || receipt.original_message_id == routing.event_id
            {
                return Err(Error::SignalEnvelope);
            }
            return Ok(Content::Receipt(receipt.original_message_id.into_owned()));
        }
    };
    if content.v != PROTOCOL_VERSION
        || !routing.matches(
            &content.client_message_id,
            &content.from_void_id,
            &content.to_void_id,
        )
        || !content
            .request_delivery_receipt
            .is_none_or(|requested| requested == 1)
        || !valid_content(&content.body, content.created_at)
    {
        return Err(Error::SignalEnvelope);
    }
    Ok(Content::Message(ParsedContent {
        body: content.body.into_owned(),
        created_at: content.created_at,
        requests_receipt: content.request_delivery_receipt == Some(1),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ROUTING: Routing<'_> = Routing {
        event_id: "7abeaf02-e740-430f-8eb7-b8ceefde11ad",
        sender: "110559210000000001",
        recipient: "110559210000000002",
    };

    #[test]
    fn react_native_v1_receipt_shape_and_routing() {
        let original = "4ee61607-e589-43f1-87cc-2076cf8a392a";
        let fixture = br#"{"v":1,"type":"delivery_receipt","client_message_id":"7abeaf02-e740-430f-8eb7-b8ceefde11ad","original_message_id":"4ee61607-e589-43f1-87cc-2076cf8a392a","from_void_id":"110559210000000001","to_void_id":"110559210000000002"}"#;
        let bytes = encode_receipt(ROUTING, original).unwrap();
        assert_eq!(&bytes[..], fixture);
        assert!(
            matches!(decode_content(ROUTING, fixture), Ok(Content::Receipt(id)) if id == original)
        );

        let expected: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        for (field, value) in [
            ("v", json!(2)),
            ("from_void_id", json!("42")),
            ("to_void_id", json!("42")),
            ("original_message_id", json!(ROUTING.event_id)),
            ("original_message_id", json!("not-a-uuid")),
            ("request_delivery_receipt", json!(1)),
        ] {
            let mut bad = expected.clone();
            bad[field] = value;
            assert!(decode_content(ROUTING, &serde_json::to_vec(&bad).unwrap()).is_err());
        }
    }

    #[test]
    fn void_message_content_validates_shape_routing_and_limits() {
        let fixture = r#"{"v":2,"client_message_id":"7abeaf02-e740-430f-8eb7-b8ceefde11ad","from_void_id":"110559210000000001","to_void_id":"110559210000000002","body":"Café / moon\nhello","created_at":1783332000000,"request_delivery_receipt":1}"#;
        let Content::Message(content) = decode_content(ROUTING, fixture.as_bytes()).unwrap() else {
            panic!("not an application message");
        };
        assert!(content.requests_receipt);
        assert_eq!(content.body, "Café / moon\nhello");
        assert_eq!(content.created_at, 1_783_332_000_000);
        let encoded = encode_content(ROUTING, &content.body, content.created_at).unwrap();
        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
            expected
        );

        let wrong_sender = Routing {
            sender: "42",
            ..ROUTING
        };
        assert!(decode_content(wrong_sender, fixture.as_bytes()).is_err());
        assert!(
            decode_content(
                ROUTING,
                fixture.replace("\"v\":2", "\"v\":2,\"v\":2").as_bytes()
            )
            .is_err()
        );
        assert!(encode_content(ROUTING, "\t\u{a0}\u{85}", 0).is_err());
        assert!(encode_content(ROUTING, "a\0b", 0).is_err());
        assert!(encode_content(ROUTING, "a", MAX_SAFE_TIMESTAMP + 1).is_err());
        assert!(encode_content(ROUTING, &"é".repeat(MAX_BODY_BYTES / 2), 0).is_ok());
        assert!(encode_content(ROUTING, &"é".repeat(MAX_BODY_BYTES / 2 + 1), 0).is_err());
    }

    #[test]
    fn void_envelope_requires_correlated_identifiers_and_canonical_base64() {
        // This fixture exercises framing only. libsignal validates the actual ciphertext.
        let mut identity_key = [0; 33];
        identity_key[0] = 5;
        let mut payload = json!({
            "device_id": 1,
            "registration_id": 16380,
            "identity_key": STANDARD.encode(identity_key),
            "account_signature": STANDARD.encode([0; 64]),
            "v": 2,
            "kind": "signal",
            "client_message_id": ROUTING.event_id,
            "from_void_id": ROUTING.sender,
            "to_void_id": ROUTING.recipient,
            "message_type": 3,
            "ciphertext_base64": "/w==",
        });
        let event = |payload: &serde_json::Value| {
            serde_json::to_vec(&json!({
                "type": "event",
                "event_id": ROUTING.event_id,
                "from_void_id": ROUTING.sender,
                "payload_base64": STANDARD.encode(payload.to_string()),
                "created_at": "2026-07-06T10:00:00.001Z",
                "expires_at": "2026-07-07T10:00:00.001Z",
            }))
            .unwrap()
        };

        let parsed = decode_event(&event(&payload), ROUTING.recipient).unwrap();
        assert_eq!(parsed.min_protocol_version, ApplicationProtocolVersion::V1);
        assert_eq!(parsed.event_id, ROUTING.event_id);
        assert_eq!(parsed.sender, ROUTING.sender);
        assert_eq!(parsed.payload_base64, STANDARD.encode(payload.to_string()));
        assert_eq!(parsed.message_type, CiphertextMessageType::PreKey);
        assert_eq!(parsed.binding.identity_key, identity_key);
        assert_eq!(parsed.ciphertext, [255]);
        assert!(decode_event(&event(&payload), "42").is_err());

        let legacy: serde_json::Value = serde_json::from_slice(&event(&payload)).unwrap();
        let mut versioned = legacy.clone();
        versioned["type"] = json!("event_versioned");
        versioned["min_protocol_version"] = json!("2");
        let parsed =
            decode_event(&serde_json::to_vec(&versioned).unwrap(), ROUTING.recipient).unwrap();
        assert_eq!(
            parsed.min_protocol_version,
            ApplicationProtocolVersion::parse("2").unwrap()
        );
        assert_eq!(parsed.payload_base64, STANDARD.encode(payload.to_string()));
        for value in [
            json!(null),
            json!(1),
            json!("1"),
            json!("0"),
            json!("02"),
            json!("+2"),
            json!(" 2"),
            json!("2147483648"),
        ] {
            versioned["min_protocol_version"] = value;
            assert!(
                decode_event(&serde_json::to_vec(&versioned).unwrap(), ROUTING.recipient).is_err()
            );
        }
        versioned
            .as_object_mut()
            .unwrap()
            .remove("min_protocol_version");
        assert!(decode_event(&serde_json::to_vec(&versioned).unwrap(), ROUTING.recipient).is_err());
        for value in [json!(null), json!("1"), json!("2")] {
            let mut malformed_legacy = legacy.clone();
            malformed_legacy["min_protocol_version"] = value;
            assert!(
                decode_event(
                    &serde_json::to_vec(&malformed_legacy).unwrap(),
                    ROUTING.recipient
                )
                .is_err()
            );
        }

        payload["device_id"] = 2.into();
        assert!(decode_event(&event(&payload), ROUTING.recipient).is_err());
        payload["device_id"] = 1.into();
        payload["message_type"] = 7.into();
        assert!(decode_event(&event(&payload), ROUTING.recipient).is_err());
        payload["message_type"] = 2.into();
        for noncanonical in ["/w", "_w==", "/x=="] {
            payload["ciphertext_base64"] = noncanonical.into();
            assert!(decode_event(&event(&payload), ROUTING.recipient).is_err());
        }
    }
}
