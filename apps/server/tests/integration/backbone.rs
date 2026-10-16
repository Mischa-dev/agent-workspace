//! W1 backbone: the job queue, encrypted secrets and inheriting settings.
//! The costly bugs here are silent: a job that runs twice or never, a key
//! that shows up in a dump, a response or a log, and a setting that comes
//! from the wrong place (or another workspace).

use std::{
    collections::BTreeMap,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    error::AppError,
    jobs::{self, Enqueued, JobError, JobRegistry, NewJob, WorkerOptions},
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    secrets::{self, FileKeyring, KeyEncryption, NewSecret, SecretOwner, SecretValue},
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use super::agent_interface::{
    Browser, PASSWORD, connect_app, create_agent, initialize, login, schema_pool, send, tool,
};

const PLAINTEXT_KEY: &str = "sk-ant-api03-ZZtopSecretBackboneValue0123456789";

struct Backbone {
    state: AppState,
    app: axum::Router,
    browser: Browser,
    organization_id: Uuid,
    pool: PgPool,
    _keys: tempfile::TempDir,
}

async fn backbone(prefix: &str) -> Option<Backbone> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    let owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Backbone".to_owned()),
            organization_slug: Some(format!("backbone-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let keys = tempfile::tempdir().expect("key directory");
    let keyring_path = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring_path, &FileKeyring::generate()).expect("keyring");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("config");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    };
    let app = router(state.clone());
    let browser = login(&app).await;
    Some(Backbone {
        state,
        app,
        browser,
        organization_id: owner.organization_id,
        pool,
        _keys: keys,
    })
}

fn options() -> WorkerOptions {
    WorkerOptions {
        concurrency: 2,
        poll_interval: Duration::from_millis(20),
        lease: Duration::from_secs(30),
        worker_id: "test".to_owned(),
    }
}

