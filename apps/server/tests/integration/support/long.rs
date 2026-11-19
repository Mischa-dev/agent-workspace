//! K2.21: long calls answered `started` and finished by a job, as a second
//! server process on the same database would finish them.

use serde_json::Value;

/// The state a second server process on the same database would have: the
/// pool is what serves; the URL only fills the configuration.
pub(crate) fn second_process(
    pool: &sqlx::PgPool,
) -> (axum::Router, agent_workspace_server::AppState) {
    let url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let config = agent_workspace_server::config::AppConfig::from_values(
        &std::collections::BTreeMap::from([("AGENT_WORKSPACE_DATABASE_URL".to_owned(), url)]),
    )
    .expect("configuration");
    let supervisors =
        agent_workspace_server::provider::SupervisorRegistry::new(&config).expect("supervisors");
    let state = agent_workspace_server::AppState {
        config: std::sync::Arc::new(config),
        pool: pool.clone(),
        rate_limits: agent_workspace_server::rate_limit::RateLimiters::default(),
        realtime: agent_workspace_server::realtime::RealtimeHub::new(32),
        supervisors,
    };
    (agent_workspace_server::router(state.clone()), state)
}

pub(crate) async fn run_jobs(state: &agent_workspace_server::AppState) -> usize {
    let registry = agent_workspace_server::jobs::default_registry();
    let options = agent_workspace_server::jobs::WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(60),
        worker_id: "k2-21-test".to_owned(),
    };
    agent_workspace_server::jobs::run_due(state, &registry, &options)
        .await
        .expect("run jobs")
}

/// The answer a long call ends with: `answer` itself unless it was
/// `started`, else the job's answer once the jobs due have run.
pub(crate) async fn finished(pool: &sqlx::PgPool, answer: Value) -> Value {
    if answer["outcome"] != "started" {
        return answer;
    }
    let job = answer["value"]["jobId"]
        .as_str()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .unwrap_or_else(|| panic!("no job id: {answer}"));
    let (_, state) = second_process(pool);
    run_jobs(&state).await;
    sqlx::query_scalar::<_, Option<Value>>("SELECT result FROM operation_tasks WHERE id = $1")
        .bind(job)
        .fetch_one(pool)
        .await
        .expect("operation task")
        .unwrap_or_else(|| panic!("the job didn't finish: {answer}"))
}
