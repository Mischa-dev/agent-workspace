//! The run configuration's resolution, against the demo harness.

use serde_json::json;

use crate::harness::run_config::{InvalidRunConfig, RunConfig};
use crate::harness::{
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
    // Reasoning and speed can't be checked without a model, so a launch
    // may not name them and account choices wait for the catalog.
    for (launch, field) in [
        (
            RunConfig {
                reasoning: Some("typo".to_owned()),
                ..RunConfig::default()
            },
            "reasoning",
        ),
        (
            RunConfig {
                speed: Some("typo".to_owned()),
                ..RunConfig::default()
            },
            "speed",
        ),
    ] {
        assert_eq!(
            launch.resolve(&RunConfig::default(), &manifest, &[]),
            Err(InvalidRunConfig(field))
        );
        assert!(
            RunConfig::default()
                .resolve(&launch, &manifest, &[])
                .is_ok_and(|config| config.reasoning.is_none() && config.speed.is_none()),
            "account {field} is not used before the catalog lists a model"
        );
    }
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

#[test]
fn stale_settings_fall_back_one_at_a_time_but_isolation_never_loosens() {
    let mut manifest = demo_manifest();
    manifest.run_options.sandbox_modes = vec!["workspace_write".to_owned(), "read_only".to_owned()];
    let predecessor = RunConfig {
        model: Some("gone-model".to_owned()),
        sandbox: Some("read_only".to_owned()),
        ..RunConfig::default()
    };
    let resolved = predecessor
        .resolve_keeping_what_still_fits(&RunConfig::default(), &manifest, &catalog())
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(resolved.model.as_deref(), Some("deep-model"));
    assert_eq!(resolved.sandbox, predecessor.sandbox, "sandbox kept");
    let loosened = RunConfig {
        sandbox: Some("no-longer-offered".to_owned()),
        ..RunConfig::default()
    };
    assert_eq!(
        loosened.resolve_keeping_what_still_fits(&RunConfig::default(), &manifest, &catalog()),
        Err(InvalidRunConfig("sandbox"))
    );
}
