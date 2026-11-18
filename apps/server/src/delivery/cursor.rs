//! The delivery cursor (A2 3.5): `c2.<position>.<mac>`, a position in the
//! event log signed with the connection's own key. It is bound to the
//! connection, not to the credential, so a rotated credential keeps its
//! place; another connection's cursor is refused.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

const PREFIX: &str = "c2.";
/// A cursor that also remembers where a paused account's held events start
/// and which pauses held them (A2.10): `c3.<position>.<held>.<paused>.<mac>`.
const HELD_PREFIX: &str = "c3.";
const MAC_BYTES: usize = 16;

fn mac(key: &[u8], connection_id: Uuid, position: i64, held: Option<Held>) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(connection_id.as_bytes());
    mac.update(&position.to_be_bytes());
    if let Some(held) = held {
        mac.update(&held.at.to_be_bytes());
        mac.update(&held.paused.to_be_bytes());
    }
    Some(mac)
}

fn signature(key: &[u8], connection_id: Uuid, position: i64, held: Option<Held>) -> String {
    mac(key, connection_id, position, held)
        .map(|mac| mac.finalize().into_bytes())
        .map(|bytes| URL_SAFE_NO_PAD.encode(&bytes[..MAC_BYTES]))
        .unwrap_or_default()
}

/// The cursor for "after `position`" on this connection.
pub fn encode(key: &[u8], connection_id: Uuid, position: i64) -> String {
    format!(
        "{PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(position.to_be_bytes()),
        signature(key, connection_id, position, None)
    )
}

/// Where a read goes on from: after `position`, and, while the account is
/// paused, where its held events start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Place {
    pub after: i64,
    pub held: Option<Held>,
}

/// Held events: they start after `at`, held by the pauses whose
/// fingerprint is `paused`. Once those pauses change (any one resumed, or
/// another added), a read goes back to `at`, so what a resume freed comes
/// even while the account stays paused elsewhere.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Held {
    pub at: i64,
    pub paused: u64,
}

/// A fingerprint of where an account is paused (`None` for everywhere),
/// in any order.
pub fn paused_fingerprint(paused: &[Option<Uuid>]) -> u64 {
    use sha2::Digest;
    let mut places = paused.to_vec();
    places.sort_unstable();
    places.dedup();
    let mut hash = Sha256::new();
    for place in places {
        match place {
            None => hash.update([0_u8]),
            Some(workspace) => {
                hash.update([1_u8]);
                hash.update(workspace.as_bytes());
            }
        }
    }
    let digest = hash.finalize();
    u64::from_be_bytes(digest[..8].try_into().unwrap_or_default())
}

/// The cursor for `place`: a plain one when nothing is held.
pub fn encode_place(key: &[u8], connection_id: Uuid, place: Place) -> String {
    match place.held {
        None => encode(key, connection_id, place.after),
        Some(held) => format!(
            "{HELD_PREFIX}{}.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(place.after.to_be_bytes()),
            URL_SAFE_NO_PAD.encode(held.at.to_be_bytes()),
            URL_SAFE_NO_PAD.encode(held.paused.to_be_bytes()),
            signature(key, connection_id, place.after, Some(held))
        ),
    }
}

fn position(part: &str) -> Option<i64> {
    let position = i64::from_be_bytes(URL_SAFE_NO_PAD.decode(part).ok()?.try_into().ok()?);
    (position >= 0).then_some(position)
}

/// The place a cursor names, if it is this connection's.
pub fn decode_place(key: &[u8], connection_id: Uuid, cursor: &str) -> Option<Place> {
    if cursor.len() > 96 {
        return None;
    }
    let (after, held, signature) = if let Some(rest) = cursor.strip_prefix(HELD_PREFIX) {
        let mut parts = rest.splitn(4, '.');
        let (after, at, paused, signature) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let paused = u64::from_be_bytes(URL_SAFE_NO_PAD.decode(paused).ok()?.try_into().ok()?);
        let held = Held {
            at: position(at)?,
            paused,
        };
        (position(after)?, Some(held), signature)
    } else {
        let (after, signature) = cursor.strip_prefix(PREFIX)?.split_once('.')?;
        (position(after)?, None, signature)
    };
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    if signature.len() != MAC_BYTES {
        return None;
    }
    mac(key, connection_id, after, held)?
        .verify_truncated_left(&signature)
        .ok()?;
    Some(Place { after, held })
}

/// The position a plain cursor names, if it is this connection's.
pub fn decode(key: &[u8], connection_id: Uuid, cursor: &str) -> Option<i64> {
    decode_place(key, connection_id, cursor).map(|place| place.after)
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

    #[test]
    fn a_held_place_is_signed_with_its_cursor() {
        let key = [3_u8; 32];
        let connection = Uuid::now_v7();
        let paused = paused_fingerprint(&[Some(connection), None]);
        assert_eq!(paused, paused_fingerprint(&[None, Some(connection)]));
        assert_ne!(paused, paused_fingerprint(&[None]));
        let place = Place {
            after: 90,
            held: Some(Held { at: 40, paused }),
        };
        let cursor = encode_place(&key, connection, place);
        assert!(cursor.starts_with("c3."));
        assert_eq!(decode_place(&key, connection, &cursor), Some(place));
        let moved = cursor.replace(
            &URL_SAFE_NO_PAD.encode(40_i64.to_be_bytes()),
            &URL_SAFE_NO_PAD.encode(80_i64.to_be_bytes()),
        );
        assert_eq!(decode_place(&key, connection, &moved), None);
        let repaused = cursor.replace(
            &URL_SAFE_NO_PAD.encode(paused.to_be_bytes()),
            &URL_SAFE_NO_PAD.encode(paused_fingerprint(&[None]).to_be_bytes()),
        );
        assert_eq!(decode_place(&key, connection, &repaused), None);
        let plain = encode_place(
            &key,
            connection,
            Place {
                after: 7,
                held: None,
            },
        );
        assert_eq!(plain, encode(&key, connection, 7));
    }
}
