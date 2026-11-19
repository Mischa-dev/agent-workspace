//! Live tickets (K5.19): what a connection may hear, signed by the
//! database side so the live side can trust it without a database.
//!
//! A ticket is `<payload>.<signature>`, both base64url without padding: the
//! payload is JSON (so another live service reads it easily), the signature
//! Ed25519 under a key derived from the secrets keyring (`live-ticket`).
//! The live side needs only the public key, which `GET /api/v2/live/key`
//! serves. Publishes to an outside live service are signed the same way.

use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    error::AppError,
    realtime::documents::{DocumentKind, DocumentRef},
    secrets,
};

/// How long a ticket works; the client refreshes it through `api` first.
pub const TICKET_LIFETIME_SECONDS: i64 = 5 * 60;
const PURPOSE: &str = "live-ticket";
const VERSION: u8 = 1;
/// The largest ticket a live service need accept.
pub const MAX_TICKET_BYTES: usize = 48 * 1024;

/// Who hears a frame: a ticket names the audiences its connection is in,
/// a published frame the audiences it is for. Written `<kind>:<id>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Audience {
    /// Everyone in a workspace.
    Workspace(Uuid),
    /// A space's readers.
    Space(Uuid),
    /// A restricted conversation's members (a DM, a group, a private
    /// channel), by its root.
    Conversation(Uuid),
    /// One account.
    Account(Uuid),
    /// A document's room (K5.21): connections watching it, whose ticket
    /// was issued after checking they may see it.
    Document(DocumentRef),
}

fn kind_name(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Page => "page",
        DocumentKind::Whiteboard => "whiteboard",
        DocumentKind::Sheet => "sheet",
    }
}

/// A document as an audience and a query write it: `<kind>.<id>`.
pub fn parse_document(text: &str) -> Option<DocumentRef> {
    let (kind, id) = text.split_once('.')?;
    let kind = match kind {
        "page" => DocumentKind::Page,
        "whiteboard" => DocumentKind::Whiteboard,
        "sheet" => DocumentKind::Sheet,
        _ => return None,
    };
    Some(DocumentRef {
        kind,
        id: Uuid::parse_str(id).ok()?,
    })
}

/// `<kind>.<id>`.
pub fn document_name(document: DocumentRef) -> String {
    format!("{}.{}", kind_name(document.kind), document.id)
}

impl Audience {
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, id) = text.split_once(':')?;
        if kind == "document" {
            return parse_document(id).map(Self::Document);
        }
        let id = Uuid::parse_str(id).ok()?;
        Some(match kind {
            "workspace" => Self::Workspace(id),
            "space" => Self::Space(id),
            "conversation" => Self::Conversation(id),
            "account" => Self::Account(id),
            _ => return None,
        })
    }
}

impl std::fmt::Display for Audience {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace(id) => write!(formatter, "workspace:{id}"),
            Self::Space(id) => write!(formatter, "space:{id}"),
            Self::Conversation(id) => write!(formatter, "conversation:{id}"),
            Self::Account(id) => write!(formatter, "account:{id}"),
            Self::Document(document) => {
                write!(formatter, "document:{}", document_name(*document))
            }
        }
    }
}

impl Serialize for Audience {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Audience {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("unknown audience"))
    }
}

/// What kind of connection a ticket is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Web,
    AgentStream,
    Host,
}

/// A ticket's claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub v: u8,
    /// The signing key's id, so keys can rotate.
    pub kid: String,
    pub account: Uuid,
    /// The session (or credential) it was issued to; a revoked session's
    /// connections close.
    pub session: Uuid,
    pub kind: ConnectionKind,
    pub audiences: BTreeSet<Audience>,
    /// Deliver durable events after this position (a string, as positions
    /// can pass JavaScript's safe integers).
    #[serde(with = "position")]
    pub from: i64,
    /// Unix seconds.
    pub expires: i64,
    /// The log's head when it was cut (`from` can be earlier, to resume).
    /// What an account may hear is decided at that point: a connection
    /// takes no ticket cut before a change to it (a `ticket.stale` at a
    /// later position).
    #[serde(default, with = "position")]
    pub head: i64,
}

mod position {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The signing key, on the database side.
pub struct Signer {
    key_id: String,
    pair: Ed25519KeyPair,
}

impl Signer {
    /// The current key, from the keyring.
    pub fn from_config(config: &AppConfig) -> Result<Self, AppError> {
        let keys = secrets::keyring(config)?;
        let (key_id, seed) = keys.derived(None, PURPOSE)?;
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed[..])
            .map_err(|_| AppError::Internal("derive the live key"))?;
        Ok(Self { key_id, pair })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key(&self) -> [u8; 32] {
        let mut key = [0_u8; 32];
        key.copy_from_slice(self.pair.public_key().as_ref());
        key
    }

    /// `ticket`, signed under this key (its `kid` is set to it).
    pub fn sign(&self, ticket: &Ticket) -> String {
        let mut ticket = ticket.clone();
        ticket.kid.clone_from(&self.key_id);
        ticket.v = VERSION;
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ticket).unwrap_or_default());
        let signature = URL_SAFE_NO_PAD.encode(self.pair.sign(payload.as_bytes()).as_ref());
        format!("{payload}.{signature}")
    }

    /// A publish body's signature: `<kid>.<signature>`.
    pub fn sign_body(&self, body: &[u8]) -> String {
        format!(
            "{}.{}",
            self.key_id,
            URL_SAFE_NO_PAD.encode(self.pair.sign(body).as_ref())
        )
    }
}

