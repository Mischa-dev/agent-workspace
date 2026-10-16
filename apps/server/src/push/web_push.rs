//! Web Push on the wire: message encryption (RFC 8291, `aes128gcm` from
//! RFC 8188) and VAPID signing (RFC 8292), with aws-lc-rs. Nothing here
//! touches the database or logs; keys come in, bytes and headers go out.

use aws_lc_rs::{
    aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    agreement::{ECDH_P256, PrivateKey, UnparsedPublicKey, agree},
    hkdf::{HKDF_SHA256, KeyType, Salt},
    rand::{SecureRandom, SystemRandom},
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use thiserror::Error;
use zeroize::Zeroizing;

/// Push services accept at least 4096 bytes of body; the encryption adds
/// 103, so payloads stay under this.
pub const MAX_PAYLOAD_BYTES: usize = 3000;
const RECORD_SIZE: u32 = 4096;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum WebPushError {
    #[error("the device's push keys are malformed")]
    BadKeys,
    #[error("the payload is too large")]
    TooLarge,
    #[error("the server's push key is malformed")]
    BadServerKey,
    #[error("encryption failed")]
    Crypto,
}

/// A device's subscription, as the browser's `PushSubscription.toJSON()`
/// gives it.
pub struct Subscription<'a> {
    pub endpoint: &'a str,
    /// The device's P-256 public key, uncompressed (65 bytes).
    pub p256dh: &'a [u8],
    /// The device's 16-byte auth secret.
    pub auth: &'a [u8],
}

pub fn decode_key(value: &str) -> Option<Vec<u8>> {
    let trimmed = value.trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .ok()
        .or_else(|| base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed).ok())
}

pub fn valid_device_keys(p256dh: &[u8], auth: &[u8]) -> bool {
    p256dh.len() == 65
        && p256dh[0] == 4
        && auth.len() == 16
        && PrivateKey::generate(&ECDH_P256).is_ok_and(|probe| {
            agree(&probe, UnparsedPublicKey::new(&ECDH_P256, p256dh), (), |_| Ok(())).is_ok()
        })
}

struct Length(usize);

impl KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> Result<Zeroizing<Vec<u8>>, WebPushError> {
    let prk = Salt::new(HKDF_SHA256, salt).extract(ikm);
    let info = [info];
    let okm = prk
        .expand(&info, Length(length))
        .map_err(|_| WebPushError::Crypto)?;
    let mut out = Zeroizing::new(vec![0; length]);
    okm.fill(&mut out).map_err(|_| WebPushError::Crypto)?;
    Ok(out)
}

/// Encrypt `payload` for the device: the request body to POST.
pub fn encrypt(subscription: &Subscription<'_>, payload: &[u8]) -> Result<Vec<u8>, WebPushError> {
    let sender = PrivateKey::generate(&ECDH_P256).map_err(|_| WebPushError::Crypto)?;
    let mut salt = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| WebPushError::Crypto)?;
    encrypt_with(subscription, payload, &sender, salt)
}

fn encrypt_with(
    subscription: &Subscription<'_>,
    payload: &[u8],
    sender: &PrivateKey,
    salt: [u8; 16],
) -> Result<Vec<u8>, WebPushError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(WebPushError::TooLarge);
    }
    if !valid_device_keys(subscription.p256dh, subscription.auth) {
        return Err(WebPushError::BadKeys);
    }
    let sender_public = sender
        .compute_public_key()
        .map_err(|_| WebPushError::Crypto)?;
    let sender_public = sender_public.as_ref();
    let shared = agree(
        sender,
        UnparsedPublicKey::new(&ECDH_P256, subscription.p256dh),
        WebPushError::BadKeys,
        |secret| Ok(Zeroizing::new(secret.to_vec())),
    )?;
    // RFC 8291 section 3.4.
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(subscription.p256dh);
    key_info.extend_from_slice(sender_public);
    let ikm = hkdf(subscription.auth, &shared, &key_info, 32)?;
    let content_key = hkdf(&salt, &ikm, b"Content-Encoding: aes128gcm\0", 16)?;
    let nonce = hkdf(&salt, &ikm, b"Content-Encoding: nonce\0", 12)?;
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_128_GCM, &content_key).map_err(|_| WebPushError::Crypto)?,
    );
    let nonce: [u8; 12] = nonce.as_slice().try_into().map_err(|_| WebPushError::Crypto)?;
    // One record: the payload, then the last-record delimiter.
    let mut record = Vec::with_capacity(payload.len() + 17);
    record.extend_from_slice(payload);
    record.push(2);
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut record)
        .map_err(|_| WebPushError::Crypto)?;
    let mut body = Vec::with_capacity(16 + 4 + 1 + sender_public.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(u8::try_from(sender_public.len()).map_err(|_| WebPushError::Crypto)?);
    body.extend_from_slice(sender_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// A new VAPID signing key, as PKCS#8 DER.
pub fn generate_vapid_key() -> Result<Zeroizing<Vec<u8>>, WebPushError> {
    let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
        .map_err(|_| WebPushError::Crypto)?;
    let document = pair.to_pkcs8v1().map_err(|_| WebPushError::Crypto)?;
    Ok(Zeroizing::new(document.as_ref().to_vec()))
}

/// The application server key browsers subscribe with: the VAPID public
/// key, uncompressed and base64url.
pub fn vapid_public_key(pkcs8: &[u8]) -> Result<String, WebPushError> {
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)
        .map_err(|_| WebPushError::BadServerKey)?;
    Ok(URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()))
}

