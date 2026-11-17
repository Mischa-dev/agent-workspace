//! The delivery cursor (A2 3.5): `c2.<position>.<mac>`, a position in the
//! event log signed with the connection's own key. It is bound to the
//! connection, not to the credential, so a rotated credential keeps its
//! place; another connection's cursor is refused.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

const PREFIX: &str = "c2.";
const MAC_BYTES: usize = 16;

fn mac(key: &[u8], connection_id: Uuid, position: i64) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(connection_id.as_bytes());
    mac.update(&position.to_be_bytes());
    Some(mac)
}

/// The cursor for "after `position`" on this connection.
pub fn encode(key: &[u8], connection_id: Uuid, position: i64) -> String {
    let signature = mac(key, connection_id, position)
        .map(|mac| mac.finalize().into_bytes())
        .map(|bytes| URL_SAFE_NO_PAD.encode(&bytes[..MAC_BYTES]))
        .unwrap_or_default();
    format!(
        "{PREFIX}{}.{signature}",
        URL_SAFE_NO_PAD.encode(position.to_be_bytes())
    )
}

/// The position a cursor names, if it is this connection's.
pub fn decode(key: &[u8], connection_id: Uuid, cursor: &str) -> Option<i64> {
    if cursor.len() > 64 {
        return None;
    }
    let (position, signature) = cursor.strip_prefix(PREFIX)?.split_once('.')?;
    let position = i64::from_be_bytes(URL_SAFE_NO_PAD.decode(position).ok()?.try_into().ok()?);
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    if position < 0 || signature.len() != MAC_BYTES {
        return None;
    }
    mac(key, connection_id, position)?
        .verify_truncated_left(&signature)
        .ok()?;
    Some(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_is_signed_for_its_connection() {
        let key = [3_u8; 32];
        let connection = Uuid::now_v7();
        let cursor = encode(&key, connection, 4_812);
        assert!(cursor.starts_with("c2."));
        assert_eq!(decode(&key, connection, &cursor), Some(4_812));
        assert_eq!(decode(&key, Uuid::now_v7(), &cursor), None);
        assert_eq!(decode(&[4_u8; 32], connection, &cursor), None);
        let forged = encode(&key, connection, 9).replace(
            &URL_SAFE_NO_PAD.encode(9_i64.to_be_bytes()),
            &URL_SAFE_NO_PAD.encode(10_i64.to_be_bytes()),
        );
        assert_eq!(decode(&key, connection, &forged), None);
        assert_eq!(decode(&key, connection, "c1.whatever"), None);
        assert_eq!(decode(&key, connection, ""), None);
    }
}
