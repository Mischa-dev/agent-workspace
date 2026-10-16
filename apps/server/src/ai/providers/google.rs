//! Google's Gemini API (`generateContent`) with a JSON response schema.

use serde_json::{Value, json};

use super::{
    Completion, CompletionRequest, ProviderError, ProviderManifest, base_url, send, tokens,
};
use crate::provider::manifest::ModelEntry;

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
    fn replies_are_read_from_the_first_candidate() {
        let reply = json!({
            "candidates": [{"content": {"parts": [{"text": "{\"ok\":"}, {"text": "true}"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 9, "candidatesTokenCount": 2}
        });
        let completion = parse(&reply).expect("parsed");
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (9, 2));
        let blocked = json!({"candidates": [{"finishReason": "SAFETY"}]});
        assert_eq!(parse(&blocked).unwrap_err(), ProviderError::Refused);
    }
}