/// The public keys the live side checks against, by key id.
pub trait PublicKeys {
    fn public_key(&self, key_id: &str) -> Option<[u8; 32]>;
}

/// The public keys from this server's own keyring: what the in-process
/// live role and a `live`-only instance of this server use.
pub struct KeyringKeys<'a>(pub &'a AppConfig);

impl PublicKeys for KeyringKeys<'_> {
    fn public_key(&self, key_id: &str) -> Option<[u8; 32]> {
        let keys = secrets::keyring(self.0).ok()?;
        let (_, seed) = keys.derived(Some(key_id), PURPOSE).ok()?;
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed[..]).ok()?;
        let mut key = [0_u8; 32];
        key.copy_from_slice(pair.public_key().as_ref());
        Some(key)
    }
}

/// A ticket's claims, if it is well formed, signed by a known key and not
/// expired at `now` (Unix seconds).
pub fn verify(token: &str, keys: &(dyn PublicKeys + Sync), now: i64) -> Option<Ticket> {
    if token.len() > MAX_TICKET_BYTES {
        return None;
    }
    let (payload, signature) = token.split_once('.')?;
    let claims = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let ticket = serde_json::from_slice::<Ticket>(&claims).ok()?;
    if ticket.v != VERSION {
        return None;
    }
    let key = keys.public_key(&ticket.kid)?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(payload.as_bytes(), &signature)
        .ok()?;
    (ticket.expires > now).then_some(ticket)
}

/// Whether `signature` (`<kid>.<signature>`) signs `body`.
pub fn verify_body(signature: &str, body: &[u8], keys: &(dyn PublicKeys + Sync)) -> bool {
    let Some((key_id, signature)) = signature.split_once('.') else {
        return false;
    };
    let Some(key) = keys.public_key(key_id) else {
        return false;
    };
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, key)
        .verify(body, &signature)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Ed25519KeyPair);

    impl PublicKeys for Fixed {
        fn public_key(&self, key_id: &str) -> Option<[u8; 32]> {
            (key_id == "k1").then(|| {
                let mut key = [0_u8; 32];
                key.copy_from_slice(self.0.public_key().as_ref());
                key
            })
        }
    }

    #[test]
    fn a_ticket_verifies_only_as_signed_and_in_time() {
        let seed = [3_u8; 32];
        let signer = Signer {
            key_id: "k1".to_owned(),
            pair: Ed25519KeyPair::from_seed_unchecked(&seed).expect("pair"),
        };
        let keys = Fixed(Ed25519KeyPair::from_seed_unchecked(&seed).expect("pair"));
        let ticket = Ticket {
            v: VERSION,
            kid: String::new(),
            account: Uuid::now_v7(),
            session: Uuid::now_v7(),
            kind: ConnectionKind::Web,
            audiences: BTreeSet::from([
                Audience::Workspace(Uuid::now_v7()),
                Audience::Conversation(Uuid::now_v7()),
            ]),
            from: 9_007_199_254_740_993,
            expires: 1_000,
            head: 0,
        };
        let token = signer.sign(&ticket);
        let opened = verify(&token, &keys, 999).expect("opens");
        assert_eq!(opened.audiences, ticket.audiences);
        assert_eq!(opened.from, ticket.from, "positions survive as strings");
        assert_eq!(opened.kid, "k1");
        assert!(verify(&token, &keys, 1_000).is_none(), "expired");
        // Any change to the claims breaks it.
        let (payload, signature) = token.split_once('.').expect("parts");
        let mut claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).expect("payload"))
                .expect("json");
        claims["audiences"]
            .as_array_mut()
            .expect("audiences")
            .push(serde_json::json!(format!("workspace:{}", Uuid::now_v7())));
        let forged = format!("{}.{signature}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        assert!(verify(&forged, &keys, 999).is_none(), "forged audiences");
        // Signed by another key.
        let other = Signer {
            key_id: "k1".to_owned(),
            pair: Ed25519KeyPair::from_seed_unchecked(&[4_u8; 32]).expect("pair"),
        };
        assert!(verify(&other.sign(&ticket), &keys, 999).is_none());
        assert!(verify("nonsense", &keys, 0).is_none());
        // Bodies.
        let body = br#"{"frames":[]}"#;
        assert!(verify_body(&signer.sign_body(body), body, &keys));
        assert!(!verify_body(&signer.sign_body(body), b"{}", &keys));
        assert!(!verify_body(&other.sign_body(body), body, &keys));
    }

    #[test]
    fn audiences_read_as_they_are_written() {
        let id = Uuid::now_v7();
        for audience in [
            Audience::Workspace(id),
            Audience::Space(id),
            Audience::Conversation(id),
            Audience::Account(id),
            Audience::Document(DocumentRef {
                kind: DocumentKind::Sheet,
                id,
            }),
        ] {
            assert_eq!(Audience::parse(&audience.to_string()), Some(audience));
        }
        assert_eq!(Audience::parse("board:x"), None);
        assert_eq!(
            Audience::parse(&format!("document:page.{id}")).map(|audience| audience.to_string()),
            Some(format!("document:page.{id}"))
        );
        assert_eq!(Audience::parse(&format!("document:note.{id}")), None);
    }
}
