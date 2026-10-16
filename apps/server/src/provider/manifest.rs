//! What each harness adapter publishes about itself (H3): its models, what it
//! can do, how many turns an account may run at once, and a JSON Schema for
//! any extra run options. Settings and the Launch dialog draw from this, so a
//! new harness needs no web code.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterManifest {
    pub kind: String,
    pub display_name: String,
    pub description: String,
    /// Binary or protocol versions the adapter speaks: at least `minimum`,
    /// and below `below` when set.
    pub versions: VersionRange,
    /// Run branches start with this, e.g. `codex/run-`.
    pub branch_prefix: String,
    pub runs_on: RunLocation,
    pub features: AdapterFeatures,
    pub concurrency: ConcurrencyLimits,
    pub login_methods: Vec<LoginMethod>,
    pub run_options: RunOptionSupport,
    /// JSON Schema for adapter-specific options (`RunConfig::options`). Only
    /// the subset `validate_options` understands is used.
    pub options_schema: Value,
    /// The catalog to start from when the harness can't list its models.
    pub default_models: Vec<ModelEntry>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRange {
    pub minimum: String,
    pub below: Option<String>,
}

impl VersionRange {
    pub fn contains(&self, version: &str) -> bool {
        let Some(version) = parse_version(version) else {
            return false;
        };
        let Some(minimum) = parse_version(&self.minimum) else {
            return false;
        };
        if version < minimum {
            return false;
        }
        match self.below.as_deref().map(parse_version) {
            None => true,
            Some(Some(below)) => version < below,
            Some(None) => false,
        }
    }
}

