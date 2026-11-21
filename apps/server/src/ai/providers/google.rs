//! Google's Gemini API (`generateContent`) with a JSON response schema.

use serde_json::{Value, json};

use super::{
    Chat, ChatRequest, Completion, CompletionRequest, ModelPart, ModelRole, ModelStop,
    ProviderError, ProviderManifest, address, base_url, object, send, tokens, tool_schema,
};
use crate::harness::manifest::ModelEntry;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

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
        id: "google",
        display_name: "Google",
        description: "Gemini models through the Gemini API, with your API key. Add prices in the model catalog to count spend.",
        needs_key: true,
        needs_base_url: false,
        default_base_url: Some(DEFAULT_BASE_URL),
        default_quick_model: "gemini-2.5-flash",
        default_deep_model: "gemini-2.5-pro",
        models: vec![
            unpriced("gemini-2.5-flash", "Gemini 2.5 Flash", true),
            unpriced("gemini-2.5-pro", "Gemini 2.5 Pro", false),
        ],
        testing_only: false,
    }
}

pub fn body(request: &CompletionRequest<'_>) -> Value {
    json!({
        "systemInstruction": {"parts": [{"text": request.system}]},
        "contents": [{"role": "user", "parts": [{"text": request.input}]}],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseJsonSchema": request.schema,
            "maxOutputTokens": request.max_output_tokens,
        },
    })
}

pub async fn complete(
    client: &reqwest::Client,
    request: &CompletionRequest<'_>,
) -> Result<Completion, ProviderError> {
    let key = request.api_key.ok_or(ProviderError::Unauthorized)?;
    let base = base_url(request, DEFAULT_BASE_URL)?;
    let mut url = url::Url::parse(&base)
        .map_err(|_| ProviderError::BadRequest("the provider address isn't a URL".to_owned()))?;
    url.path_segments_mut()
        .map_err(|_| ProviderError::BadRequest("the provider address isn't a URL".to_owned()))?
        .push("models")
        .push(&format!("{}:generateContent", request.model));
    let reply = send(
        client
            .post(url)
            .timeout(request.timeout)
            .header("x-goog-api-key", key.expose())
            .json(&body(request)),
    )
    .await?;
    parse(&reply)
}

/// A1.25: a turn with tools as `generateContent` contents: calls and their
/// results are `functionCall` and `functionResponse` parts, matched by name.
pub fn chat_body(request: &ChatRequest<'_>) -> Value {
    let contents = request
        .turn
        .messages
        .iter()
        .filter(|message| !message.parts.is_empty())
        .map(|message| {
            let parts = message
                .parts
                .iter()
                .map(|part| match part {
                    ModelPart::Text { text } => json!({"text": text}),
                    ModelPart::ToolCall { id, name, input } => {
                        json!({"functionCall": {"id": id, "name": name, "args": object(input)}})
                    }
                    ModelPart::ToolResult {
                        id,
                        name,
                        output,
                        is_error,
                    } => json!({"functionResponse": {"id": id, "name": name,
                                "response": {"output": output, "isError": is_error}}}),
                })
                .collect::<Vec<_>>();
            let role = match message.role {
                ModelRole::User => "user",
                ModelRole::Assistant => "model",
            };
            json!({"role": role, "parts": parts})
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "systemInstruction": {"parts": [{"text": request.turn.system}]},
        "contents": contents,
        "generationConfig": {"maxOutputTokens": request.max_output_tokens},
    });
    if !request.turn.tools.is_empty() {
        let declarations = request
            .turn
            .tools
            .iter()
            .map(|tool| {
                json!({"name": tool.name, "description": tool.description,
                       "parametersJsonSchema": tool_schema(&tool.input_schema)})
            })
            .collect::<Vec<_>>();
        body["tools"] = json!([{"functionDeclarations": declarations}]);
    }
    body
}

