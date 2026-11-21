//! The MCP server card (K2.20), `GET /.well-known/mcp/server-card.json`:
//! what a client or a registry can learn before connecting. Its name and
//! version, the revisions it speaks and its capabilities come from the
//! handler's own `get_info`, its tools from the same catalogue `tools/list`
//! draws on (which tools a caller sees depends on their roles there).

use axum::{Json, Router, extract::State, routing::get};
use rmcp::{ServerHandler as _, model::ProtocolVersion};
use serde_json::{Value, json};

use crate::AppState;

pub(super) const SERVER_CARD_PATH: &str = "/.well-known/mcp/server-card.json";

pub(super) fn router() -> Router<AppState> {
    Router::new().route(SERVER_CARD_PATH, get(server_card))
}

async fn server_card(State(state): State<AppState>) -> Json<Value> {
    let origin = super::oauth::public_origin(&state);
    let info = super::McpServer {
        state: state.clone(),
    }
    .get_info();
    let tools = super::tool_catalog()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "title": tool.title.or_else(|| {
                    tool.annotations.as_ref().and_then(|annotations| annotations.title.clone())
                }),
                "readOnly": tool
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.read_only_hint)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "serverInfo": info.server_info,
        "protocolVersion": ProtocolVersion::LATEST.as_str(),
        "supportedProtocolVersions": ProtocolVersion::KNOWN_VERSIONS
            .iter()
            .map(ProtocolVersion::as_str)
            .collect::<Vec<_>>(),
        "capabilities": info.capabilities,
        "instructions": info.instructions,
        "transport": {
            "type": "streamable-http",
            "endpoint": format!("{origin}{}", super::MCP_PATH),
        },
        "authentication": {
            "required": true,
            "schemes": ["oauth2", "bearer"],
            "protectedResourceMetadata":
                format!("{origin}/.well-known/oauth-protected-resource{}", super::MCP_PATH),
        },
        "tools": tools,
        "resources": "dynamic",
        "prompts": "dynamic",
    }))
}
