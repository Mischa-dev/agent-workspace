//! Anthropic's Messages API, with structured output against the prompt's
//! JSON Schema (`output_config.format`).

use serde_json::{Value, json};

use super::{
    Completion, CompletionRequest, ProviderError, ProviderManifest, base_url, model, send, tokens,
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
