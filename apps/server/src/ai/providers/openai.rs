//! OpenAI's Chat Completions API, and any server that speaks it (OpenRouter,
//! Ollama, vLLM and the like), with a JSON Schema response format.

use serde_json::{Value, json};

use super::{
    Chat, ChatRequest, Completion, CompletionRequest, ModelPart, ModelRole, ModelStop,
    ProviderError, ProviderManifest, address, base_url, object, send, tokens, tool_schema,
};
use crate::provider::manifest::ModelEntry;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

fn unpriced(id: &str, name: &str, is_default: bool) -> ModelEntry {
    ModelEntry {
        id: id.to_owned(),
        display_name: name.to_owned(),
        is_default,
        ..ModelEntry::default()
    }
}

pub fn manifest() -> ProviderManifest {
    ProviderManifest {
        id: "openai",
        display_name: "OpenAI",
        description: "OpenAI models through the OpenAI API, with your API key. Add prices in the model catalog to count spend.",
        needs_key: true,
        needs_base_url: false,
        default_base_url: Some(DEFAULT_BASE_URL),
        default_quick_model: "gpt-5-mini",
        default_deep_model: "gpt-5",
        models: vec![
            unpriced("gpt-5-mini", "GPT-5 mini", true),
            unpriced("gpt-5", "GPT-5", false),
        ],
        testing_only: false,
    }
}

pub fn compatible_manifest() -> ProviderManifest {
    ProviderManifest {
        id: "openai-compatible",
        display_name: "OpenAI-compatible server",
        description: "Any server with an OpenAI-style chat completions API, such as OpenRouter, Ollama or vLLM. Set its address; a key only if it needs one.",
        needs_key: false,
        needs_base_url: true,
        default_base_url: None,
        default_quick_model: "llama3.1",
        default_deep_model: "llama3.1",
        models: Vec::new(),
        testing_only: false,
    }
}

pub fn body(request: &CompletionRequest<'_>, strict: bool) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": [
            {"role": "system", "content": request.system},
            {"role": "user", "content": request.input},
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": request.prompt_id, "schema": request.schema, "strict": strict},
        },
    });
    // OpenAI's own API takes max_completion_tokens; older compatible
    // servers only know max_tokens.
    let limit = if strict {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[limit] = json!(request.max_output_tokens);
    body
}

pub async fn complete(
    client: &reqwest::Client,
    request: &CompletionRequest<'_>,
    official: bool,
) -> Result<Completion, ProviderError> {
    let base = if official {
        base_url(request, DEFAULT_BASE_URL)?
    } else {
        let Some(address) = request.base_url else {
            return Err(ProviderError::BadRequest(
                "set the server's address first".to_owned(),
            ));
        };
        base_url(request, address)?
    };
    let mut call = client
        .post(format!("{base}/chat/completions"))
        .timeout(request.timeout)
        .json(&body(request, official));
    match request.api_key {
        Some(key) => call = call.bearer_auth(key.expose()),
        None if official => return Err(ProviderError::Unauthorized),
        None => {}
    }
    parse(&send(call).await?)
}

/// A1.25: a turn with tools as chat completion messages: a tool's result
/// is a `tool` message of its own, and a call's input is JSON text.
pub fn chat_body(request: &ChatRequest<'_>, strict: bool) -> Value {
    let mut messages = vec![json!({"role": "system", "content": request.turn.system})];
    for message in &request.turn.messages {
        match message.role {
            ModelRole::User => {
                for part in &message.parts {
                    if let ModelPart::ToolResult { id, output, .. } = part {
                        messages
                            .push(json!({"role": "tool", "tool_call_id": id, "content": output}));
                    }
                }
                let text = message.text();
                if !text.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
            }
            ModelRole::Assistant => {
                let calls = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ModelPart::ToolCall { id, name, input } => Some(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": object(input).to_string()},
                        })),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let text = message.text();
                let mut entry = json!({"role": "assistant",
                                       "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    entry["tool_calls"] = json!(calls);
                }
                messages.push(entry);
            }
        }
    }
    let mut body = json!({"model": request.model, "messages": messages});
    if !request.turn.tools.is_empty() {
        body["tools"] = request
            .turn
            .tools
            .iter()
            .map(|tool| {
                json!({"type": "function", "function": {
                    "name": tool.name, "description": tool.description,
                    "parameters": tool_schema(&tool.input_schema)}})
            })
            .collect();
    }
    let limit = if strict {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[limit] = json!(request.max_output_tokens);
    body
}