async fn job_state(pool: &PgPool, id: Uuid) -> (String, i32, Option<String>) {
    sqlx::query_as::<_, (String, i32, Option<String>)>(
        "SELECT state, attempts, last_error FROM background_jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("job")
}

fn created(enqueued: Enqueued) -> Uuid {
    match enqueued {
        Enqueued::Created(id) => id,
        Enqueued::Duplicate => panic!("expected a new job"),
    }
}

#[tokio::test]
async fn a_job_scheduled_for_later_runs_once_survives_a_restart_and_retries() {
    let Some(setup) = backbone("backbone_jobs").await else {
        return;
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let registry = |runs: Arc<AtomicUsize>| {
        let mut registry = JobRegistry::new();
        registry.register("test.count", move |_| {
            let runs = runs.clone();
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        registry
    };

    // Scheduled a little in the future, with a unique key held once.
    let id = created(
        jobs::enqueue(
            &setup.pool,
            NewJob::new("test.count")
                .workspace(setup.organization_id)
                .delay(Duration::from_millis(400))
                .unique("count:one"),
        )
        .await
        .expect("enqueue"),
    );
    assert_eq!(
        jobs::enqueue(&setup.pool, NewJob::new("test.count").unique("count:one"))
            .await
            .expect("enqueue twice"),
        Enqueued::Duplicate
    );

    // Not due yet: nothing runs.
    let first = registry(runs.clone());
    assert_eq!(
        jobs::run_due(&setup.state, &first, &options())
            .await
            .expect("run"),
        0
    );
    // "Restart": a new registry and worker see the same row once it's due.
    drop(first);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let second = registry(runs.clone());
    assert_eq!(
        jobs::run_due(&setup.state, &second, &options())
            .await
            .expect("run"),
        1
    );
    assert_eq!(
        jobs::run_due(&setup.state, &second, &options())
            .await
            .expect("run again"),
        0
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(job_state(&setup.pool, id).await.0, "succeeded");
    // Finished, the key is free again.
    assert!(matches!(
        jobs::enqueue(&setup.pool, NewJob::new("test.count").unique("count:one"))
            .await
            .expect("enqueue after"),
        Enqueued::Created(_)
    ));

    // A failing job is retried with backoff, then dies after its attempts.
    let mut failing = JobRegistry::new();
    failing.register("test.fail", |_| async {
        Err(JobError::retry("upstream said 503"))
    });
    let failed = created(
        jobs::enqueue(&setup.pool, NewJob::new("test.fail").max_attempts(2))
            .await
            .expect("enqueue failing"),
    );
    jobs::run_due(&setup.state, &failing, &options())
        .await
        .expect("first attempt");
    let (state, attempts, error) = job_state(&setup.pool, failed).await;
    assert_eq!(
        (state.as_str(), attempts, error.as_deref()),
        ("queued", 1, Some("upstream said 503"))
    );
    let retry_at = sqlx::query_scalar::<_, bool>(
        "SELECT run_at > now() + interval '5 seconds' FROM background_jobs WHERE id = $1",
    )
    .bind(failed)
    .fetch_one(&setup.pool)
    .await
    .expect("retry time");
    assert!(retry_at, "a retry waits for its backoff");
    sqlx::query("UPDATE background_jobs SET run_at = now() WHERE id = $1")
        .bind(failed)
        .execute(&setup.pool)
        .await
        .expect("make due");
    jobs::run_due(&setup.state, &failing, &options())
        .await
        .expect("second attempt");
    assert_eq!(job_state(&setup.pool, failed).await.0, "dead");

    // A permanent error goes dead at once; a worker never claims kinds it
    // has no handler for.
    let mut permanent = JobRegistry::new();
    permanent.register("test.bad_input", |_| async {
        Err(JobError::permanent("the payload names a deleted project"))
    });
    let bad = created(
        jobs::enqueue(&setup.pool, NewJob::new("test.bad_input"))
            .await
            .expect("enqueue bad"),
    );
    let unknown = created(
        jobs::enqueue(&setup.pool, NewJob::new("test.from_a_newer_server"))
            .await
            .expect("enqueue unknown"),
    );
    jobs::run_due(&setup.state, &permanent, &options())
        .await
        .expect("permanent");
    assert_eq!(
        job_state(&setup.pool, bad).await,
        (
            "dead".to_owned(),
            1,
            Some("the payload names a deleted project".to_owned())
        )
    );
    assert_eq!(job_state(&setup.pool, unknown).await.0, "queued");
}

#[tokio::test]
async fn a_job_whose_worker_died_runs_again_and_recurring_jobs_queue_their_next_run() {
    let Some(setup) = backbone("backbone_leases").await else {
        return;
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let mut registry = JobRegistry::new();
    let counted = runs.clone();
    registry.register("test.lease", move |_| {
        let runs = counted.clone();
        async move {
            runs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    let id = created(
        jobs::enqueue(&setup.pool, NewJob::new("test.lease"))
            .await
            .expect("enqueue"),
    );
    // A worker claimed it and then vanished: running, with a lapsed lease.
    sqlx::query(
        r#"
        UPDATE background_jobs
        SET state = 'running', attempts = 1, lease_token = gen_random_uuid(),
            lease_owner = 'gone', lease_expires_at = now() - interval '1 second'
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(&setup.pool)
    .await
    .expect("lapse lease");
    assert_eq!(
        jobs::run_due(&setup.state, &registry, &options())
            .await
            .expect("reclaim"),
        1
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(job_state(&setup.pool, id).await.0, "succeeded");
    assert_eq!(job_state(&setup.pool, id).await.1, 2);

    // The real worker loop picks up recurring system jobs and stops cleanly.
    let mut recurring = JobRegistry::new();
    let ticks = Arc::new(AtomicUsize::new(0));
    let counted = ticks.clone();
    recurring.register("test.tick", move |_| {
        let ticks = counted.clone();
        async move {
            ticks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    recurring.every("test.tick", Duration::from_secs(3600));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(jobs::run_workers(
        setup.state.clone(),
        Arc::new(recurring),
        options(),
        shutdown,
    ));
    for _ in 0..100 {
        if ticks.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stop.send(true).expect("stop");
    worker.await.expect("join").expect("workers");
    assert_eq!(ticks.load(Ordering::SeqCst), 1);
    let next = sqlx::query_scalar::<_, bool>(
        "SELECT run_at > now() + interval '50 minutes' FROM background_jobs WHERE kind = 'test.tick' AND state = 'queued'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("next tick queued");
    assert!(next, "the next run waits for the interval");
}

#[tokio::test]
async fn a_handler_that_crashes_after_its_outside_call_retries_with_the_same_key() {
    let Some(setup) = backbone("backbone_idempotency").await else {
        return;
    };
    // The "remote system": the keys it has seen, and the effects it applied.
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let applied = Arc::new(AtomicUsize::new(0));
    let mut registry = JobRegistry::new();
    let (remote, effects) = (seen.clone(), applied.clone());
    registry.register("test.push", move |context| {
        let (remote, effects) = (remote.clone(), effects.clone());
        async move {
            let key = context.job.idempotency_key();
            let mut keys = remote.lock().expect("remote");
            if !keys.contains(&key) {
                effects.fetch_add(1, Ordering::SeqCst);
            }
            keys.push(key);
            if context.job.attempts == 1 {
                // The call landed, then the worker failed before recording it.
                return Err(JobError::retry("lost the response"));
            }
            Ok(())
        }
    });
    let id = created(
        jobs::enqueue(&setup.pool, NewJob::new("test.push"))
            .await
            .expect("enqueue"),
    );
    jobs::run_due(&setup.state, &registry, &options())
        .await
        .expect("first attempt");
    sqlx::query("UPDATE background_jobs SET run_at = now() WHERE id = $1")
        .bind(id)
        .execute(&setup.pool)
        .await
        .expect("make due");
    jobs::run_due(&setup.state, &registry, &options())
        .await
        .expect("retry");
    assert_eq!(job_state(&setup.pool, id).await.0, "succeeded");
    let keys = seen.lock().expect("remote").clone();
    assert_eq!(keys.len(), 2, "both attempts reached the remote");
    assert_eq!(keys[0], keys[1], "the retry reused the first attempt's key");
    assert_eq!(keys[0], format!("aw-job-{id}"));
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "the effect happened once"
    );
}

/// Captures every log line written while it's the default subscriber.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Every row in the test's schema that contains `needle`, as text or as
/// the hex PostgreSQL uses for bytes: what a database dump would show.
async fn rows_containing(pool: &PgPool, needle: &str) -> Vec<String> {
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
    )
    .fetch_all(pool)
    .await
    .expect("tables");
    let hex = needle
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut found = Vec::new();
    for table in tables {
        let count = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM \"{table}\" row WHERE row_to_json(row)::text LIKE '%' || $1 || '%' OR row_to_json(row)::text LIKE '%' || $2 || '%'"
        ))
        .bind(needle)
        .bind(&hex)
        .fetch_one(pool)
        .await
        .expect("scan table");
        if count > 0 {
            found.push(table);
        }
    }
    found
}

#[tokio::test]
async fn a_saved_key_never_shows_in_a_dump_a_response_a_log_or_a_run_environment() {
    let Some(setup) = backbone("backbone_secrets").await else {
        return;
    };
    let logs = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let organization = setup.organization_id;
    let mut responses = Vec::new();

    // Saved through the API: the answer names it and shows the last four.
    let (status, saved) = send(
        &setup.app,
        setup.browser.request(
            "PUT",
            &format!("/api/v1/organizations/{organization}/secrets/anthropic_api_key"),
            None,
            json!({"value": PLAINTEXT_KEY, "description": "AI and models"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["name"], "anthropic_api_key");
    assert_eq!(saved["lastFour"], "6789");
    assert_eq!(saved["createdByName"], "Owner");
    responses.push(saved.to_string());
    let (status, listed) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/organizations/{organization}/secrets"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["configured"], true);
    assert_eq!(listed["secrets"].as_array().map(Vec::len), Some(1));
    responses.push(listed.to_string());

    // The adapter call that needs it gets it back.
    let keys = secrets::keyring(&setup.state.config).expect("keyring");
    let revealed = secrets::reveal_named(
        &setup.pool,
        keys.as_ref(),
        SecretOwner::Workspace {
            organization_id: organization,
        },
        "anthropic_api_key",
        "test.adapter_call",
    )
    .await
    .expect("reveal")
    .expect("saved key");
    assert_eq!(revealed.expose(), PLAINTEXT_KEY);
    assert!(!format!("{revealed:?}").contains("ZZtop"));

    // Credentials come back byte for byte, edge whitespace included.
    let padded = format!("  {PLAINTEXT_KEY}\n\t");
    let (status, saved) = send(
        &setup.app,
        setup.browser.request(
            "PUT",
            &format!("/api/v1/organizations/{organization}/secrets/padded_key"),
            None,
            json!({"value": padded}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let revealed_padded = secrets::reveal_named(
        &setup.pool,
        keys.as_ref(),
        SecretOwner::Workspace {
            organization_id: organization,
        },
        "padded_key",
        "test.adapter_call",
    )
    .await
    .expect("reveal")
    .expect("saved padded key");
    assert_eq!(revealed_padded.expose(), padded);
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "DELETE",
            &format!("/api/v1/organizations/{organization}/secrets/padded_key"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{status}");

    // A wakeup webhook's signing secret is shown once, then lives encrypted.
    let space = send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (agent, _) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let connection = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(&setup.pool)
    .await
    .expect("connection");
    let (status, hooked) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/external-agents/{connection}/webhook"),
            None,
            json!({"url": "https://hooks.example.test/wake"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hooked}");
    let webhook_secret = hooked["webhookSecret"]
        .as_str()
        .expect("signing secret shown once")
        .to_owned();
    let (plaintext, secret_id) = sqlx::query_as::<_, (Option<String>, Option<Uuid>)>(
        "SELECT webhook_secret, webhook_secret_id FROM external_agent_connections WHERE id = $1",
    )
    .bind(connection)
    .fetch_one(&setup.pool)
    .await
    .expect("connection secret");
    assert_eq!(plaintext, None);
    let secret_id = secret_id.expect("stored secret");
    assert_eq!(
        secrets::reveal(&setup.pool, keys.as_ref(), secret_id, "test.sign")
            .await
            .expect("webhook secret")
            .expose(),
        webhook_secret
    );
    // Server-made secrets aren't listed with the ones people add.
    let (_, project_secrets) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{space}/secrets"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(project_secrets["secrets"], json!([]), "{project_secrets}");
    responses.push(project_secrets.to_string());

    // Nothing anyone can read holds either value.
    for needle in [PLAINTEXT_KEY, webhook_secret.as_str()] {
        assert_eq!(
            rows_containing(&setup.pool, needle).await,
            Vec::<String>::new(),
            "a table holds a secret in the clear"
        );
        for response in &responses {
            assert!(!response.contains(needle), "{response}");
        }
        let logged = String::from_utf8(logs.0.lock().expect("logs").clone()).expect("UTF-8 logs");
        assert!(!logged.contains(needle), "a log line holds a secret");
        assert!(
            !logged.contains("ZZtop"),
            "a log line holds part of a secret"
        );
        // Runs start from a cleared environment with only these variables.
        let environment = codex_bridge_spike::safe_environment(std::path::Path::new("/usr/bin"));
        assert!(environment.values().all(|value| !value.contains(needle)));
        assert!(std::env::vars().all(|(_, value)| !value.contains(needle)));
    }

    // Clearing the webhook deletes its secret; deleting a key removes it.
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/external-agents/{connection}/webhook"),
            None,
            json!({"url": null}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        secrets::metadata(&setup.pool, secret_id)
            .await
            .expect("metadata")
            .is_none()
    );
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "DELETE",
            &format!("/api/v1/organizations/{organization}/secrets/anthropic_api_key"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let remaining = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM secrets")
        .fetch_one(&setup.pool)
        .await
        .expect("count");
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn old_plaintext_webhook_secrets_move_into_the_store_and_keys_rotate() {
    let Some(setup) = backbone("backbone_rotation").await else {
        return;
    };
    let space = send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (agent, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    // As a connection looked before the store: plaintext in the row.
    sqlx::query(
        "UPDATE external_agent_connections SET webhook_url = 'https://old.example.test/', webhook_secret = 'whsec_legacyplaintextvalue123' WHERE actor_id = $1",
    )
    .bind(agent)
    .execute(&setup.pool)
    .await
    .expect("legacy row");
    assert_eq!(
        secrets::migrate_plaintext_webhook_secrets(&setup.pool, &setup.state.config)
            .await
            .expect("migrate"),
        1
    );
    assert_eq!(
        rows_containing(&setup.pool, "whsec_legacyplaintextvalue123").await,
        Vec::<String>::new()
    );
    let secret_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT webhook_secret_id FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(&setup.pool)
    .await
    .expect("moved");

    // Rotating the master key re-seals the data key under a new one and
    // drops the old key; the value still opens.
    let before = secrets::keyring(&setup.state.config)
        .expect("keyring")
        .active_key_id()
        .to_owned();
    // The keyring cache keys on modification time; make sure it moves.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let rotated = secrets::rotate_master_key(&setup.pool, &setup.state.config)
        .await
        .expect("rotate");
    assert_eq!(rotated.rewrapped_secrets, 1);
    assert_eq!(rotated.retired_key_ids, vec![before.clone()]);
    let keys = secrets::read_keyring(
        setup
            .state
            .config
            .secrets_key_path
            .as_deref()
            .expect("key path"),
    )
    .expect("rotated keyring");
    assert_eq!(keys.key_ids(), vec![rotated.active_key_id.clone()]);
    assert_eq!(
        secrets::reveal(&setup.pool, &keys, secret_id, "test.sign")
            .await
            .expect("still opens")
            .expose(),
        "whsec_legacyplaintextvalue123"
    );

    // A server that read the keyring before the rotation can't store a
    // secret under the key it just retired: that value could never open.
    let stale = secrets::read_keyring(
        setup
            .state
            .config
            .secrets_key_path
            .as_deref()
            .expect("key path"),
    )
    .expect("keyring");
    let saved = |keys: FileKeyring, name: &'static str| {
        let pool = setup.pool.clone();
        let organization_id = setup.organization_id;
        async move {
            let mut connection = pool.acquire().await.expect("connection");
            secrets::put(
                &mut connection,
                &keys,
                NewSecret {
                    owner: SecretOwner::Workspace { organization_id },
                    name,
                    description: None,
                    value: &SecretValue::new("sk-held-writer-value".to_owned()),
                    actor_id: None,
                    system_managed: false,
                },
            )
            .await
        }
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    secrets::rotate_master_key(&setup.pool, &setup.state.config)
        .await
        .expect("rotate again");
    assert!(matches!(
        saved(stale, "stale_writer").await,
        Err(AppError::Conflict(_, _))
    ));

    // A write in flight while a rotation runs holds it off, and the rotation
    // re-seals what that write stored before it drops the old key.
    let current = secrets::read_keyring(
        setup
            .state
            .config
            .secrets_key_path
            .as_deref()
            .expect("key path"),
    )
    .expect("keyring");
    let mut writer = setup.pool.begin().await.expect("writer");
    let held = secrets::put(
        &mut writer,
        &current,
        NewSecret {
            owner: SecretOwner::Workspace {
                organization_id: setup.organization_id,
            },
            name: "held_writer",
            description: None,
            value: &SecretValue::new("sk-held-writer-value".to_owned()),
            actor_id: None,
            system_managed: false,
        },
    )
    .await
    .expect("held write");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let rotation = tokio::spawn({
        let pool = setup.pool.clone();
        let config = setup.state.config.clone();
        async move { secrets::rotate_master_key(&pool, &config).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!rotation.is_finished(), "the rotation waits for the writer");
    writer.commit().await.expect("commit writer");
    let result = rotation
        .await
        .expect("join")
        .expect("rotate under a writer");
    assert_eq!(
        result.retired_key_ids,
        vec![current.active_key_id().to_owned()]
    );
    let after = secrets::read_keyring(
        setup
            .state
            .config
            .secrets_key_path
            .as_deref()
            .expect("key path"),
    )
    .expect("keyring");
    assert_eq!(
        secrets::reveal(&setup.pool, &after, held.id, "test.held")
            .await
            .expect("the held write still opens")
            .expose(),
        "sk-held-writer-value"
    );

    // Two rotations at once take turns, and every secret still opens.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (first, second) = tokio::join!(
        secrets::rotate_master_key(&setup.pool, &setup.state.config),
        secrets::rotate_master_key(&setup.pool, &setup.state.config),
    );
    first.expect("first rotation");
    second.expect("second rotation");
    let last = secrets::read_keyring(
        setup
            .state
            .config
            .secrets_key_path
            .as_deref()
            .expect("key path"),
    )
    .expect("keyring");
    for id in sqlx::query_scalar::<_, Uuid>("SELECT id FROM secrets")
        .fetch_all(&setup.pool)
        .await
        .expect("ids")
    {
        secrets::reveal(&setup.pool, &last, id, "test.after_rotations")
            .await
            .expect("opens after concurrent rotations");
    }
}

/// A person who owns another workspace, made directly (bootstrap only
/// makes the first one).
async fn outsider_workspace(pool: &PgPool) -> Uuid {
    let organization = Uuid::now_v7();
    let actor = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(organization)
        .bind(format!("elsewhere-{}", Uuid::now_v7().simple()))
        .execute(pool)
        .await
        .expect("organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(organization)
    .bind(actor)
    .execute(pool)
    .await
    .expect("membership");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, 'outsider', 'Outsider')",
    )
    .bind(user)
    .bind(actor)
    .execute(pool)
    .await
    .expect("user");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user)
        .bind(
            agent_workspace_server::auth::password::hash_password(PASSWORD.to_owned())
                .await
                .expect("hash"),
        )
        .execute(pool)
        .await
        .expect("password");
    organization
}

async fn login_named(app: &axum::Router, login_name: &str) -> Browser {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": login_name, "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    Browser {
        cookie: cookies.join("; "),
        csrf,
    }
}

fn setting<'a>(effective: &'a Value, key: &str) -> &'a Value {
    effective["settings"]
        .as_array()
        .and_then(|settings| settings.iter().find(|setting| setting["key"] == key))
        .unwrap_or_else(|| panic!("{key} in {effective}"))
}

#[tokio::test]
async fn a_project_overrides_the_workspace_says_so_and_reset_falls_back() {
    let Some(setup) = backbone("backbone_settings").await else {
        return;
    };
    let app = &setup.app;
    let organization = setup.organization_id;
    let (status, project) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some("settings-project"),
            json!({"organizationId": organization, "key": "WORK", "name": "Work"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project = project["id"].as_str().expect("project").to_owned();
    let effective = |query: String| {
        let request = setup.browser.request(
            "GET",
            &format!("/api/v1/settings/effective?{query}"),
            None,
            Value::Null,
        );
        async move { send(app, request).await }
    };

    // Nothing set: the built-in default.
    let (status, before) = effective(format!("projectId={project}")).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(setting(&before, "run.model")["source"], "default");
    assert_eq!(before["capabilities"]["manageProject"], true);

    // The workspace picks a model; the project overrides it.
    for (scope, model, target) in [
        (
            "workspace",
            "big-model",
            json!({"organizationId": organization}),
        ),
        ("project", "small-model", json!({"projectId": project})),
    ] {
        let mut body = json!({"scope": scope, "value": model, "expectedVersion": 0});
        body.as_object_mut()
            .expect("body")
            .extend(target.as_object().expect("target").clone());
        let (status, written) = send(
            app,
            setup
                .browser
                .request("PUT", "/api/v1/settings/values/run.model", None, body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{written}");
    }
    let (_, now) = effective(format!("projectId={project}")).await;
    let model = setting(&now, "run.model");
    assert_eq!(model["value"], "small-model");
    assert_eq!(model["source"], "project");
    assert_eq!(model["overrides"], "workspace");
    assert_eq!(model["fallbackValue"], "big-model");
    // The workspace alone sees its own value.
    let (_, workspace) = effective(format!("organizationId={organization}")).await;
    assert_eq!(setting(&workspace, "run.model")["source"], "workspace");

    // A stale write is refused instead of overwriting.
    let (status, stale) = send(
        app,
        setup.browser.request(
            "PUT",
            "/api/v1/settings/values/run.model",
            None,
            json!({"scope": "project", "projectId": project, "value": "other", "expectedVersion": 0}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    // Values must match the key's type.
    let (status, _) = send(
        app,
        setup.browser.request(
            "PUT",
            "/api/v1/settings/values/run.reasoning",
            None,
            json!({"scope": "project", "projectId": project, "value": "extreme"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Reset at the project falls back to the workspace.
    let (status, reset) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/settings/values/run.model/reset",
            None,
            json!({"scope": "project", "projectId": project}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["setting"]["value"], "big-model");
    assert_eq!(reset["setting"]["source"], "workspace");

    // A launch override wins for that launch only, and only where allowed.
    let mut connection = setup.pool.acquire().await.expect("connection");
    let context = agent_workspace_server::settings::SettingsContext {
        organization_id: organization,
        project_id: Some(project.parse().expect("project id")),
        actor_id: None,
    };
    let launched = agent_workspace_server::settings::resolve(
        &mut connection,
        context,
        &BTreeMap::from([("run.model".to_owned(), json!("tiny-model"))]),
    )
    .await
    .expect("resolve with override");
    assert_eq!(launched.string("run.model").as_deref(), Some("tiny-model"));
    assert!(
        agent_workspace_server::settings::resolve(
            &mut connection,
            context,
            &BTreeMap::from([("ai.auto_apply_suggestions".to_owned(), json!(true))]),
        )
        .await
        .is_err()
    );
    drop(connection);

    // Agents and apps read settings over MCP but can't change them.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("MCP session");
    let read = tool(
        app,
        &codex,
        &session,
        "settings_get",
        json!({"projectId": "WORK", "keys": ["run.model"]}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["settings"][0]["value"], "big-model");
    assert_eq!(read["value"]["settings"][0]["source"], "workspace");

    // A plain member of the workspace can't change the workspace's
    // settings; an outsider can't even see them.
    let elsewhere = outsider_workspace(&setup.pool).await;
    let outsider = login_named(app, "outsider").await;
    let (status, _) = send(
        app,
        outsider.request(
            "GET",
            &format!("/api/v1/settings/effective?organizationId={organization}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        app,
        outsider.request(
            "GET",
            &format!(
                "/api/v1/settings/effective?projectId={project}&organizationId={organization}"
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        app,
        outsider.request(
            "GET",
            &format!("/api/v1/organizations/{organization}/secrets"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, actor_id, role)
        SELECT $1, actor_id, 'member' FROM users WHERE login_name = 'outsider'
        "#,
    )
    .bind(organization)
    .execute(&setup.pool)
    .await
    .expect("join as member");
    let (status, _) = send(
        app,
        outsider.request(
            "PUT",
            "/api/v1/settings/values/run.model",
            None,
            json!({"scope": "workspace", "organizationId": organization, "value": "mine"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Their own preference is theirs to set, and applies only to them.
    let (status, own) = send(
        app,
        outsider.request(
            "PUT",
            "/api/v1/settings/values/run.speed",
            None,
            json!({"scope": "user", "organizationId": organization, "value": "fast"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["setting"]["source"], "user");
    let (_, owners) = effective(format!("organizationId={organization}")).await;
    assert_eq!(setting(&owners, "run.speed")["source"], "default");
    // The other workspace's values never apply here.
    let (status, _) = send(
        app,
        outsider.request(
            "PUT",
            "/api/v1/settings/values/run.model",
            None,
            json!({"scope": "workspace", "organizationId": elsewhere, "value": "leaky-model"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, ours) = effective(format!("projectId={project}")).await;
    assert_eq!(setting(&ours, "run.model")["value"], "big-model");
}

#[tokio::test]
async fn access_revoked_while_a_write_is_in_flight_stops_the_write() {
    let Some(setup) = backbone("backbone_revoke").await else {
        return;
    };
    let organization = setup.organization_id;
    let writes = [
        (
            "PUT",
            "/api/v1/settings/values/run.model".to_owned(),
            json!({"scope": "workspace", "organizationId": organization, "value": "late-model"}),
        ),
        (
            "PUT",
            format!("/api/v1/organizations/{organization}/secrets/late_key"),
            json!({"value": "late-value"}),
        ),
    ];
    for (method, uri, body) in writes {
        // An administrator removes the owner and hasn't committed yet.
        let mut revoke = setup.pool.begin().await.expect("revoke");
        sqlx::query(
            r#"
            UPDATE organization_memberships SET enabled = false
            WHERE organization_id = $1
              AND actor_id = (SELECT actor_id FROM users WHERE login_name = 'owner')
            "#,
        )
        .bind(organization)
        .execute(&mut *revoke)
        .await
        .expect("disable membership");
        let request = setup.browser.request(method, &uri, None, body);
        let app = setup.app.clone();
        let write = tokio::spawn(async move { send(&app, request).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!write.is_finished(), "{uri} waits for the revocation");
        revoke.commit().await.expect("commit revocation");
        let (status, answer) = write.await.expect("write task");
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {answer}");
        sqlx::query(
            "UPDATE organization_memberships SET enabled = true WHERE organization_id = $1",
        )
        .bind(organization)
        .execute(&setup.pool)
        .await
        .expect("restore membership");
    }
    let (settings, secrets): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM settings), (SELECT count(*) FROM secrets WHERE name = 'late_key')",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("counts");
    assert_eq!((settings, secrets), (0, 0));
}

#[tokio::test]
async fn metrics_need_their_token_outside_development() {
    let Some(setup) = backbone("backbone_metrics").await else {
        return;
    };
    let directory = tempfile::tempdir().expect("token directory");
    let token_path = directory.path().join("metrics.token");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&token_path)
        .expect("token file");
    file.write_all(b"metrics-token-123\n").expect("write token");
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_METRICS_TOKEN_PATH".to_owned(),
            token_path.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("config");
    let app = router(AppState {
        config: Arc::new(config),
        ..setup.state.clone()
    });
    let scrape = |token: Option<&str>| {
        let mut request = Request::builder().uri("/metrics");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(request.body(Body::empty()).expect("request"))
    };
    assert_eq!(
        scrape(None).await.expect("response").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        scrape(Some("wrong")).await.expect("response").status(),
        StatusCode::NOT_FOUND
    );
    let response = scrape(Some("metrics-token-123")).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let text = String::from_utf8(body.to_vec()).expect("text");
    for metric in [
        "aw_job_queue{state=\"due\"}",
        "aw_outbox_lag_seconds",
        "aw_agent_webhooks_failing",
        "aw_http_requests_total",
    ] {
        assert!(text.contains(metric), "{metric} missing from\n{text}");
    }

    // A browser report larger than its limit is refused before the body is
    // read into memory.
    let oversized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/client-errors")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b' '; 64 * 1024]))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn retention_runs_as_scheduled_jobs() {
    let Some(setup) = backbone("backbone_retention").await else {
        return;
    };
    // More expired agent events than one cleanup batch holds.
    let space = send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (agent, _) = create_agent(&setup.app, &setup.browser, &space, "Wren", "wren").await;
    sqlx::query(
        r#"
        INSERT INTO external_agent_events (
            id, project_id, connection_id, sequence, kind, summary, resource_kind,
            resource_id, canonical_path, created_at, expires_at
        )
        SELECT gen_random_uuid(), connection.project_id, connection.id, 1000000 + n, 'test.expired',
               'expired', 'message', gen_random_uuid(), '/expired',
               now() - interval '40 days', now() - interval '1 day'
        FROM external_agent_connections connection, generate_series(1, 10050) n
        WHERE connection.actor_id = $1
        "#,
    )
    .bind(agent)
    .execute(&setup.pool)
    .await
    .expect("expired events");
    let registry = jobs::default_registry();
    jobs::ensure_recurring(&setup.pool, &registry)
        .await
        .expect("schedule");
    let ran = jobs::run_due(&setup.state, &registry, &options())
        .await
        .expect("run");
    assert!(ran >= 4, "ran {ran}");
    let states = sqlx::query_as::<_, (String, String)>(
        "SELECT kind, state FROM background_jobs WHERE kind LIKE 'retention.%' AND finished_at IS NOT NULL ORDER BY kind",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("jobs");
    assert_eq!(
        states,
        vec![
            (
                "retention.database_windows".to_owned(),
                "succeeded".to_owned()
            ),
            (
                "retention.external_events".to_owned(),
                "succeeded".to_owned()
            ),
            ("retention.worktrees".to_owned(), "succeeded".to_owned()),
        ]
    );
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM external_agent_events WHERE expires_at <= now()",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("left");
    assert_eq!(left, 0, "one run drains every expired event");
    // Each queued its next run for later.
    let later = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind LIKE 'retention.%' AND state = 'queued' AND run_at > now() + interval '5 hours'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("next runs");
    assert_eq!(later, 3);
}
