//! Checking a pushed webhook, as a receiver should.
//!
//! Every push is a Standard Webhooks push: `webhook-signature` holds one or
//! more space-separated `v1,<base64>` signatures, each an HMAC-SHA256 over
//! `"{webhook-id}.{webhook-timestamp}.{body}"` keyed by the base64-decoded
//! part of the secret after `whsec_`. More than one while a rotated secret
//! is retiring; any one matching is enough.
//!
//! Receivers should also refuse a timestamp more than five minutes from
//! their clock, and treat `webhook-id` as the key that makes a redelivery
//! harmless.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::{Report, check_envelope, fixtures};

/// How far a push's timestamp may be from now.
pub const TOLERANCE_SECONDS: u64 = 300;

/// The key a `whsec_` secret signs with.
fn standard_key(secret: &str) -> Option<Vec<u8>> {
    let encoded = secret.strip_prefix("whsec_").unwrap_or(secret);
    [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(encoded).ok())
        .filter(|bytes| !bytes.is_empty())
}

fn mac(key: &[u8], parts: &[&str]) -> Option<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            mac.update(b".");
        }
        mac.update(part.as_bytes());
    }
    Some(mac.finalize().into_bytes().to_vec())
}

/// Whether any signature in a `webhook-signature` header is the secret's
/// over this id, timestamp and body.
pub fn verify_standard(
    secret: &str,
    webhook_id: &str,
    timestamp: &str,
    signatures: &str,
    body: &str,
) -> bool {
    let Some(key) = standard_key(secret) else {
        return false;
    };
    let Some(expected) = mac(&key, &[webhook_id, timestamp, body]) else {
        return false;
    };
    signatures.split(' ').any(|signature| {
        signature
            .strip_prefix("v1,")
            .and_then(|encoded| STANDARD.decode(encoded).ok())
            .is_some_and(|given| given == expected)
    })
}

/// One received push: its headers (names in any case) and its raw body.
#[derive(Clone, Debug)]
pub struct Received {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Checks one push, signed with `secret`, as a receiver would. `now` is
/// the receiver's clock in seconds (`None`: this machine's), so a
/// fixture's fixed timestamp can be checked too.
pub fn check_webhook(secret: &str, push: &Received, now: Option<u64>) -> Report {
    let mut report = Report::default();
    let fixture = fixtures::parse(fixtures::WEBHOOK_STANDARD);
    let missing = fixture["headers"]
        .as_object()
        .map(|headers| {
            headers
                .keys()
                .filter(|name| push.header(name).is_none())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    report.record(
        "sends every header the fixture shows",
        missing.is_empty(),
        missing.join(", "),
    );
    let header = |name: &str| push.header(name).unwrap_or_default();
    report.record(
        "signs the Standard Webhooks way",
        verify_standard(
            secret,
            header("webhook-id"),
            header("webhook-timestamp"),
            header("webhook-signature"),
            &push.body,
        ),
        header("webhook-signature"),
    );
    let now = now.unwrap_or_else(now_seconds);
    let stamped = header("webhook-timestamp").parse::<u64>().ok();
    report.record(
        "stamps the time it was sent",
        stamped.is_some_and(|at| at.abs_diff(now) <= TOLERANCE_SECONDS),
        header("webhook-timestamp"),
    );
    let body = serde_json::from_str::<Value>(&push.body).unwrap_or(Value::Null);
    report.result(
        "sends a body shaped like the fixture",
        check_envelope(&body),
    );
    report.record(
        "sends the event's id as webhook-id",
        body["id"]
            .as_str()
            .is_some_and(|id| id == header("webhook-id")),
        header("webhook-id"),
    );
    report
}

/// The fixture's push, as [`Received`], with its secret.
pub fn fixture_push() -> (String, Received) {
    let fixture = fixtures::parse(fixtures::WEBHOOK_STANDARD);
    let headers = fixture["headers"]
        .as_object()
        .map(|headers| {
            headers
                .iter()
                .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    (
        fixture["secret"].as_str().unwrap_or_default().to_owned(),
        Received {
            headers,
            body: fixture["body"].as_str().unwrap_or_default().to_owned(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published Standard Webhooks test vector.
    #[test]
    fn verifies_the_standard_webhooks_test_vector() {
        assert!(verify_standard(
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            "1614265330",
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
            r#"{"test": 2432232314}"#,
        ));
    }

    #[test]
    fn the_webhook_fixture_passes_its_own_check() {
        let (secret, push) = fixture_push();
        let stamped = push
            .header("webhook-timestamp")
            .and_then(|at| at.parse().ok());
        let report = check_webhook(&secret, &push, stamped);
        assert!(report.passed(), "{report}");
    }

    #[test]
    fn a_tampered_body_or_wrong_secret_fails() {
        let (secret, mut push) = fixture_push();
        let stamped = push
            .header("webhook-timestamp")
            .and_then(|at| at.parse().ok());
        let wrong = check_webhook("whsec_c2VjcmV0", &push, stamped);
        assert_eq!(wrong.failures(), vec!["signs the Standard Webhooks way"]);
        push.body = push.body.replace("mentioned", "assigned");
        let tampered = check_webhook(&secret, &push, stamped);
        assert!(!tampered.passed(), "{tampered}");
    }
}
