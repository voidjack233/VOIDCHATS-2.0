use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::account::{API_BASE, valid_void_id};

fn binding_message(
    owner: &str,
    device_id: u32,
    registration_id: u32,
    signal_public_key: &[u8],
) -> Option<String> {
    // Libsignal serializes this public key as its 0x05 type byte and 32 key bytes.
    if !valid_void_id(owner)
        || device_id != 1
        || !(1..=16_380).contains(&registration_id)
        || signal_public_key.len() != 33
        || signal_public_key[0] != 0x05
    {
        return None;
    }

    let public_key = STANDARD.encode(signal_public_key);
    Some(format!(
        "VOID:signal-identity:v1\n{API_BASE}\n{owner}\n{device_id}\n{registration_id}\n{public_key}\n"
    ))
}

pub(super) fn sign_binding(
    account_key: &SigningKey,
    owner: &str,
    device_id: u32,
    registration_id: u32,
    signal_public_key: &[u8],
) -> Option<[u8; 64]> {
    let message = binding_message(owner, device_id, registration_id, signal_public_key)?;
    Some(account_key.sign(message.as_bytes()).to_bytes())
}

pub(super) fn verify_binding(
    account_key: &VerifyingKey,
    owner: &str,
    device_id: u32,
    registration_id: u32,
    signal_public_key: &[u8],
    signature: &[u8],
) -> bool {
    let Some(message) = binding_message(owner, device_id, registration_id, signal_public_key) else {
        return false;
    };
    Signature::from_slice(signature)
        .is_ok_and(|signature| account_key.verify_strict(message.as_bytes(), &signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANDROID_BINDING: &str = concat!(
        "VOID:signal-identity:v1\n",
        "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz\n",
        "123\n",
        "1\n",
        "16380\n",
        "BYUg8AmJMKdUdIt93LQ+91oNvzoNJjga9OukqY6qm05q\n",
    );

    fn signal_key() -> Vec<u8> {
        STANDARD
            .decode("BYUg8AmJMKdUdIt93LQ+91oNvzoNJjga9OukqY6qm05q")
            .unwrap()
    }

    #[test]
    fn signature_uses_exact_android_binding_bytes() {
        let account = SigningKey::from_bytes(&[7; 32]);
        let signal_key = signal_key();
        let signature = sign_binding(&account, "123", 1, 16_380, &signal_key).unwrap();
        assert_eq!(signature, account.sign(ANDROID_BINDING.as_bytes()).to_bytes());
        assert!(verify_binding(
            &account.verifying_key(), "123", 1, 16_380, &signal_key, &signature,
        ));

        let foreign_audience = ANDROID_BINDING.replace(API_BASE, "https://other.invalid");
        let signature = account.sign(foreign_audience.as_bytes()).to_bytes();
        assert!(!verify_binding(
            &account.verifying_key(), "123", 1, 16_380, &signal_key, &signature,
        ));
    }

    #[test]
    fn binding_rejects_other_accounts_keys_and_bad_identifiers() {
        let account = SigningKey::from_bytes(&[7; 32]);
        let signal_key = signal_key();
        let signature = sign_binding(&account, "123", 1, 24, &signal_key).unwrap();
        let mut different_signal_key = signal_key.clone();
        different_signal_key[2] ^= 1;
        let wrong_account = SigningKey::from_bytes(&[8; 32]).verifying_key();
        let account = account.verifying_key();

        assert!(!verify_binding(&wrong_account, "123", 1, 24, &signal_key, &signature));
        assert!(!verify_binding(&account, "124", 1, 24, &signal_key, &signature));
        assert!(!verify_binding(&account, "123", 1, 25, &signal_key, &signature));
        assert!(!verify_binding(&account, "123", 1, 24, &different_signal_key, &signature));
        assert!(!verify_binding(&account, "123", 1, 24, &signal_key, &signature[..63]));

        let signer = SigningKey::from_bytes(&[7; 32]);
        for (owner, device, registration, key) in [
            ("0123", 1, 24, signal_key.as_slice()),
            ("123", 2, 24, signal_key.as_slice()),
            ("123", 1, 0, signal_key.as_slice()),
            ("123", 1, 16_381, signal_key.as_slice()),
            ("123", 1, 24, &signal_key[1..]),
            ("123", 1, 24, &[0; 33]),
        ] {
            assert!(sign_binding(&signer, owner, device, registration, key).is_none());
        }
    }
}