pub async fn chat(
    client: &reqwest::Client,
    request: &ChatRequest<'_>,
) -> Result<Chat, ProviderError> {
    let key = request.api_key.ok_or(ProviderError::Unauthorized)?;
    let base = address(request.base_url, DEFAULT_BASE_URL)?;
    let mut url = url::Url::parse(&base)
        .map_err(|_| ProviderError::BadRequest("the provider address isn't a URL".to_owned()))?;
    url.path_segments_mut()
        .map_err(|_| ProviderError::BadRequest("the provider address isn't a URL".to_owned()))?
        .push("models")
        .push(&format!("{}:generateContent", request.model));
    let reply = send(
        client
            .post(url)
            .timeout(request.timeout)
            .header("x-goog-api-key", key.expose())
            .json(&chat_body(request)),
    )
    .await?;
    parse_chat(&reply)
}

pub fn parse_chat(reply: &Value) -> Result<Chat, ProviderError> {
    if reply.pointer("/promptFeedback/blockReason").is_some() {
        return Err(ProviderError::Refused);
    }
    let finish = reply
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str);
    if matches!(
        finish,
        Some("SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "RECITATION")
    ) {
        return Err(ProviderError::Refused);
    }
    let mut parts = Vec::new();
    for (index, part) in reply
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) != Some(true) {
                parts.push(ModelPart::Text {
                    text: text.to_owned(),
                });
            }
        } else if let Some(call) = part.get("functionCall")
            && let Some(name) = call.get("name").and_then(Value::as_str)
        {
            // Older models give calls no id: their place stands in for one.
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("call-{index}"), str::to_owned);
            parts.push(ModelPart::ToolCall {
                id,
                name: name.to_owned(),
                input: call.get("args").cloned().unwrap_or(Value::Null),
            });
        }
    }
    let calls = parts
        .iter()
        .any(|part| matches!(part, ModelPart::ToolCall { .. }));
    let stop = match finish {
        Some("MAX_TOKENS") => ModelStop::MaxTokens,
        _ if calls => ModelStop::ToolUse,
        _ => ModelStop::End,
    };
    Ok(Chat {
        parts,
        stop,
        input_tokens: tokens(reply.pointer("/usageMetadata/promptTokenCount")),
        output_tokens: tokens(reply.pointer("/usageMetadata/candidatesTokenCount")),
    })
}

pub fn parse(reply: &Value) -> Result<Completion, ProviderError> {
    if reply.pointer("/promptFeedback/blockReason").is_some() {
        return Err(ProviderError::Refused);
    }
    match reply
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
    {
        Some("SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "RECITATION") => {
            return Err(ProviderError::Refused);
        }
        Some("MAX_TOKENS") => {
            return Err(ProviderError::BadRequest(
                "the reply ran past its token limit".to_owned(),
            ));
        }
        _ => {}
    }
    let text = reply
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    Ok(Completion {
        text,
        input_tokens: tokens(reply.pointer("/usageMetadata/promptTokenCount")),
        output_tokens: tokens(reply.pointer("/usageMetadata/candidatesTokenCount")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_calls_are_tool_calls() {
        let chat = parse_chat(&json!({"candidates": [{"content": {"parts": [
            {"text": "Let me look."},
            {"functionCall": {"name": "search", "args": {"q": "x"}}}]}, "finishReason": "STOP"}]}))
        .expect("chat");
        assert_eq!(chat.stop, ModelStop::ToolUse);
        assert!(matches!(&chat.parts[1], ModelPart::ToolCall { id, .. } if id == "call-1"));
    }

    #[test]
    fn replies_are_read_from_the_first_candidate() {
        let reply = json!({
            "candidates": [{"content": {"parts": [{"text": "{\"ok\":"}, {"text": "true}"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 9, "candidatesTokenCount": 2}
        });
        let completion = parse(&reply).expect("parsed");
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (9, 2));
        let blocked = json!({"candidates": [{"finishReason": "SAFETY"}]});
        assert_eq!(
            parse(&blocked).expect_err("blocked"),
            ProviderError::Refused
        );
    }
}
