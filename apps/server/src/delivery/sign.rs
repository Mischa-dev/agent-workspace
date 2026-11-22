//! Webhook signatures (A2 3.5). Standard Webhooks: `v1,<base64>` of an
//! HMAC-SHA256 over `"{webhook-id}.{webhook-timestamp}.{body}"`, keyed by
//! the secret's bytes after `whsec_`.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// A new random signing secret, shown to its owner once.
pub fn new_secret() -> String {
    use rand_core::{OsRng, RngCore};
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("whsec_{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// The key bytes of a `whsec_` secret: base64 in either alphabet, padded
/// or not. A secret that isn't base64 signs with its own bytes.
fn key_bytes(secret: &str) -> Vec<u8> {
    let encoded = secret.strip_prefix("whsec_").unwrap_or(secret);
    [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(encoded).ok())
        .filter(|bytes| !bytes.is_empty())
        .unwrap_or_else(|| encoded.as_bytes().to_vec())
}

/// One `v1,<signature>` for the Standard Webhooks header.
pub fn standard(secret: &str, message_id: &str, timestamp: &str, body: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes(secret)).ok()?;
    mac.update(message_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    Some(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

/// Seconds since the epoch, as the headers carry them.
pub fn unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published Standard Webhooks test vector.
    #[test]
    fn signs_the_standard_webhooks_test_vector() {
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let id = "msg_p5jXN8AQM9LWM0D4loKWxJek";
        let timestamp = "1614265330";
        let body = r#"{"test": 2432232314}"#;
        assert_eq!(
            standard(secret, id, timestamp, body).as_deref(),
            Some("v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=")
        );
    }

    #[test]
    fn new_secrets_decode_to_32_bytes() {
        let secret = new_secret();
        assert!(secret.starts_with("whsec_"));
        assert_eq!(key_bytes(&secret).len(), 32);
    }
}