/// Dotted numeric versions (`0.146.0`); a pre-release or build suffix is
/// ignored for ordering.
fn parse_version(value: &str) -> Option<Vec<u64>> {
    let core = value.split(['-', '+']).next()?;
    let parts = core
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut padded = parts;
    padded.resize(4, 0);
    Some(padded)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLocation {
    /// Runs on this server, so the host-wide run limit applies.
    LocalHost,
    /// Runs elsewhere; never waits on the host limit.
    Cloud,
}

impl RunLocation {
    /// The `provider_connections.host` value for connections of this kind.
    pub fn host(self) -> &'static str {
        match self {
            Self::LocalHost => "local_project_host",
            Self::Cloud => "cloud",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterFeatures {
    pub streaming: bool,
    pub commands: bool,
    pub file_changes: bool,
    pub approvals: bool,
    pub questions: bool,
    pub resume: bool,
    pub interrupt: bool,
    pub cloud: bool,
    pub multiple_attempts: bool,
    pub concurrent_runs: bool,
    pub model_listing: bool,
    pub pull_request_creation: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyLimits {
    pub default_per_account: u16,
    pub max_per_account: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginMethod {
    pub kind: super::adapter::LoginKind,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptionSupport {
    pub sandbox_modes: Vec<String>,
    pub approval_policies: Vec<String>,
    pub max_attempts: u8,
}

/// One model in a harness's catalog.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    #[serde(default)]
    pub default_reasoning: Option<String>,
    #[serde(default)]
    pub speeds: Vec<SpeedOption>,
    #[serde(default)]
    pub default_speed: Option<String>,
    #[serde(default)]
    pub prices: Option<ModelPrices>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedOption {
    pub id: String,
    pub name: String,
}

/// Prices per million tokens.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPrices {
    pub currency: String,
    pub input_per_million: f64,
    pub output_per_million: f64,
    #[serde(default)]
    pub cached_input_per_million: Option<f64>,
}

/// The fields a person may change on a catalog model. A refresh from the
/// provider never touches them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speeds: Option<Vec<SpeedOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prices: Option<ModelPrices>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

impl ModelOverrides {
    pub fn validate(&self) -> Result<(), &'static str> {
        let text = |value: &str, max: usize| {
            !value.trim().is_empty()
                && value.chars().count() <= max
                && !value.chars().any(char::is_control)
        };
        if self
            .display_name
            .as_deref()
            .is_some_and(|value| !text(value, 120))
        {
            return Err("displayName");
        }
        if self
            .description
            .as_deref()
            .is_some_and(|value| !text(value, 2000))
        {
            return Err("description");
        }
        if self
            .context_window
            .is_some_and(|value| !(1..=100_000_000).contains(&value))
        {
            return Err("contextWindow");
        }
        let identifier = |value: &str| {
            (1..=64).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        };
        if self.reasoning_levels.as_ref().is_some_and(|levels| {
            levels.len() > 16 || !levels.iter().all(|level| identifier(level))
        }) {
            return Err("reasoningLevels");
        }
        if self
            .default_reasoning
            .as_deref()
            .is_some_and(|value| !identifier(value))
        {
            return Err("defaultReasoning");
        }
        if self.speeds.as_ref().is_some_and(|speeds| {
            speeds.len() > 16
                || !speeds
                    .iter()
                    .all(|speed| identifier(&speed.id) && text(&speed.name, 120))
        }) {
            return Err("speeds");
        }
        if self
            .default_speed
            .as_deref()
            .is_some_and(|value| !identifier(value))
        {
            return Err("defaultSpeed");
        }
        if let Some(prices) = &self.prices {
            let price = |value: f64| value.is_finite() && (0.0..=1_000_000.0).contains(&value);
            if !(prices.currency.len() == 3
                && prices.currency.bytes().all(|b| b.is_ascii_uppercase()))
                || !price(prices.input_per_million)
                || !price(prices.output_per_million)
                || prices
                    .cached_input_per_million
                    .is_some_and(|value| !price(value))
            {
                return Err("prices");
            }
        }
        Ok(())
    }

    /// The catalog entry a person sees: what the provider reported, with
    /// their edits on top.
    pub fn apply(&self, mut model: ModelEntry) -> ModelEntry {
        if let Some(value) = &self.display_name {
            model.display_name = value.clone();
        }
        if let Some(value) = &self.description {
            model.description = Some(value.clone());
        }
        if let Some(value) = self.context_window {
            model.context_window = Some(value);
        }
        if let Some(value) = &self.reasoning_levels {
            model.reasoning_levels = value.clone();
        }
        if let Some(value) = &self.default_reasoning {
            model.default_reasoning = Some(value.clone());
        }
        if let Some(value) = &self.speeds {
            model.speeds = value.clone();
        }
        if let Some(value) = &self.default_speed {
            model.default_speed = Some(value.clone());
        }
        if let Some(value) = &self.prices {
            model.prices = Some(value.clone());
        }
        if let Some(value) = self.hidden {
            model.hidden = value;
        }
        if let Some(value) = self.is_default {
            model.is_default = value;
        }
        model
    }
}

/// Checks adapter options against the subset of JSON Schema manifests use:
/// an object whose properties are strings (with `enum`, `maxLength`),
/// booleans, or integers/numbers (with `minimum`, `maximum`), and no other
/// properties.
pub fn validate_options(schema: &Value, options: &Map<String, Value>) -> Result<(), String> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (name, value) in options {
        let Some(property) = properties.get(name) else {
            return Err(format!("unknown option {name}"));
        };
        let kind = property.get("type").and_then(Value::as_str).unwrap_or("");
        let valid = match kind {
            "string" => value.as_str().is_some_and(|text| {
                let within = property
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .is_none_or(|max| text.chars().count() as u64 <= max);
                let allowed = property
                    .get("enum")
                    .and_then(Value::as_array)
                    .is_none_or(|values| values.iter().any(|allowed| allowed == value));
                within && allowed && !text.chars().any(char::is_control)
            }),
            "boolean" => value.is_boolean(),
            "integer" | "number" => {
                let number = if kind == "integer" {
                    value.as_i64().map(|number| number as f64)
                } else {
                    value.as_f64()
                };
                number.is_some_and(|number| {
                    property
                        .get("minimum")
                        .and_then(Value::as_f64)
                        .is_none_or(|minimum| number >= minimum)
                        && property
                            .get("maximum")
                            .and_then(Value::as_f64)
                            .is_none_or(|maximum| number <= maximum)
                })
            }
            _ => false,
        };
        if !valid {
            return Err(format!("option {name} is invalid"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ModelEntry, ModelOverrides, VersionRange, validate_options};

    #[test]
    fn version_ranges_accept_only_versions_inside_them() {
        let range = VersionRange {
            minimum: "0.146.0".to_owned(),
            below: Some("1.0.0".to_owned()),
        };
        assert!(range.contains("0.146.0"));
        assert!(range.contains("0.150.2"));
        assert!(range.contains("0.146.1-alpha.1"));
        assert!(!range.contains("0.145.9"));
        assert!(!range.contains("1.0.0"));
        assert!(!range.contains("latest"));
    }

    #[test]
    fn overrides_win_over_provider_data_and_are_bounded() {
        let model = ModelEntry {
            id: "m".to_owned(),
            display_name: "Provider".to_owned(),
            ..ModelEntry::default()
        };
        let overrides = ModelOverrides {
            display_name: Some("Mine".to_owned()),
            context_window: Some(200_000),
            ..ModelOverrides::default()
        };
        assert!(overrides.validate().is_ok());
        let merged = overrides.apply(model);
        assert_eq!(merged.display_name, "Mine");
        assert_eq!(merged.context_window, Some(200_000));
        assert!(
            ModelOverrides {
                display_name: Some("bad\nname".to_owned()),
                ..ModelOverrides::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn options_follow_the_manifest_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "enum": ["auto", "none"]},
                "webSearch": {"type": "boolean"},
                "budget": {"type": "integer", "minimum": 1, "maximum": 10}
            },
            "additionalProperties": false
        });
        let options = |value: serde_json::Value| value.as_object().cloned().expect("object");
        assert!(
            validate_options(&schema, &options(json!({"summary": "auto", "budget": 3}))).is_ok()
        );
        assert!(validate_options(&schema, &options(json!({"summary": "loud"}))).is_err());
        assert!(validate_options(&schema, &options(json!({"budget": 11}))).is_err());
        assert!(validate_options(&schema, &options(json!({"other": true}))).is_err());
    }
}
