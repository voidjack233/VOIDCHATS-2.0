use base64::{Engine, engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}};
use chrono::DateTime;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{io::Read, time::{Duration, SystemTime, UNIX_EPOCH}};

pub(super) const API_BASE: &str = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz";
const MAX_RESPONSE: u64 = 16_384;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    pub void_id: String,
    pub created_at: String,
}

#[derive(Deserialize)]
struct ChallengeResponse {
    challenge_id: String,
    message: String,
    expires_at: String,
}

pub(super) struct Challenge {
    pub id: String,
    pub message: Vec<u8>,
    pub(super) expires_at_ms: i64,
}

impl Challenge {
    pub fn is_expired(&self) -> bool {
        self.expires_at_ms <= now_ms()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Network,
    Timeout,
    InvalidResponse,
    InvalidChallenge,
    ExpiredChallenge,
    Rejected(Rejection, Option<u64>),
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Rejection {
    InvalidInput,
    InvalidProof,
    KeyRegistered,
    RateLimited,
    CapacityReached,
}

pub(super) trait AccountApi {
    fn challenge(&self, public_key: &str) -> Result<Challenge, Failure>;
    fn register(&self, challenge_id: &str, signature: &str) -> Result<Registration, Failure>;
}

pub(super) struct HttpAccountApi(Client);

impl HttpAccountApi {
    pub fn new() -> Result<Self, Failure> {
        Client::builder()
            .user_agent("VOIDCHATS/2.0")
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map(Self)
            .map_err(|_| Failure::Network)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T, Failure> {
        let response = self.0.post(format!("{API_BASE}{path}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body)
            .send()
            .map_err(|error| if error.is_timeout() { Failure::Timeout } else { Failure::Network })?;
        let status = response.status();
        let retry_after = response.headers().get(reqwest::header::RETRY_AFTER)
            .and_then(|header| header.to_str().ok())
            .and_then(parse_retry_after);
        let mut bytes = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes)
            .map_err(|_| Failure::Network)?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(Failure::InvalidResponse);
        }
        if status == reqwest::StatusCode::CREATED {
            return serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse);
        }

        #[derive(Deserialize)]
        struct ErrorResponse { error: ErrorCode }
        #[derive(Deserialize)]
        struct ErrorCode { code: String }

        let rejection = serde_json::from_slice::<ErrorResponse>(&bytes).ok()
            .and_then(|row| match row.error.code.as_str() {
                "invalid_input" => Some(Rejection::InvalidInput),
                "invalid_proof" => Some(Rejection::InvalidProof),
                "key_registered" => Some(Rejection::KeyRegistered),
                "rate_limited" => Some(Rejection::RateLimited),
                "capacity_reached" => Some(Rejection::CapacityReached),
                _ => None,
            });
        // Only explicit client rejections can clear an in-flight registration.
        match rejection {
            Some(code) if status.is_client_error() => Err(Failure::Rejected(code, retry_after)),
            _ => Err(Failure::Unavailable),
        }
    }
}

impl AccountApi for HttpAccountApi {
    fn challenge(&self, public_key: &str) -> Result<Challenge, Failure> {
        #[derive(Serialize)]
        struct Request<'a> { auth_public_key: &'a str }
        let row = self.post("/challenges", &Request { auth_public_key: public_key })?;
        validate_challenge(row, public_key, now_ms())
    }

    fn register(&self, challenge_id: &str, signature: &str) -> Result<Registration, Failure> {
        #[derive(Serialize)]
        struct Request<'a> { challenge_id: &'a str, signature: &'a str }
        let row = self.post("/accounts", &Request { challenge_id, signature })?;
        validate_registration(row)
    }
}

pub(crate) fn valid_void_id(id: &str) -> bool {
    !id.starts_with('0') && id.bytes().all(|b| b.is_ascii_digit())
        && id.parse::<i64>().is_ok_and(|number| number > 0)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn timestamp_ms(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    let date = b.get(..19)?;
    for (i, &ch) in date.iter().enumerate() {
        let expected = match i {
            4 | 7 => b'-',
            10 => b'T',
            13 | 16 => b':',
            _ if ch.is_ascii_digit() => continue,
            _ => return None,
        };
        if ch != expected { return None; }
    }
    if &date[17..19] > b"59".as_slice() { return None; }
    let mut zone = &b[19..];
    if let Some(fraction) = zone.strip_prefix(b".") {
        let digits = fraction.iter().take_while(|ch| ch.is_ascii_digit()).count();
        if !(1..=9).contains(&digits) { return None; }
        zone = &fraction[digits..];
    }
    let valid_zone = match zone {
        b"Z" => true,
        [b'+' | b'-', h1, h2, b':', m1, m2] =>
            [*h1, *h2, *m1, *m2].iter().all(u8::is_ascii_digit)
                && [*h1, *h2] <= *b"23" && [*m1, *m2] <= *b"59",
        _ => false,
    };
    if !valid_zone { return None; }
    let ms = DateTime::parse_from_rfc3339(value).ok()?.timestamp_millis();
    (ms >= 0).then_some(ms)
}

fn parse_retry_after(value: &str) -> Option<u64> {
    let seconds = value.parse::<u64>().ok().or_else(|| {
        let date = DateTime::parse_from_rfc2822(value).ok()?;
        let delta = date.timestamp_millis().checked_sub(now_ms())?;
        u64::try_from(delta).ok().map(|ms| ms.div_ceil(1000))
    })?;
    (seconds > 0).then_some(seconds)
}

fn valid_token(value: &str) -> bool {
    value.len() == 43 && URL_SAFE_NO_PAD.decode(value).is_ok_and(|bytes| bytes.len() == 32)
}

fn validate_challenge(row: ChallengeResponse, public_key: &str, now: i64) -> Result<Challenge, Failure> {
    if !valid_token(&row.challenge_id) {
        return Err(Failure::InvalidChallenge);
    }
    let expires_at_ms = timestamp_ms(&row.expires_at).ok_or(Failure::InvalidChallenge)?;
    if expires_at_ms <= now {
        return Err(Failure::ExpiredChallenge);
    }
    if expires_at_ms > now + 10 * 60 * 1000 || row.message.len() > 4096 {
        return Err(Failure::InvalidChallenge);
    }
    let bytes = STANDARD.decode(&row.message).map_err(|_| Failure::InvalidChallenge)?;
    if !bytes.is_ascii() {
        return Err(Failure::InvalidChallenge);
    }
    let prefix = format!("VOID-NOTHINGNESS:register:v1\n{}\n{}\n{}\n\n",
        row.challenge_id, row.expires_at, public_key);
    let valid_nonce = bytes.strip_prefix(prefix.as_bytes())
        .and_then(|tail| tail.strip_suffix(b"\n"))
        .and_then(|nonce| std::str::from_utf8(nonce).ok())
        .is_some_and(valid_token);
    if !valid_nonce {
        return Err(Failure::InvalidChallenge);
    }
    Ok(Challenge { id: row.challenge_id, message: bytes, expires_at_ms })
}

fn validate_registration(row: serde_json::Value) -> Result<Registration, Failure> {
    if row.get("username").is_some() {
        return Err(Failure::InvalidResponse);
    }
    // As on Android, ignore added response fields other than the legacy username.
    let void_id = row.get("void_id").and_then(|id| id.as_str()).filter(|id| valid_void_id(id))
        .ok_or(Failure::InvalidResponse)?;
    let created_at = row.get("created_at").and_then(|date| date.as_str())
        .filter(|value| timestamp_ms(value).is_some()).ok_or(Failure::InvalidResponse)?;
    Ok(Registration { void_id: void_id.into(), created_at: created_at.into() })
}

pub(super) fn valid_registration(account: &Registration) -> bool {
    valid_void_id(&account.void_id) && timestamp_ms(&account.created_at).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn challenge_binds_domain_public_key_token_and_expiry() {
        let expiry = "2026-11-14T04:05:00.123456789Z";
        let expires = timestamp_ms(expiry).unwrap();
        let id = URL_SAFE_NO_PAD.encode([1; 32]);
        let nonce = URL_SAFE_NO_PAD.encode([2; 32]);
        let key = STANDARD.encode([3; 32]);
        let message = format!("VOID-NOTHINGNESS:register:v1\n{id}\n{expiry}\n{key}\n\n{nonce}\n");
        let make = |message: &str| ChallengeResponse {
            challenge_id: id.clone(), expires_at: expiry.into(), message: STANDARD.encode(message),
        };
        assert!(validate_challenge(make(&message), &key, expires - 1).is_ok());
        assert!(matches!(validate_challenge(make(&message), &key, expires), Err(Failure::ExpiredChallenge)));
        assert!(validate_challenge(make(&message), &key, expires - 600_001).is_err());
        assert!(validate_challenge(make(&message), &STANDARD.encode([4; 32]), expires - 1).is_err());
        assert!(validate_challenge(make(&message.replace("register:v1", "other:v1")), &key, expires - 1).is_err());
        assert!(validate_challenge(make(message.trim_end()), &key, expires - 1).is_err());
        assert!(validate_challenge(make(&message.replace(&nonce, &format!("{}B", &nonce[..42]))), &key, expires - 1).is_err());
    }

    #[test]
    fn registration_requires_a_server_id_and_valid_timestamp() {
        let created = "2026-11-14T12:05:00+08:00";
        assert!(validate_registration(json!({"void_id":"9223372036854775807","created_at":created,"extra":true})).is_ok());
        for id in [json!("01"), json!("9223372036854775808"), json!(123), json!("+123")] {
            assert!(validate_registration(json!({"void_id":id,"created_at":created})).is_err());
        }
        assert!(validate_registration(json!({"void_id":"123","created_at":created,"username":null})).is_err());
        for invalid in ["2026-02-30T04:05:00Z", "2026-11-14T04:05:60Z", "2026-11-14t04:05:00z", "2026-11-14T04:05:00.1234567891Z"] {
            assert!(timestamp_ms(invalid).is_none());
        }
    }
}