/// The `Authorization` header for a push to `endpoint`, valid for 12 hours.
pub fn vapid_authorization(
    pkcs8: &[u8],
    endpoint: &url::Url,
    contact: &str,
    now_seconds: i64,
) -> Result<String, WebPushError> {
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)
        .map_err(|_| WebPushError::BadServerKey)?;
    let audience = endpoint.origin().ascii_serialization();
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        json!({"aud": audience, "exp": now_seconds + 12 * 60 * 60, "sub": contact}).to_string(),
    );
    let signing_input = format!("{header}.{claims}");
    let signature = pair
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|_| WebPushError::Crypto)?;
    Ok(format!(
        "vapid t={signing_input}.{}, k={}",
        URL_SAFE_NO_PAD.encode(signature.as_ref()),
        URL_SAFE_NO_PAD.encode(pair.public_key().as_ref())
    ))
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::{
        agreement::{ECDH_P256, PrivateKey},
        signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey},
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    use super::{
        Subscription, WebPushError, decode_key, encrypt, encrypt_with, generate_vapid_key,
        vapid_authorization, vapid_public_key,
    };

    fn b64(value: &str) -> Vec<u8> {
        decode_key(value).expect("base64url")
    }

    /// RFC 8291 appendix A.
    #[test]
    fn encryption_matches_the_rfc_example() {
        let p256dh = b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let sender = PrivateKey::from_private_key(
            &ECDH_P256,
            &b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"),
        )
        .expect("sender key");
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().expect("salt");
        let body = encrypt_with(
            &Subscription {
                endpoint: "https://push.example.net/push/JzLQ3raZJfFBR0aqvOMsLrt54w4rJUsV",
                p256dh: &p256dh,
                auth: &auth,
            },
            b"When I grow up, I want to be a watermelon",
            &sender,
            salt,
        )
        .expect("encrypted");
        assert_eq!(
            URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn bad_keys_and_big_payloads_are_refused() {
        let auth = [7_u8; 16];
        let short = [4_u8; 10];
        assert_eq!(
            encrypt(
                &Subscription {
                    endpoint: "https://push.example.net/x",
                    p256dh: &short,
                    auth: &auth,
                },
                b"hi",
            ),
            Err(WebPushError::BadKeys)
        );
        let device = PrivateKey::generate(&ECDH_P256).expect("device key");
        let public = device.compute_public_key().expect("public key");
        assert_eq!(
            encrypt(
                &Subscription {
                    endpoint: "https://push.example.net/x",
                    p256dh: public.as_ref(),
                    auth: &auth,
                },
                &vec![b'a'; 5000],
            ),
            Err(WebPushError::TooLarge)
        );
    }

    #[test]
    fn vapid_tokens_verify_with_the_public_key() {
        let key = generate_vapid_key().expect("vapid key");
        let public = b64(&vapid_public_key(&key).expect("public key"));
        let endpoint = url::Url::parse("https://fcm.googleapis.com/fcm/send/abc").expect("url");
        let header =
            vapid_authorization(&key, &endpoint, "mailto:ops@example.com", 1_800_000_000)
                .expect("authorization");
        let token = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split(", k=").next())
            .expect("token");
        let (signing_input, signature) = token.rsplit_once('.').expect("signature");
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
            .verify(signing_input.as_bytes(), &b64(signature))
            .expect("valid signature");
        let claims = b64(signing_input.split('.').nth(1).expect("claims"));
        let claims: serde_json::Value = serde_json::from_slice(&claims).expect("json");
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "mailto:ops@example.com");
    }
}
