//! OpenAI's Chat Completions API, and any server that speaks it (OpenRouter,
//! Ollama, vLLM and the like), with a JSON Schema response format.

use serde_json::{Value, json};

use super::{
    Completion, CompletionRequest, ProviderError, ProviderManifest, base_url, send, tokens,
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
    fn replies_are_read_from_the_first_choice() {
        let reply = json!({
            "choices": [{"message": {"content": "{\"ok\":true}", "refusal": null}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 4}
        });
        let completion = parse(&reply).expect("parsed");
        assert_eq!(completion.text, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (20, 4));
        let refused = json!({"choices": [{"message": {"content": null, "refusal": "no"}}]});
        assert_eq!(parse(&refused).unwrap_err(), ProviderError::Refused);
    }
}
