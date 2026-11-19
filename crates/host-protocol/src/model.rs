//! A1.25: a model turn through the server's model gateway. A host whose
//! agent thinks with the workspace's own model (the built-in host's
//! assistant) sends the conversation so far and the tools it may call to
//! `POST /api/v1/host/model` with the run's credential; the server calls the
//! provider with the workspace's key, which never leaves it, records the
//! tokens on the run and against the workspace's model budget, and answers
//! with what the model said: text, tool calls, or both.
//!
//! The shapes are the provider-neutral ones the server's gateway speaks;
//! each provider's own JSON stays in the server's provider adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most messages one turn carries.
pub const MAX_MESSAGES: usize = 200;
/// The most tools one turn offers.
pub const MAX_TOOLS: usize = 128;

/// One call to the model: instructions, the conversation, the tools.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTurn {
    pub system: String,
    pub messages: Vec<ModelMessage>,
    #[serde(default)]
    pub tools: Vec<ModelTool>,
    /// The most the model may write in its answer; the server caps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMessage {
    pub role: ModelRole,
    pub parts: Vec<ModelPart>,
}

impl ModelMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ModelRole::User,
            parts: vec![ModelPart::Text { text: text.into() }],
        }
    }

    /// The message's text parts, joined.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                ModelPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A piece of a message. Tool calls come from the model; their results go
/// back in the next user message, by the call's id and tool name.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelPart {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        id: String,
        name: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
}

/// A tool the model may call: its name, what it does, and its input's JSON
/// Schema.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Why the model stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStop {
    /// It answered.
    End,
    /// It wants its tool calls carried out, then to go on.
    ToolUse,
    /// It ran out of room to answer.
    MaxTokens,
}

/// What the model said, and what it cost.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReply {
    pub parts: Vec<ModelPart>,
    pub stop: ModelStop,
    /// The model that answered, as the workspace's settings chose it.
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl ModelReply {
    /// The tool calls in the reply, in order.
    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.parts.iter().filter_map(|part| match part {
            ModelPart::ToolCall { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    /// The reply's text parts, joined.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                ModelPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parts_are_tagged_by_type() {
        let message = ModelMessage {
            role: ModelRole::User,
            parts: vec![ModelPart::ToolResult {
                id: "c1".to_owned(),
                name: "task_open".to_owned(),
                output: "{}".to_owned(),
                is_error: false,
            }],
        };
        assert_eq!(
            serde_json::to_value(&message).expect("json"),
            json!({"role": "user", "parts": [{"type": "tool_result", "id": "c1",
                   "name": "task_open", "output": "{}", "isError": false}]})
        );
        let reply: ModelReply = serde_json::from_value(json!({
            "parts": [{"type": "text", "text": "Looking."},
                      {"type": "tool_call", "id": "c2", "name": "search", "input": {"q": "x"}}],
            "stop": "tool_use", "model": "m", "inputTokens": 3, "outputTokens": 4
        }))
        .expect("reply");
        assert_eq!(reply.tool_calls().count(), 1);
        assert_eq!(reply.text(), "Looking.");
    }
}
