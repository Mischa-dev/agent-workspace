use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MESSAGE_CONTENT_VERSION: u16 = 1;
pub const MAX_MESSAGE_NODES: usize = 128;
pub const MAX_MESSAGE_TEXT_CHARACTERS: usize = 20_000;
pub const MAX_MESSAGE_MENTIONS: usize = 32;

/// Incrementally validates provider-authored plain text against the same
/// boundary used for durable Message content. A fragment is checked before it
/// is appended, so rejected output is never exposed through realtime or later
/// persisted by a caller using this buffer.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlainTextMessageBuffer {
    text: String,
    scalar_values: usize,
}

impl PlainTextMessageBuffer {
    pub fn push(&mut self, fragment: &str) -> Result<(), MessageContentError> {
        if fragment.chars().any(unsupported_control_character) {
            return Err(MessageContentError::UnsupportedControlCharacter);
        }
        let scalar_values = self
            .scalar_values
            .checked_add(fragment.chars().count())
            .ok_or(MessageContentError::TextTooLong)?;
        if scalar_values > MAX_MESSAGE_TEXT_CHARACTERS {
            return Err(MessageContentError::TextTooLong);
        }

        self.text.push_str(fragment);
        self.scalar_values = scalar_values;
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn scalar_values(&self) -> usize {
        self.scalar_values
    }

    pub fn into_validated(self) -> Result<ValidatedMessageContent, MessageContentError> {
        MessageContent::from_plain_text(self.text)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageContent {
    pub version: u16,
    pub nodes: Vec<MessageNode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessageNode {
    Text {
        text: String,
    },
    Mention {
        #[serde(rename = "actorId")]
        actor_id: Uuid,
    },
}

impl MessageNode {
    pub fn actor_id(&self) -> Option<Uuid> {
        match self {
            Self::Mention { actor_id } => Some(*actor_id),
            Self::Text { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMessageContent {
    content: MessageContent,
    plain_text: String,
    mentioned_actor_ids: Vec<Uuid>,
}

impl ValidatedMessageContent {
    pub fn content(&self) -> &MessageContent {
        &self.content
    }

    pub fn plain_text(&self) -> &str {
        &self.plain_text
    }

    pub fn mentioned_actor_ids(&self) -> &[Uuid] {
        &self.mentioned_actor_ids
    }

    pub fn into_parts(self) -> (MessageContent, String, Vec<Uuid>) {
        (self.content, self.plain_text, self.mentioned_actor_ids)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MessageContentError {
    #[error("message content is malformed")]
    Malformed,
    #[error("message content version is unsupported")]
    UnsupportedVersion,
    #[error("message content must contain at least one node")]
    Empty,
    #[error("message content has too many nodes")]
    TooManyNodes,
    #[error("message content has too many mentions")]
    TooManyMentions,
    #[error("message text is too long")]
    TextTooLong,
    #[error("message text contains an unsupported control character")]
    UnsupportedControlCharacter,
    #[error("a mentioned Actor is unavailable")]
    MentionUnavailable,
    #[error("a mentioned Actor has an invalid handle")]
    InvalidMentionHandle,
}

impl MessageContent {
    pub fn from_plain_text(
        text: impl Into<String>,
    ) -> Result<ValidatedMessageContent, MessageContentError> {
        Self {
            version: MESSAGE_CONTENT_VERSION,
            nodes: vec![MessageNode::Text { text: text.into() }],
        }
        .validate_with_mentions(|_| None)
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, MessageContentError> {
        let content: Self =
            serde_json::from_value(value).map_err(|_| MessageContentError::Malformed)?;
        content.validate_structure()?;
        Ok(content)
    }

    pub fn validate_structure(&self) -> Result<(), MessageContentError> {
        if self.version != MESSAGE_CONTENT_VERSION {
            return Err(MessageContentError::UnsupportedVersion);
        }
        if self.nodes.is_empty() {
            return Err(MessageContentError::Empty);
        }
        if self.nodes.len() > MAX_MESSAGE_NODES {
            return Err(MessageContentError::TooManyNodes);
        }

        let mut text_characters = 0usize;
        let mut mentions = 0usize;
        for node in &self.nodes {
            match node {
                MessageNode::Text { text } => {
                    if text.chars().any(unsupported_control_character) {
                        return Err(MessageContentError::UnsupportedControlCharacter);
                    }
                    text_characters = text_characters
                        .checked_add(text.chars().count())
                        .ok_or(MessageContentError::TextTooLong)?;
                    if text_characters > MAX_MESSAGE_TEXT_CHARACTERS {
                        return Err(MessageContentError::TextTooLong);
                    }
                }
                MessageNode::Mention { actor_id } => {
                    if actor_id.is_nil() {
                        return Err(MessageContentError::Malformed);
                    }
                    mentions += 1;
                    if mentions > MAX_MESSAGE_MENTIONS {
                        return Err(MessageContentError::TooManyMentions);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn mentioned_actor_ids(&self) -> Vec<Uuid> {
        let mut seen = HashSet::new();
        self.nodes
            .iter()
            .filter_map(MessageNode::actor_id)
            .filter(|actor_id| seen.insert(*actor_id))
            .collect()
    }

    pub fn validate_with_mentions<F>(
        self,
        mut resolve_handle: F,
    ) -> Result<ValidatedMessageContent, MessageContentError>
    where
        F: FnMut(Uuid) -> Option<String>,
    {
        self.validate_structure()?;

        let mut plain_text = String::new();
        let mut mentioned_actor_ids = Vec::new();
        let mut seen = HashSet::new();
        let mut plain_text_characters = 0usize;
        for node in &self.nodes {
            match node {
                MessageNode::Text { text } => {
                    plain_text.push_str(text);
                    plain_text_characters = plain_text_characters
                        .checked_add(text.chars().count())
                        .ok_or(MessageContentError::TextTooLong)?;
                }
                MessageNode::Mention { actor_id } => {
                    let handle =
                        resolve_handle(*actor_id).ok_or(MessageContentError::MentionUnavailable)?;
                    if !valid_actor_handle(&handle) {
                        return Err(MessageContentError::InvalidMentionHandle);
                    }
                    plain_text.push('@');
                    plain_text.push_str(&handle);
                    plain_text_characters = plain_text_characters
                        .checked_add(1 + handle.chars().count())
                        .ok_or(MessageContentError::TextTooLong)?;
                    if seen.insert(*actor_id) {
                        mentioned_actor_ids.push(*actor_id);
                    }
                }
            }
            if plain_text_characters > MAX_MESSAGE_TEXT_CHARACTERS {
                return Err(MessageContentError::TextTooLong);
            }
        }

        if plain_text.trim().is_empty() {
            return Err(MessageContentError::Empty);
        }

        Ok(ValidatedMessageContent {
            content: self,
            plain_text,
            mentioned_actor_ids,
        })
    }
}

fn unsupported_control_character(character: char) -> bool {
    character.is_control() && !matches!(character, '\n' | '\r' | '\t')
}

fn valid_actor_handle(handle: &str) -> bool {
    let bytes = handle.as_bytes();
    (2..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_MESSAGE_MENTIONS, MAX_MESSAGE_NODES, MAX_MESSAGE_TEXT_CHARACTERS, MessageContent,
        MessageContentError, MessageNode, PlainTextMessageBuffer,
    };
    use uuid::Uuid;

    fn actor(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    #[test]
    fn version_one_round_trips_and_derives_plain_text() {
        let human = actor(1);
        let worker = actor(2);
        let content = MessageContent {
            version: 1,
            nodes: vec![
                MessageNode::Text {
                    text: "Ask ".to_owned(),
                },
                MessageNode::Mention { actor_id: human },
                MessageNode::Text {
                    text: " and ".to_owned(),
                },
                MessageNode::Mention { actor_id: worker },
                MessageNode::Mention { actor_id: human },
            ],
        };
        let json = serde_json::to_value(&content).expect("serialize content");
        assert_eq!(json["nodes"][1]["actorId"], human.to_string());
        let parsed = MessageContent::from_json(json).expect("parse content");

        let validated = parsed
            .validate_with_mentions(|actor_id| match actor_id {
                value if value == human => Some("mischa".to_owned()),
                value if value == worker => Some("agent".to_owned()),
                _ => None,
            })
            .expect("validate content");

        assert_eq!(validated.plain_text(), "Ask @mischa and @agent@mischa");
        assert_eq!(validated.mentioned_actor_ids(), &[human, worker]);
        assert_eq!(validated.content(), &content);
    }

    #[test]
    fn rejects_unknown_nodes_fields_and_malformed_actor_ids() {
        for value in [
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "link", "url": "javascript:alert(1)"}],
            }),
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "text", "text": "hello", "html": "<b>hello</b>"}],
            }),
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "mention", "actorId": "not-a-uuid"}],
            }),
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "mention", "actorId": Uuid::nil()}],
            }),
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "mention", "actor_id": actor(1)}],
            }),
            serde_json::json!({
                "version": 1,
                "nodes": [{"type": "text", "text": "hello"}],
                "authorActorId": actor(1),
            }),
        ] {
            assert_eq!(
                MessageContent::from_json(value),
                Err(MessageContentError::Malformed)
            );
        }
    }

    #[test]
    fn rejects_version_node_text_and_mention_bounds() {
        let unsupported = MessageContent {
            version: 2,
            nodes: vec![MessageNode::Text {
                text: "hello".to_owned(),
            }],
        };
        assert_eq!(
            unsupported.validate_structure(),
            Err(MessageContentError::UnsupportedVersion)
        );

        let too_many_nodes = MessageContent {
            version: 1,
            nodes: (0..=MAX_MESSAGE_NODES)
                .map(|_| MessageNode::Text {
                    text: "x".to_owned(),
                })
                .collect(),
        };
        assert_eq!(
            too_many_nodes.validate_structure(),
            Err(MessageContentError::TooManyNodes)
        );

        let too_much_text = MessageContent {
            version: 1,
            nodes: vec![MessageNode::Text {
                text: "🙂".repeat(MAX_MESSAGE_TEXT_CHARACTERS + 1),
            }],
        };
        assert_eq!(
            too_much_text.validate_structure(),
            Err(MessageContentError::TextTooLong)
        );

        let too_many_mentions = MessageContent {
            version: 1,
            nodes: (0..=MAX_MESSAGE_MENTIONS)
                .map(|index| MessageNode::Mention {
                    actor_id: actor(index as u128 + 1),
                })
                .collect(),
        };
        assert_eq!(
            too_many_mentions.validate_structure(),
            Err(MessageContentError::TooManyMentions)
        );
    }

    #[test]
    fn preserves_hostile_text_as_plain_text_but_rejects_controls() {
        let hostile = MessageContent {
            version: 1,
            nodes: vec![MessageNode::Text {
                text: "<img src=x onerror=alert(1)>\nhello".to_owned(),
            }],
        }
        .validate_with_mentions(|_| None)
        .expect("hostile markup remains inert text");
        assert_eq!(hostile.plain_text(), "<img src=x onerror=alert(1)>\nhello");

        let control = MessageContent {
            version: 1,
            nodes: vec![MessageNode::Text {
                text: "hello\0world".to_owned(),
            }],
        };
        assert_eq!(
            control.validate_structure(),
            Err(MessageContentError::UnsupportedControlCharacter)
        );
    }

    #[test]
    fn requires_resolvable_mentions_and_safe_project_handles() {
        let mentioned = actor(1);
        let content = || MessageContent {
            version: 1,
            nodes: vec![MessageNode::Mention {
                actor_id: mentioned,
            }],
        };

        assert_eq!(
            content().validate_with_mentions(|_| None),
            Err(MessageContentError::MentionUnavailable)
        );
        assert_eq!(
            content().validate_with_mentions(|_| Some("@agent".to_owned())),
            Err(MessageContentError::InvalidMentionHandle)
        );
    }

    #[test]
    fn derived_plain_text_cannot_exceed_the_text_limit() {
        let mentioned = actor(1);
        let content = MessageContent {
            version: 1,
            nodes: vec![
                MessageNode::Text {
                    text: "x".repeat(MAX_MESSAGE_TEXT_CHARACTERS - 1),
                },
                MessageNode::Mention {
                    actor_id: mentioned,
                },
            ],
        };
        assert_eq!(
            content.validate_with_mentions(|_| Some("agent".to_owned())),
            Err(MessageContentError::TextTooLong)
        );
    }

    #[test]
    fn plain_text_boundary_counts_unicode_scalar_values() {
        let exact = "🙂".repeat(MAX_MESSAGE_TEXT_CHARACTERS);
        let validated = MessageContent::from_plain_text(exact.clone())
            .expect("exact Unicode scalar-value boundary");
        assert_eq!(validated.plain_text(), exact);

        assert_eq!(
            MessageContent::from_plain_text(format!("{exact}🙂")),
            Err(MessageContentError::TextTooLong)
        );
    }

    #[test]
    fn streaming_buffer_rejects_a_fragment_before_mutating_safe_text() {
        let mut buffer = PlainTextMessageBuffer::default();
        buffer.push("safe\n").expect("safe provider fragment");
        let accepted = buffer.as_str().to_owned();

        assert_eq!(
            buffer.push("unsafe\u{0000}"),
            Err(MessageContentError::UnsupportedControlCharacter)
        );
        assert_eq!(buffer.as_str(), accepted);

        buffer
            .push(&"x".repeat(MAX_MESSAGE_TEXT_CHARACTERS - buffer.scalar_values()))
            .expect("fill exact provider boundary");
        assert_eq!(buffer.scalar_values(), MAX_MESSAGE_TEXT_CHARACTERS);
        assert_eq!(buffer.push("🙂"), Err(MessageContentError::TextTooLong));
        assert_eq!(buffer.scalar_values(), MAX_MESSAGE_TEXT_CHARACTERS);
    }
}
