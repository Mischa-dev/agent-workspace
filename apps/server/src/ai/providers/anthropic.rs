//! Anthropic's Messages API, with structured output against the prompt's
//! JSON Schema (`output_config.format`), and tool use for an assistant's
//! turns (A1.25).

use serde_json::{Value, json};

use super::{
    Chat, ChatRequest, Completion, CompletionRequest, ModelPart, ModelRole, ModelStop,
    ProviderError, ProviderManifest, address, base_url, model, object, send, tokens, tool_schema,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub fn manifest() -> ProviderManifest {
    ProviderManifest {
        id: "anthropic",
        display_name: "Anthropic",
        description: "Claude models through the Anthropic API, with your API key.",
        needs_key: true,
        needs_base_url: false,
        default_base_url: Some(DEFAULT_BASE_URL),
        default_quick_model: "claude-haiku-5-5",
        default_deep_model: "claude-opus-5-5",
        models: vec![
            model("claude-haiku-5-5", "Claude Haiku 5.5", 0.10, 0.50, true),
            model("claude-sonnet-5-5", "Claude Sonnet 5.5", 2.0, 10.0, false),
            model("claude-opus-5-5", "Claude Opus 5.5", 4.0, 20.0, false),
        ],
        testing_only: false,
    }
}

pub fn body(request: &CompletionRequest<'_>) -> Value {
    json!({
        "model": request.model,
        "max_tokens": request.max_output_tokens,
        "system": request.system,
        "messages": [{"role": "user", "content": request.input}],
        "output_config": {"format": {"type": "json_schema", "schema": request.schema}},
    })
}

pub async fn complete(
    client: &reqwest::Client,
    request: &CompletionRequest<'_>,
) -> Result<Completion, ProviderError> {
    let key = request.api_key.ok_or(ProviderError::Unauthorized)?;
    let url = format!("{}/v1/messages", base_url(request, DEFAULT_BASE_URL)?);
    let reply = send(
        client
            .post(url)
            .timeout(request.timeout)
            .header("x-api-key", key.expose())
            .header("anthropic-version", API_VERSION)
            .json(&body(request)),
    )
    .await?;
    parse(&reply)
}

pub fn chat_body(request: &ChatRequest<'_>) -> Value {
    let messages = request
        .turn
        .messages
        .iter()
        .filter(|message| !message.parts.is_empty())
        .map(|message| {
            let content = message
                .parts
                .iter()
                .map(|part| match part {
                    ModelPart::Text { text } => json!({"type": "text", "text": text}),
                    ModelPart::ToolCall { id, name, input } => {
                        json!({"type": "tool_use", "id": id, "name": name, "input": object(input)})
                    }
                    ModelPart::ToolResult {
                        id,
                        output,
                        is_error,
                        ..
                    } => json!({"type": "tool_result", "tool_use_id": id,
                                "content": output, "is_error": is_error}),
                })
                .collect::<Vec<_>>();
            let role = match message.role {
                ModelRole::User => "user",
                ModelRole::Assistant => "assistant",
            };
            json!({"role": role, "content": content})
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_output_tokens,
        "system": request.turn.system,
        "messages": messages,
    });
    if !request.turn.tools.is_empty() {
        body["tools"] = request
            .turn
            .tools
            .iter()
            .map(|tool| {
                json!({"name": tool.name, "description": tool.description,
                       "input_schema": tool_schema(&tool.input_schema)})
            })
            .collect();
    }
    body
}

pub async fn chat(
    client: &reqwest::Client,
    request: &ChatRequest<'_>,
) -> Result<Chat, ProviderError> {
    let key = request.api_key.ok_or(ProviderError::Unauthorized)?;
    let url = format!(
        "{}/v1/messages",
        address(request.base_url, DEFAULT_BASE_URL)?
    );
    let reply = send(
        client
            .post(url)
            .timeout(request.timeout)
            .header("x-api-key", key.expose())
            .header("anthropic-version", API_VERSION)
            .json(&chat_body(request)),
    )
    .await?;
    parse_chat(&reply)
}

pub fn parse_chat(reply: &Value) -> Result<Chat, ProviderError> {
    let stop = match reply.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => return Err(ProviderError::Refused),
        Some("max_tokens") => ModelStop::MaxTokens,
        Some("tool_use") => ModelStop::ToolUse,
        _ => ModelStop::End,
    };
    let parts = reply
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => Some(ModelPart::Text {
                text: block.get("text")?.as_str()?.to_owned(),
            }),
            Some("tool_use") => Some(ModelPart::ToolCall {
                id: block.get("id")?.as_str()?.to_owned(),
                name: block.get("name")?.as_str()?.to_owned(),
                input: block.get("input").cloned().unwrap_or(Value::Null),
            }),
            _ => None,
        })
        .collect();
    Ok(Chat {
        parts,
        stop,
        input_tokens: tokens(reply.pointer("/usage/input_tokens")),
        output_tokens: tokens(reply.pointer("/usage/output_tokens")),
    })
}

pub fn parse(reply: &Value) -> Result<Completion, ProviderError> {
    match reply.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => return Err(ProviderError::Refused),
        Some("max_tokens") => {
            return Err(ProviderError::BadRequest(
                "the reply ran past its token limit".to_owned(),
            ));
        }
        _ => {}
    }
    let text = reply
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    Ok(Completion {
        text,
        input_tokens: tokens(reply.pointer("/usage/input_tokens")),
        output_tokens: tokens(reply.pointer("/usage/output_tokens")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::providers::{ModelMessage, ModelTool, ModelTurn};

    #[test]
    fn a_turn_with_tools_round_trips() {
        let turn = ModelTurn {
            system: "Be brief.".to_owned(),
            messages: vec![
                ModelMessage::user("Open AW-1"),
                ModelMessage {
                    role: ModelRole::Assistant,
                    parts: vec![ModelPart::ToolCall {
                        id: "t1".to_owned(),
                        name: "task_open".to_owned(),
                        input: json!({"taskId": "AW-1"}),
                    }],
                },
                ModelMessage {
                    role: ModelRole::User,
                    parts: vec![ModelPart::ToolResult {
                        id: "t1".to_owned(),
                        name: "task_open".to_owned(),
                        output: "{}".to_owned(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ModelTool {
                name: "task_open".to_owned(),
                description: "Open a task".to_owned(),
                input_schema: json!({"type": "object"}),
            }],
            max_output_tokens: None,
        };
        let request = ChatRequest {
            model: "m",
            turn: &turn,
            max_output_tokens: 100,
            api_key: None,
            base_url: None,
            timeout: std::time::Duration::from_secs(1),
        };
        let body = chat_body(&request);
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        let chat = parse_chat(&json!({
            "stop_reason": "tool_use",
            "content": [{"type": "text", "text": "Looking."},
                        {"type": "tool_use", "id": "t2", "name": "search", "input": {"q": "x"}}],
            "usage": {"input_tokens": 5, "output_tokens": 2}
        }))
        .expect("chat");
        assert_eq!(chat.stop, ModelStop::ToolUse);
        assert_eq!(chat.parts.len(), 2);
    }

    #[test]
    fn replies_are_read_from_text_blocks() {
        let reply = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"ok\":true}"}],
            "usage": {"input_tokens": 12, "output_tokens": 3}
        });
        let completion = parse(&reply).expect("parsed");
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (12, 3));
        assert_eq!(
            parse(&json!({"stop_reason": "refusal", "content": []})).expect_err("refused"),
            ProviderError::Refused
        );
    }
}
