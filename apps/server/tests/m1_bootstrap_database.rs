use std::sync::Arc;

use agent_workspace_server::bootstrap::{BootstrapCommand, BootstrapError, bootstrap_admin};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn bootstrap_adopts_m0_owner_and_has_exactly_one_concurrent_winner() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m1_bootstrap_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated bootstrap schema");
    let pool = schema_pool(&database_url, &schema).await;

    sqlx::raw_sql(include_str!("../migrations/0001_foundation.sql"))
        .execute(&pool)
        .await
        .expect("apply foundation migration");
    sqlx::raw_sql(include_str!("../migrations/0002_walking_skeleton.sql"))
        .execute(&pool)
        .await
        .expect("apply walking-skeleton migration");
    sqlx::raw_sql(include_str!("../migrations/0003_m0_consistency.sql"))
        .execute(&pool)
        .await
        .expect("apply M0 consistency migration");

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("insert M0 Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'M0 Personal', $2)")
        .bind(organization_id)
        .bind(format!("m0-personal-{}", organization_id.simple()))
        .execute(&pool)
        .await
        .expect("insert M0 Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'AW', 'Agent Workspace')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert M0 Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Mischa', 'mischa')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert M0 ownership");
    sqlx::raw_sql(include_str!("../migrations/0004_m1_identity_projects.sql"))
        .execute(&pool)
        .await
        .expect("apply M1 migration");

    let command = BootstrapCommand {
        login_name: "mischa".to_owned(),
        display_name: "Mischa".to_owned(),
        organization_name: None,
        organization_slug: None,
        adopt_actor_id: None,
        organization_id: None,
    };
    let first = bootstrap_admin(
        &pool,
        command.clone(),
        "correct horse battery staple".to_owned(),
    );
    let second = bootstrap_admin(&pool, command, "correct horse battery staple".to_owned());
    let (first, second) = tokio::join!(first, second);
    let results = [first, second];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "exactly one bootstrap attempt must commit"
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(BootstrapError::AlreadyBootstrapped)))
            .count(),
        1,
        "the losing bootstrap must observe the committed User"
    );
    let winner = results
        .into_iter()
        .find_map(Result::ok)
        .expect("bootstrap winner");
    assert_eq!(winner.actor_id, actor_id);
    assert_eq!(winner.organization_id, organization_id);
    assert!(winner.adopted_existing_actor);

    let stored = sqlx::query_as::<_, (Uuid, String, i64, i64)>(
        r#"
        SELECT u.actor_id, c.password_phc,
               (SELECT count(*) FROM users),
               (SELECT count(*) FROM audit_events
                 WHERE action = 'identity.bootstrap_admin'
                   AND outcome = 'succeeded')
        FROM users u
        JOIN user_password_credentials c ON c.user_id = u.id
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read bootstrapped identity");
    assert_eq!(stored.0, actor_id);
    assert!(stored.1.starts_with("$argon2id$"));
    assert!(!stored.1.contains("correct horse battery staple"));
    assert_eq!(stored.2, 1);
    assert_eq!(stored.3, 1);

    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated bootstrap schema");
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _metadata| {
            let schema = Arc::clone(&schema);
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema.as_str())
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema-scoped bootstrap pool")
}