pub async fn chat(
    client: &reqwest::Client,
    request: &ChatRequest<'_>,
    official: bool,
) -> Result<Chat, ProviderError> {
    let base = if official {
        address(request.base_url, DEFAULT_BASE_URL)?
    } else {
        let Some(configured) = request.base_url else {
            return Err(ProviderError::BadRequest(
                "set the server's address first".to_owned(),
            ));
        };
        address(Some(configured), configured)?
    };
    let mut call = client
        .post(format!("{base}/chat/completions"))
        .timeout(request.timeout)
        .json(&chat_body(request, official));
    match request.api_key {
        Some(key) => call = call.bearer_auth(key.expose()),
        None if official => return Err(ProviderError::Unauthorized),
        None => {}
    }
    parse_chat(&send(call).await?)
}

pub fn parse_chat(reply: &Value) -> Result<Chat, ProviderError> {
    let message = reply.pointer("/choices/0/message");
    if message
        .and_then(|message| message.get("refusal"))
        .is_some_and(|refusal| !refusal.is_null())
    {
        return Err(ProviderError::Refused);
    }
    let mut parts = Vec::new();
    if let Some(text) = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        parts.push(ModelPart::Text {
            text: text.to_owned(),
        });
    }
    for call in message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(name)) = (
            call.get("id").and_then(Value::as_str),
            call.pointer("/function/name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let input = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
            .unwrap_or(Value::Null);
        parts.push(ModelPart::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
        });
    }
    let calls = parts
        .iter()
        .any(|part| matches!(part, ModelPart::ToolCall { .. }));
    let stop = match reply
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
    {
        Some("length") => ModelStop::MaxTokens,
        _ if calls => ModelStop::ToolUse,
        _ => ModelStop::End,
    };
    Ok(Chat {
        parts,
        stop,
        input_tokens: tokens(reply.pointer("/usage/prompt_tokens")),
        output_tokens: tokens(reply.pointer("/usage/completion_tokens")),
    })
}

pub fn parse(reply: &Value) -> Result<Completion, ProviderError> {
    let message = reply.pointer("/choices/0/message");
    if message
        .and_then(|message| message.get("refusal"))
        .is_some_and(|refusal| !refusal.is_null())
    {
        return Err(ProviderError::Refused);
    }
    if reply
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length")
    {
        return Err(ProviderError::BadRequest(
            "the reply ran past its token limit".to_owned(),
        ));
    }
    let text = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(Completion {
        text,
        input_tokens: tokens(reply.pointer("/usage/prompt_tokens")),
        output_tokens: tokens(reply.pointer("/usage/completion_tokens")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls_and_results_are_chat_messages() {
        use crate::ai::providers::{ModelMessage, ModelTurn};
        let turn = ModelTurn {
            system: "s".to_owned(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::Assistant,
                    parts: vec![ModelPart::ToolCall {
                        id: "c1".to_owned(),
                        name: "search".to_owned(),
                        input: json!({"q": "x"}),
                    }],
                },
                ModelMessage {
                    role: ModelRole::User,
                    parts: vec![ModelPart::ToolResult {
                        id: "c1".to_owned(),
                        name: "search".to_owned(),
                        output: "[]".to_owned(),
                        is_error: false,
                    }],
                },
            ],
            tools: Vec::new(),
            max_output_tokens: None,
        };
        let request = ChatRequest {
            model: "m",
            turn: &turn,
            max_output_tokens: 10,
            api_key: None,
            base_url: None,
            timeout: std::time::Duration::from_secs(1),
        };
        let body = chat_body(&request, true);
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"x\"}"
        );
        assert_eq!(body["messages"][2]["role"], "tool");
        let chat = parse_chat(&json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"id": "c2", "type": "function", "function": {"name": "search", "arguments": "{\"q\":1}"}}]},
            "finish_reason": "tool_calls"}]})).expect("chat");
        assert_eq!(chat.stop, ModelStop::ToolUse);
        assert!(matches!(&chat.parts[0], ModelPart::ToolCall { input, .. } if input["q"] == 1));
    }

    #[test]
    fn replies_are_read_from_the_first_choice() {
        let reply = json!({
            "choices": [{"message": {"content": "{\"ok\":true}", "refusal": null}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 4}
        });
        let completion = parse(&reply).expect("parsed");
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (20, 4));
        let refused = json!({"choices": [{"message": {"content": null, "refusal": "no"}}]});
        assert_eq!(
            parse(&refused).expect_err("refused"),
            ProviderError::Refused
        );
    }
}
