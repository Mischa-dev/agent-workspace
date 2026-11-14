//! Typed references in inputs: they accept what people paste (an id, or a
//! link to the thing) and hold the id.

use std::borrow::Cow;

use rmcp::schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// The longest reference accepted (a full link).
const MAX_REFERENCE: usize = 2048;

/// An id, or a link whose last path segment is the id.
pub fn parse_id(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value.len() > MAX_REFERENCE {
        return None;
    }
    value.parse::<Uuid>().ok().or_else(|| {
        value
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|segment| segment.split(['?', '#']).next())
            .and_then(|segment| segment.parse::<Uuid>().ok())
    })
}

/// A message's id, or a link to it: the app links a message as its
/// conversation's address plus `#message-<id>`.
pub fn parse_message_id(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value.len() > MAX_REFERENCE {
        return None;
    }
    match value.rsplit_once("#message-") {
        Some((_, anchor)) => anchor
            .split(|character: char| !(character.is_ascii_hexdigit() || character == '-'))
            .next()
            .and_then(|id| id.parse::<Uuid>().ok()),
        None => parse_id(value),
    }
}

macro_rules! reference {
    ($name:ident, $parse:path, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn id(self) -> Uuid {
                self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                $parse(&value).map(Self).ok_or_else(|| {
                    serde::de::Error::custom(concat!("expected ", $description))
                })
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "description": $description,
                    "maxLength": MAX_REFERENCE,
                })
            }

            fn inline_schema() -> bool {
                true
            }
        }
    };
}

reference!(IdRef, parse_id, "an id, or a link to it");
reference!(
    MessageRef,
    parse_message_id,
    "a message's id, or a link to it"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_accept_ids_and_links() {
        let id = Uuid::now_v7();
        for text in [
            id.to_string(),
            format!("https://example.org/projects/x/pages/{id}"),
            format!("/projects/x/pages/{id}?tab=history"),
        ] {
            let parsed: IdRef = serde_json::from_value(text.clone().into()).expect("id");
            assert_eq!(parsed.id(), id, "{text}");
        }
        let conversation = Uuid::now_v7();
        let message: MessageRef =
            serde_json::from_value(format!("/messages/{conversation}#message-{id}").into())
                .expect("message");
        assert_eq!(message.id(), id);
        assert!(serde_json::from_value::<IdRef>("not an id".into()).is_err());
    }
}
