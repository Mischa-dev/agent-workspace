//! The manifests of harnesses a workspace registers (X2, A1.21): a
//! command-line harness or an agent that speaks the Agent Client Protocol.
//! The registration is a row the server keeps; the host and the server both
//! build its manifest from it, the same way.

use serde::Deserialize;
use serde_json::{Value, json};

use super::manifest::{
    AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginKind, LoginMethod, ModelEntry,
    RunLocation, RunOptionSupport, VersionRange,
};

/// The `aw-harness` line protocol version command-line harnesses speak.
pub const COMMAND_LINE_PROTOCOL: u32 = 1;
/// The Agent Client Protocol version the host speaks.
pub const ACP_PROTOCOL_VERSION: u64 = 1;
const MAX_MODELS: usize = 50;

/// What a registration says about its harness, as people write it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredManifest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub features: RegisteredFeatures,
    #[serde(default)]
    pub max_concurrent_runs: Option<u16>,
    #[serde(default)]
    pub api_key: bool,
    /// Accepted for the guide's examples; the registration's own name wins.
    #[serde(default)]
    #[allow(dead_code)]
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredFeatures {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub questions: bool,
    #[serde(default)]
    pub commands: bool,
}

/// Turns a registration into the manifest the host runs it by and the
/// server offers it with.
/// `runs_on` is the host for commands and the cloud for connected runners
/// (their turns never wait on this host's limit).
pub fn command_line_manifest(
    kind: &str,
    name: &str,
    manifest: &Value,
    runs_on: RunLocation,
) -> Result<AdapterManifest, String> {
    let registered: RegisteredManifest =
        serde_json::from_value(manifest.clone()).map_err(|error| format!("manifest: {error}"))?;
    if registered.models.len() > MAX_MODELS {
        return Err(format!("manifest: at most {MAX_MODELS} models"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for model in &registered.models {
        let id_ok = (1..=120).contains(&model.id.len())
            && model.id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
            });
        if !id_ok || !seen.insert(model.id.clone()) {
            return Err(format!(
                "manifest: model id {:?} is invalid or repeated",
                model.id
            ));
        }
        if model.display_name.trim().is_empty() || model.display_name.chars().count() > 120 {
            return Err(format!("manifest: model {} needs a display name", model.id));
        }
        if model
            .prices
            .as_ref()
            .is_some_and(|prices| prices.input_per_million < 0.0 || prices.output_per_million < 0.0)
        {
            return Err(format!("manifest: model {} has a negative price", model.id));
        }
    }
    let options = registered.options.clone().unwrap_or_else(
        || json!({ "type": "object", "properties": {}, "additionalProperties": false }),
    );
    let properties = options
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("manifest: options needs properties")?;
    for (option, schema) in properties {
        let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "string" | "boolean" | "integer" | "number")
            || option.is_empty()
            || option.len() > 64
            || !option
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!(
                "manifest: option {option} has an unsupported shape"
            ));
        }
    }
    let mut options = options;
    if let Some(object) = options.as_object_mut() {
        object.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    let concurrency = registered.max_concurrent_runs.unwrap_or(1).clamp(1, 16);
    let mut models = registered.models;
    if !models.iter().any(|model| model.is_default)
        && let Some(first) = models.first_mut()
    {
        first.is_default = true;
    }
    let description = registered
        .description
        .map(|text| text.trim().chars().take(500).collect::<String>())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| match runs_on {
            RunLocation::LocalHost => {
                "A command-line harness registered in this workspace.".to_owned()
            }
            RunLocation::Cloud => "A connected runner registered in this workspace.".to_owned(),
        });
    Ok(AdapterManifest {
        kind: kind.to_owned(),
        display_name: name.to_owned(),
        description,
        versions: VersionRange {
            minimum: format!("{COMMAND_LINE_PROTOCOL}.0.0"),
            below: Some(format!("{}.0.0", COMMAND_LINE_PROTOCOL + 1)),
        },
        branch_prefix: format!("{}/run-", kind.replace('_', "-")),
        runs_on,
        features: AdapterFeatures {
            streaming: false,
            commands: registered.features.commands,
            file_changes: true,
            approvals: registered.features.approvals,
            questions: registered.features.questions,
            resume: true,
            interrupt: true,
            cloud: runs_on == RunLocation::Cloud,
            multiple_attempts: false,
            concurrent_runs: concurrency > 1,
            model_listing: false,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 1,
            max_per_account: concurrency,
        },
        login_methods: if registered.api_key {
            vec![LoginMethod {
                kind: LoginKind::ApiKey,
                label: "Paste the key this harness signs in with. It reaches the harness as API_KEY on its secrets descriptor, never its environment.".to_owned(),
            }]
        } else {
            Vec::new()
        },
        run_options: RunOptionSupport {
            sandbox_modes: vec!["workspace_write".to_owned()],
            approval_policies: vec!["on_request".to_owned()],
            max_attempts: 1,
        },
        options_schema: options,
        default_models: models,
        open_locally: None,
    })
}

/// The manifest of an ACP registration: a command-line harness's, with
/// what the protocol always gives (streamed messages, tool calls, file
/// changes, permission requests).
pub fn acp_manifest(kind: &str, name: &str, manifest: &Value) -> Result<AdapterManifest, String> {
    let mut built = command_line_manifest(kind, name, manifest, RunLocation::LocalHost)?;
    let described = manifest
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if !described {
        built.description =
            "An agent that speaks the Agent Client Protocol, registered in this workspace."
                .to_owned();
    }
    built.versions = VersionRange {
        minimum: format!("{ACP_PROTOCOL_VERSION}.0.0"),
        below: Some(format!("{}.0.0", ACP_PROTOCOL_VERSION + 1)),
    };
    built.features.streaming = true;
    built.features.commands = true;
    built.features.file_changes = true;
    built.features.approvals = true;
    built.features.questions = false;
    Ok(built)
}
