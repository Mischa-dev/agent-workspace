use std::sync::Arc;

use agent_workspace_server::{
    auth::password,
    bootstrap::{
        BootstrapCommand, BootstrapError, PasswordRotationCommand, bootstrap_admin, rotate_password,
    },
};
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

    sqlx::raw_sql(include_str!("../../migrations/0001_foundation.sql"))
        .execute(&pool)
        .await
        .expect("apply foundation migration");
    sqlx::raw_sql(include_str!("../../migrations/0002_walking_skeleton.sql"))
        .execute(&pool)
        .await
        .expect("apply walking-skeleton migration");
    sqlx::raw_sql(include_str!("../../migrations/0003_m0_consistency.sql"))
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
    sqlx::raw_sql(include_str!(
        "../../migrations/0004_m1_identity_projects.sql"
    ))
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

    let repeated = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "another-admin".to_owned(),
            display_name: "Another Administrator".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: None,
            organization_id: None,
        },
        "another correct horse battery staple".to_owned(),
    )
    .await;
    assert!(
        matches!(repeated, Err(BootstrapError::AlreadyBootstrapped)),
        "a later deployed-style bootstrap attempt must reject once the first User exists"
    );
    let persisted_counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64)>(
        r#"
        SELECT
            (SELECT count(*) FROM users),
            (SELECT count(*) FROM user_password_credentials),
            (SELECT count(*) FROM organization_memberships WHERE role = 'owner'),
            (SELECT count(*) FROM project_memberships WHERE role = 'owner'),
            (SELECT count(*) FROM audit_events
              WHERE action = 'identity.bootstrap_admin'
                AND outcome = 'succeeded')
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read state after repeated bootstrap");
    assert_eq!(
        persisted_counts,
        (1, 1, 1, 1, 1),
        "a rejected repeat must not create another User, credential, Owner, or audit event"
    );

    let before_rotation = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT u.id, u.authentication_epoch
        FROM users u
        JOIN user_password_credentials credential ON credential.user_id = u.id
        WHERE u.login_name = 'mischa'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read pre-rotation identity");
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        ) VALUES (
            $1, $2, $3, $4, $5, now() + interval '30 minutes',
            now() + interval '8 hours'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(before_rotation.0)
    .bind(vec![1_u8; 32])
    .bind(vec![2_u8; 32])
    .bind(before_rotation.1)
    .execute(&pool)
    .await
    .expect("insert active session fixture");
    let replacement = "replacement correct horse battery staple";
    let rotation = rotate_password(
        &pool,
        PasswordRotationCommand {
            login_name: "mischa".to_owned(),
        },
        replacement.to_owned(),
    )
    .await
    .expect("rotate password");
    assert_eq!(rotation.user_id, before_rotation.0);
    assert_eq!(rotation.authentication_epoch, before_rotation.1 + 1);
    assert_eq!(rotation.revoked_sessions, 1);
    let after_rotation = sqlx::query_as::<_, (i64, String, bool, i64)>(
        r#"
        SELECT u.authentication_epoch, credential.password_phc,
               session.revoked_at IS NOT NULL,
               (SELECT count(*) FROM audit_events
                WHERE action = 'identity.password.rotate'
                  AND target_id = u.id
                  AND outcome = 'succeeded')
        FROM users u
        JOIN user_password_credentials credential ON credential.user_id = u.id
        JOIN sessions session ON session.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(before_rotation.0)
    .fetch_one(&pool)
    .await
    .expect("read rotated identity");
    assert_eq!(after_rotation.0, before_rotation.1 + 1);
    assert!(after_rotation.2);
    assert_eq!(after_rotation.3, 1);
    assert!(
        !password::verify_password(
            "correct horse battery staple".to_owned(),
            after_rotation.1.clone(),
        )
        .await
        .expect("reject prior password")
    );
    assert!(
        password::verify_password(replacement.to_owned(), after_rotation.1)
            .await
            .expect("verify replacement password")
    );

    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated bootstrap schema");
}

#[tokio::test]
async fn bootstrap_rolls_back_every_insert_when_a_late_database_write_fails() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m1_bootstrap_rollback_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated bootstrap rollback schema");
    let pool = schema_pool(&database_url, &schema).await;
    for migration in [
        include_str!("../../migrations/0001_foundation.sql"),
        include_str!("../../migrations/0002_walking_skeleton.sql"),
        include_str!("../../migrations/0003_m0_consistency.sql"),
        include_str!("../../migrations/0004_m1_identity_projects.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&pool)
            .await
            .expect("apply migration");
    }
    sqlx::raw_sql(
        r#"
        CREATE FUNCTION reject_bootstrap_organization_membership()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            RAISE EXCEPTION 'induced bootstrap membership failure';
        END;
        $$;

        CREATE TRIGGER reject_bootstrap_organization_membership
        BEFORE INSERT ON organization_memberships
        FOR EACH ROW
        EXECUTE FUNCTION reject_bootstrap_organization_membership();
        "#,
    )
    .execute(&pool)
    .await
    .expect("install late bootstrap failure trigger");

    let failed = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "rollback-admin".to_owned(),
            display_name: "Rollback Administrator".to_owned(),
            organization_name: Some("Rollback Organization".to_owned()),
            organization_slug: Some(format!("rollback-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        "correct horse battery staple".to_owned(),
    )
    .await;
    assert!(
        matches!(failed, Err(BootstrapError::Database(_))),
        "the induced late database failure must escape as a bootstrap database error"
    );

    let counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(
        r#"
        SELECT
            (SELECT count(*) FROM actors),
            (SELECT count(*) FROM organizations),
            (SELECT count(*) FROM users),
            (SELECT count(*) FROM user_password_credentials),
            (SELECT count(*) FROM organization_memberships),
            (SELECT count(*) FROM audit_events)
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read state after failed bootstrap");
    assert_eq!(
        counts,
        (0, 0, 0, 0, 0, 0),
        "Actor, Organization, User, credential, membership, and audit inserts must roll back together"
    );

    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated bootstrap rollback schema");
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
