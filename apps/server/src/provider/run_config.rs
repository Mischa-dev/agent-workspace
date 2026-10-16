//! A run's settings (H4): which harness, model, reasoning level and speed,
//! how many attempts, the sandbox and approval policy, and any adapter
//! options. Resolved once when the run is created, from the launch overrides
//! over the account's defaults over the catalog and manifest defaults, and
//! stored on `agent_runs.config`, which the database freezes from then on.
//! Workspace and project settings (S2) slot in between the launch overrides
//! and the account defaults when they arrive.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::manifest::{AdapterManifest, ModelEntry, validate_options};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

/// Where a resolved value came from, for the run card and later for S2's
/// "where did this come from".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Launch,
    Account,
    Catalog,
    Adapter,
}

#[derive(Debug, Eq, PartialEq)]
pub struct InvalidRunConfig(pub &'static str);

impl RunConfig {
    pub fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    fn identifier(value: &str) -> bool {
        (1..=200).contains(&value.len())
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
            })
    }

    /// Shape checks that don't need the catalog: identifiers stay short and
    /// printable.
    pub fn check_shape(&self) -> Result<(), InvalidRunConfig> {
        for (value, field) in [
            (&self.harness, "harness"),
            (&self.model, "model"),
            (&self.reasoning, "reasoning"),
            (&self.speed, "speed"),
            (&self.sandbox, "sandbox"),
            (&self.approval_policy, "approvalPolicy"),
        ] {
            if value
                .as_deref()
                .is_some_and(|value| !Self::identifier(value))
            {
                return Err(InvalidRunConfig(field));
            }
        }
        if self
            .attempts
            .is_some_and(|attempts| !(1..=8).contains(&attempts))
        {
            return Err(InvalidRunConfig("attempts"));
        }
        if serde_json::to_vec(&self.options).map_or(true, |bytes| bytes.len() > 8192) {
            return Err(InvalidRunConfig("options"));
        }
        Ok(())
    }

    /// Resolves the launch overrides (`self`) over the account defaults for
    /// one adapter, checking every value against its manifest and catalog.
    /// `catalog` is the workspace's whole catalog; hidden models can't be
    /// picked. An empty catalog means the adapter's own default model is
    /// used and a model named at launch is refused until the catalog lists
    /// it; a catalog with every model hidden refuses to run at all. An
    /// account default model that is no longer pickable falls back to the
    /// catalog default rather than blocking the account's runs.
    pub fn resolve(
        &self,
        account_defaults: &RunConfig,
        manifest: &AdapterManifest,
        catalog: &[ModelEntry],
    ) -> Result<RunConfig, InvalidRunConfig> {
        self.check_shape()?;
        account_defaults.check_shape()?;
        if let Some(harness) = &self.harness
            && harness != &manifest.kind
        {
            return Err(InvalidRunConfig("harness"));
        }
        let launch_model = self.model.clone();
        let pickable = |id: &str| catalog.iter().find(|model| model.id == id && !model.hidden);
        if let Some(id) = &launch_model
            && pickable(id).is_none()
        {
            return Err(InvalidRunConfig("model"));
        }
        let model = match launch_model
            .as_deref()
            .or(account_defaults.model.as_deref())
            .and_then(pickable)
        {
            Some(model) => Some(model),
            None => {
                let visible = || catalog.iter().filter(|model| !model.hidden);
                let fallback = visible()
                    .find(|model| model.is_default)
                    .or_else(|| visible().next());
                if fallback.is_none() && !catalog.is_empty() {
                    // Every model is hidden: running the adapter's own
                    // default would bypass what the admins hid.
                    return Err(InvalidRunConfig("model"));
                }
                fallback
            }
        };
        let model_id = model.map(|model| model.id.clone());
        // Account defaults for reasoning and speed only apply when the model
        // they were chosen for is still the one in use, and only while that
        // model still offers them; otherwise the model's own default is used.
        // Values named at launch are checked strictly below.
        let defaults_apply =
            launch_model.is_none() || launch_model.as_deref() == account_defaults.model.as_deref();
        let account_default = |value: &Option<String>, offered: &dyn Fn(&str) -> bool| {
            value
                .clone()
                .filter(|value| defaults_apply && offered(value))
        };
        let reasoning = self
            .reasoning
            .clone()
            .or_else(|| {
                account_default(&account_defaults.reasoning, &|level| {
                    model.is_none_or(|model| {
                        model.reasoning_levels.is_empty()
                            || model
                                .reasoning_levels
                                .iter()
                                .any(|offered| offered == level)
                    })
                })
            })
            .or_else(|| model.and_then(|model| model.default_reasoning.clone()));
        if let (Some(level), Some(model)) = (&reasoning, model)
            && !model.reasoning_levels.is_empty()
            && !model.reasoning_levels.contains(level)
        {
            return Err(InvalidRunConfig("reasoning"));
        }
        let speed = self
            .speed
            .clone()
            .or_else(|| {
                account_default(&account_defaults.speed, &|speed| {
                    model.is_none_or(|model| model.speeds.iter().any(|option| option.id == speed))
                })
            })
            .or_else(|| model.and_then(|model| model.default_speed.clone()));
        if let (Some(speed), Some(model)) = (&speed, model)
            && !model.speeds.iter().any(|option| &option.id == speed)
        {
            return Err(InvalidRunConfig("speed"));
        }
        let attempts = self.attempts.or(account_defaults.attempts).unwrap_or(1);
        if attempts > manifest.run_options.max_attempts.max(1) {
            return Err(InvalidRunConfig("attempts"));
        }
        let sandbox = self
            .sandbox
            .clone()
            .or_else(|| account_defaults.sandbox.clone())
            .or_else(|| manifest.run_options.sandbox_modes.first().cloned());
        if sandbox
            .as_ref()
            .is_some_and(|mode| !manifest.run_options.sandbox_modes.contains(mode))
        {
            return Err(InvalidRunConfig("sandbox"));
        }
        let approval_policy = self
            .approval_policy
            .clone()
            .or_else(|| account_defaults.approval_policy.clone())
            .or_else(|| manifest.run_options.approval_policies.first().cloned());
        if approval_policy
            .as_ref()
            .is_some_and(|policy| !manifest.run_options.approval_policies.contains(policy))
        {
            return Err(InvalidRunConfig("approvalPolicy"));
        }
        let mut options = account_defaults.options.clone();
        options.extend(self.options.clone());
        validate_options(&manifest.options_schema, &options)
            .map_err(|_| InvalidRunConfig("options"))?;
        Ok(RunConfig {
            harness: Some(manifest.kind.clone()),
            model: model_id,
            reasoning,
            speed,
            attempts: Some(attempts),
            sandbox,
            approval_policy,
            options,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InvalidRunConfig, RunConfig};
    use crate::provider::{
        demo::demo_manifest,
        manifest::{ModelEntry, SpeedOption},
    };

    fn catalog() -> Vec<ModelEntry> {
        vec![
            ModelEntry {
                id: "fast-model".to_owned(),
                display_name: "Fast".to_owned(),
                reasoning_levels: vec!["low".to_owned(), "high".to_owned()],
                default_reasoning: Some("low".to_owned()),
                speeds: vec![
                    SpeedOption {
                        id: "standard".to_owned(),
                        name: "Standard".to_owned(),
                    },
                    SpeedOption {
                        id: "priority".to_owned(),
                        name: "Priority".to_owned(),
                    },
                ],
                default_speed: Some("standard".to_owned()),
                ..ModelEntry::default()
            },
            ModelEntry {
                id: "deep-model".to_owned(),
                display_name: "Deep".to_owned(),
                reasoning_levels: vec!["medium".to_owned(), "xhigh".to_owned()],
                default_reasoning: Some("medium".to_owned()),
                is_default: true,
                ..ModelEntry::default()
            },
        ]
    }

    #[test]
    fn launch_overrides_beat_account_defaults_which_beat_the_catalog() {
        let manifest = demo_manifest();
        let resolved = RunConfig::default()
            .resolve(&RunConfig::default(), &manifest, &catalog())
            .expect("catalog defaults");
        assert_eq!(resolved.model.as_deref(), Some("deep-model"));
        assert_eq!(resolved.reasoning.as_deref(), Some("medium"));
        assert_eq!(resolved.harness.as_deref(), Some(manifest.kind.as_str()));

        let account = RunConfig {
            model: Some("fast-model".to_owned()),
            reasoning: Some("high".to_owned()),
            ..RunConfig::default()
        };
        let resolved = RunConfig::default()
            .resolve(&account, &manifest, &catalog())
            .expect("account defaults");
        assert_eq!(resolved.model.as_deref(), Some("fast-model"));
        assert_eq!(resolved.reasoning.as_deref(), Some("high"));
        assert_eq!(resolved.speed.as_deref(), Some("standard"));

        let launch = RunConfig {
            speed: Some("priority".to_owned()),
            reasoning: Some("low".to_owned()),
            ..RunConfig::default()
        };
        let resolved = launch
            .resolve(&account, &manifest, &catalog())
            .expect("launch");
        assert_eq!(resolved.reasoning.as_deref(), Some("low"));
        assert_eq!(resolved.speed.as_deref(), Some("priority"));

        // Picking another model at launch drops the account's reasoning
        // choice made for a different model.
        let switched = RunConfig {
            model: Some("deep-model".to_owned()),
            ..RunConfig::default()
        }
        .resolve(&account, &manifest, &catalog())
        .expect("switched model");
        assert_eq!(switched.reasoning.as_deref(), Some("medium"));
    }

    #[test]
    fn values_outside_the_manifest_or_catalog_are_refused() {
        let manifest = demo_manifest();
        let refuse = |config: RunConfig| {
            config
                .resolve(&RunConfig::default(), &manifest, &catalog())
                .expect_err("invalid run config")
        };
        assert_eq!(
            refuse(RunConfig {
                model: Some("missing".to_owned()),
                ..RunConfig::default()
            }),
            InvalidRunConfig("model")
        );
        assert_eq!(
            refuse(RunConfig {
                reasoning: Some("xhigh".to_owned()),
                model: Some("fast-model".to_owned()),
                ..RunConfig::default()
            }),
            InvalidRunConfig("reasoning")
        );
        assert_eq!(
            refuse(RunConfig {
                harness: Some("other".to_owned()),
                ..RunConfig::default()
            }),
            InvalidRunConfig("harness")
        );
        assert_eq!(
            refuse(RunConfig {
                sandbox: Some("everything".to_owned()),
                ..RunConfig::default()
            }),
            InvalidRunConfig("sandbox")
        );
        assert_eq!(
            refuse(RunConfig {
                options: json!({"unknown": 1}).as_object().cloned().expect("object"),
                ..RunConfig::default()
            }),
            InvalidRunConfig("options")
        );
        // Before the catalog lists any model, only the adapter's default runs.
        let named = RunConfig {
            model: Some("fast-model".to_owned()),
            ..RunConfig::default()
        };
        assert_eq!(
            named.resolve(&RunConfig::default(), &manifest, &[]),
            Err(InvalidRunConfig("model"))
        );
        assert!(
            RunConfig::default()
                .resolve(&named, &manifest, &[])
                .is_ok_and(|config| config.model.is_none()),
            "an account default the catalog doesn't list falls back"
        );
        assert!(
            RunConfig::default()
                .resolve(&RunConfig::default(), &manifest, &[])
                .is_ok_and(|config| config.model.is_none())
        );
        assert!(
            serde_json::from_value::<RunConfig>(json!({"model": "x", "secret": "y"})).is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn account_defaults_the_model_no_longer_offers_fall_back_to_its_own() {
        let manifest = demo_manifest();
        // Chosen while fast-model was the default; deep-model offers neither.
        let account = RunConfig {
            reasoning: Some("high".to_owned()),
            speed: Some("priority".to_owned()),
            ..RunConfig::default()
        };
        let resolved = RunConfig::default()
            .resolve(&account, &manifest, &catalog())
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(resolved.model.as_deref(), Some("deep-model"));
        assert_eq!(resolved.reasoning.as_deref(), Some("medium"));
        assert_eq!(resolved.speed, None);
        // Named at launch, the same values are still refused.
        let launch = RunConfig {
            reasoning: Some("high".to_owned()),
            ..RunConfig::default()
        };
        assert_eq!(
            launch.resolve(&account, &manifest, &catalog()).err(),
            Some(InvalidRunConfig("reasoning"))
        );
    }

    #[test]
    fn hidden_models_are_never_run_and_stale_account_models_fall_back() {
        let manifest = demo_manifest();
        let mut hidden = catalog();
        hidden[1].hidden = true;
        let account = RunConfig {
            model: Some("deep-model".to_owned()),
            ..RunConfig::default()
        };
        let resolved = RunConfig::default()
            .resolve(&account, &manifest, &hidden)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(resolved.model.as_deref(), Some("fast-model"));
        assert_eq!(
            account.resolve(&RunConfig::default(), &manifest, &hidden),
            Err(InvalidRunConfig("model")),
            "named at launch, a hidden model is refused"
        );
        for model in &mut hidden {
            model.hidden = true;
        }
        assert_eq!(
            RunConfig::default().resolve(&RunConfig::default(), &manifest, &hidden),
            Err(InvalidRunConfig("model")),
            "with every model hidden nothing runs"
        );
    }
}
