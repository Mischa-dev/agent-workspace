//! The contract every harness adapter keeps (H2), run against each adapter
//! the server ships. A new adapter adds one test that calls
//! `adapter_contract`.

use std::path::Path;

use super::{
    adapter::{AccountHandle, HarnessAdapter, HarnessError},
    run_config::RunConfig,
};
use crate::git_runtime::is_run_branch_prefix;

pub(super) async fn adapter_contract(
    adapter: &dyn HarnessAdapter,
    account: AccountHandle,
    worktree: &Path,
) {
    let manifest = adapter.manifest();
    assert!(
        manifest
            .kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            && manifest.kind.len() <= 64,
        "adapter kind fits the database shape"
    );
    assert!(
        is_run_branch_prefix(&manifest.branch_prefix),
        "run branches look like <harness>/run-"
    );
    assert!(manifest.versions.contains(&manifest.versions.minimum));
    assert!(
        (1..=manifest.concurrency.max_per_account)
            .contains(&manifest.concurrency.default_per_account)
            && manifest.concurrency.max_per_account <= 16,
        "concurrency defaults sit inside the database limits"
    );
    assert!(manifest.options_schema.is_object());
    let mut ids = manifest
        .default_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        manifest.default_models.len(),
        "model ids are unique"
    );
    let defaults = RunConfig::default()
        .resolve(&RunConfig::default(), manifest, &manifest.default_models)
        .expect("the manifest's own defaults resolve");
    assert_eq!(defaults.harness.as_deref(), Some(manifest.kind.as_str()));

    // Account pages work.
    {
        let mut control = adapter
            .acquire_control(account.clone())
            .await
            .expect("control session");
        assert_eq!(control.account_id(), account.id);
        control.read_account().await.expect("read account");
        if manifest.features.model_listing {
            control.list_models().await.expect("list models");
        }
    }

    // Runs: up to the account's limit at once, each with its own thread and
    // turn, carrying the run's settings.
    let limit = usize::from(account.max_concurrent_runs);
    let mut runs = Vec::new();
    for index in 0..limit {
        let mut run = adapter
            .acquire_run(account.clone())
            .await
            .unwrap_or_else(|error| panic!("run {index} within the account limit: {error:?}"));
        let thread = run
            .start_thread(worktree, &defaults)
            .await
            .expect("start thread");
        assert!(!thread.is_empty());
        let turn = run
            .start_turn(&thread, worktree, "contract prompt", &defaults)
            .await
            .expect("start turn");
        assert!(!turn.is_empty());
        runs.push(run);
    }
    assert!(
        matches!(
            adapter.acquire_run(account.clone()).await,
            Err(HarnessError::AccountCapacity | HarnessError::HostCapacity)
        ),
        "one run past the account limit is refused, not queued inside the adapter"
    );

    // An account page while every run slot is busy still works (F8), and
    // never takes a run's place.
    {
        let mut control = adapter
            .acquire_control(account.clone())
            .await
            .expect("control session while runs are busy");
        control.read_account().await.expect("read account");
    }
    drop(runs.pop());
    let mut again = adapter
        .acquire_run(account.clone())
        .await
        .expect("a freed slot can run again");
    again
        .start_thread(worktree, &defaults)
        .await
        .expect("thread");
    drop(again);
    drop(runs);

    let health = adapter.health(account.id).await;
    assert!(health.is_some_and(|health| !health.circuit_open));
    adapter.shutdown_account(account.id).await;
}
