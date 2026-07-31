use std::time::Duration;

use sqlx::{Acquire, PgPool, Row, postgres::PgPoolOptions};
use tokio::time::timeout;
use uuid::Uuid;

#[tokio::test]
async fn m1_upgrade_rejects_incompatible_legacy_actor_roles_without_rewriting_them() {
    let Some(pool) = integration_pool().await else {
        return;
    };

    let schema = format!("m1_legacy_role_{}", Uuid::now_v7().simple());
    let mut connection = pool.acquire().await.expect("acquire PostgreSQL connection");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut *connection)
        .await
        .expect("create isolated legacy-role schema");
    sqlx::query("SELECT set_config('search_path', $1, false)")
        .bind(&schema)
        .execute(&mut *connection)
        .await
        .expect("select isolated legacy-role schema");
    for migration in [
        include_str!("../migrations/0001_foundation.sql"),
        include_str!("../migrations/0002_walking_skeleton.sql"),
        include_str!("../migrations/0003_m0_consistency.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *connection)
            .await
            .expect("apply M0 migration");
    }

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&mut *connection)
        .await
        .expect("insert legacy human Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Legacy role', $2)")
        .bind(organization_id)
        .bind(format!("legacy-role-{}", organization_id.simple()))
        .execute(&mut *connection)
        .await
        .expect("insert legacy Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'LR', 'Legacy role Project')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&mut *connection)
    .await
    .expect("insert legacy Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Legacy human worker', 'legacy-worker')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut *connection)
    .await
    .expect("insert M0-compatible mismatched role");

    let upgrade = sqlx::raw_sql(include_str!("../migrations/0004_m1_identity_projects.sql"))
        .execute(&mut *connection)
        .await;
    assert!(
        upgrade.is_err(),
        "M1 must reject an incompatible legacy Actor role instead of rewriting authority"
    );

    sqlx::query("SELECT set_config('search_path', 'public', false)")
        .execute(&mut *connection)
        .await
        .expect("restore default schema");
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut *connection)
        .await
        .expect("drop isolated legacy-role schema");
}

#[tokio::test]
async fn m0_to_m1_identity_and_project_upgrade_enforces_invariants() {
    let Some(pool) = integration_pool().await else {
        return;
    };

    let schema = format!("m1_identity_{}", Uuid::now_v7().simple());
    let mut connection = pool.acquire().await.expect("acquire PostgreSQL connection");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut *connection)
        .await
        .expect("create isolated migration schema");
    sqlx::query("SELECT set_config('search_path', $1, false)")
        .bind(&schema)
        .execute(&mut *connection)
        .await
        .expect("select isolated migration schema");

    sqlx::raw_sql(include_str!("../migrations/0001_foundation.sql"))
        .execute(&mut *connection)
        .await
        .expect("apply M0 foundation migration");
    sqlx::raw_sql(include_str!("../migrations/0002_walking_skeleton.sql"))
        .execute(&mut *connection)
        .await
        .expect("apply M0 walking-skeleton migration");
    sqlx::raw_sql(include_str!("../migrations/0003_m0_consistency.sql"))
        .execute(&mut *connection)
        .await
        .expect("apply M0 consistency migration");

    let human_actor_id = Uuid::now_v7();
    let worker_actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let owner_membership_id = Uuid::now_v7();
    let worker_membership_id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO actors (id, kind)
        VALUES ($1, 'human'), ($2, 'execution_worker')
        "#,
    )
    .bind(human_actor_id)
    .bind(worker_actor_id)
    .execute(&mut *connection)
    .await
    .expect("insert M0 Actors");
    sqlx::query(
        r#"
        INSERT INTO organizations (id, name, slug)
        VALUES ($1, 'M0 upgrade organization', $2)
        "#,
    )
    .bind(organization_id)
    .bind(format!("m0-upgrade-{}", organization_id.simple()))
    .execute(&mut *connection)
    .await
    .expect("insert M0 Organization");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'UP', 'M0 upgrade Project')
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&mut *connection)
    .await
    .expect("insert M0 Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES
            ($1, $2, $3, 'owner', 'Upgrade owner', 'upgrade-owner'),
            ($4, $2, $5, 'worker', 'Upgrade worker', 'upgrade-worker')
        "#,
    )
    .bind(owner_membership_id)
    .bind(project_id)
    .bind(human_actor_id)
    .bind(worker_membership_id)
    .bind(worker_actor_id)
    .execute(&mut *connection)
    .await
    .expect("insert M0 Project memberships");

    sqlx::raw_sql(include_str!("../migrations/0004_m1_identity_projects.sql"))
        .execute(&mut *connection)
        .await
        .expect("upgrade M0 schema to M1");

    let upgraded_project = sqlx::query(
        r#"
        SELECT p.id,
               p.organization_id,
               p.version,
               pc.next_work_item_number,
               pi.content,
               pi.version AS instructions_version
          FROM projects p
          JOIN project_counters pc ON pc.project_id = p.id
          JOIN project_instructions pi ON pi.project_id = p.id
         WHERE p.id = $1
        "#,
    )
    .bind(project_id)
    .fetch_one(&mut *connection)
    .await
    .expect("read upgraded M0 Project");
    assert_eq!(upgraded_project.get::<Uuid, _>("id"), project_id);
    assert_eq!(
        upgraded_project.get::<Uuid, _>("organization_id"),
        organization_id
    );
    assert_eq!(upgraded_project.get::<i64, _>("version"), 1);
    assert_eq!(upgraded_project.get::<i64, _>("next_work_item_number"), 1);
    assert_eq!(upgraded_project.get::<String, _>("content"), "");
    assert_eq!(upgraded_project.get::<i64, _>("instructions_version"), 1);

    let preserved_ids = sqlx::query(
        r#"
        SELECT
            (SELECT count(*) FROM actors WHERE id IN ($1, $2)) AS actors,
            (
                SELECT count(*)
                  FROM project_memberships
                 WHERE id IN ($3, $4)
            ) AS memberships
        "#,
    )
    .bind(human_actor_id)
    .bind(worker_actor_id)
    .bind(owner_membership_id)
    .bind(worker_membership_id)
    .fetch_one(&mut *connection)
    .await
    .expect("verify preserved M0 identifiers");
    assert_eq!(preserved_ids.get::<i64, _>("actors"), 2);
    assert_eq!(preserved_ids.get::<i64, _>("memberships"), 2);

    let organization_owner = sqlx::query_scalar::<_, String>(
        r#"
        SELECT role
          FROM organization_memberships
         WHERE organization_id = $1
           AND actor_id = $2
        "#,
    )
    .bind(organization_id)
    .bind(human_actor_id)
    .fetch_one(&mut *connection)
    .await
    .expect("read backfilled Organization owner");
    assert_eq!(organization_owner, "owner");

    let authentication_rows =
        sqlx::query("SELECT (SELECT count(*) FROM users) AS users, (SELECT count(*) FROM user_password_credentials) AS credentials, (SELECT count(*) FROM sessions) AS sessions")
            .fetch_one(&mut *connection)
            .await
            .expect("count authentication rows");
    assert_eq!(authentication_rows.get::<i64, _>("users"), 0);
    assert_eq!(authentication_rows.get::<i64, _>("credentials"), 0);
    assert_eq!(authentication_rows.get::<i64, _>("sessions"), 0);

    let user_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO users (
            id, actor_id, login_name, display_name, authentication_epoch
        )
        VALUES ($1, $2, 'upgrade-owner', 'Upgrade Owner', 1)
        "#,
    )
    .bind(user_id)
    .bind(human_actor_id)
    .execute(&mut *connection)
    .await
    .expect("insert human User");

    let worker_user = sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, 'worker-user', 'Invalid Worker User')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(worker_actor_id)
    .execute(&mut *connection)
    .await;
    assert!(
        worker_user.is_err(),
        "a User must not reference an Execution Worker Actor"
    );

    let second_human_actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(second_human_actor_id)
        .execute(&mut *connection)
        .await
        .expect("insert second human Actor");
    let duplicate_login = sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, 'upgrade-owner', 'Duplicate Login')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(second_human_actor_id)
    .execute(&mut *connection)
    .await;
    assert!(
        duplicate_login.is_err(),
        "login names must be unique case-insensitively"
    );
    let short_login = sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, 'ab', 'Short Login')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(second_human_actor_id)
    .execute(&mut *connection)
    .await;
    assert!(
        short_login.is_err(),
        "database login validation must match the three-character application minimum"
    );

    let invalid_password = sqlx::query(
        r#"
        INSERT INTO user_password_credentials (user_id, password_phc)
        VALUES ($1, 'plaintext-password')
        "#,
    )
    .bind(user_id)
    .execute(&mut *connection)
    .await;
    assert!(
        invalid_password.is_err(),
        "password rows must accept only Argon2id PHC strings"
    );

    let password_phc =
        "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQxMjM0NTY$YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo";
    sqlx::query(
        r#"
        INSERT INTO user_password_credentials (user_id, password_phc)
        VALUES ($1, $2)
        "#,
    )
    .bind(user_id)
    .bind(password_phc)
    .execute(&mut *connection)
    .await
    .expect("insert Argon2id credential");

    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, decode(repeat('ab', 32), 'hex'),
            decode(repeat('cd', 32), 'hex'), 1,
            now() + interval '1 hour', now() + interval '8 hours'
        )
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .execute(&mut *connection)
    .await
    .expect("insert digest-only Session");

    let digest_lengths = sqlx::query(
        r#"
        SELECT octet_length(token_hash) AS token_length,
               octet_length(csrf_token_hash) AS csrf_length
          FROM sessions
         WHERE id = $1
        "#,
    )
    .bind(session_id)
    .fetch_one(&mut *connection)
    .await
    .expect("read Session digests");
    assert_eq!(digest_lengths.get::<i32, _>("token_length"), 32);
    assert_eq!(digest_lengths.get::<i32, _>("csrf_length"), 32);

    let short_digest_session = sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, decode(repeat('ef', 31), 'hex'),
            decode(repeat('12', 32), 'hex'), 1,
            now() + interval '1 hour', now() + interval '8 hours'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .execute(&mut *connection)
    .await;
    assert!(
        short_digest_session.is_err(),
        "Session and CSRF hashes must be exactly 32 bytes"
    );

    sqlx::query("UPDATE sessions SET revoked_at = now(), updated_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(&mut *connection)
        .await
        .expect("revoke Session");
    let revoked_session_is_active = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT revoked_at IS NULL
               AND idle_expires_at > now()
               AND absolute_expires_at > now()
          FROM sessions
         WHERE id = $1
        "#,
    )
    .bind(session_id)
    .fetch_one(&mut *connection)
    .await
    .expect("classify revoked Session");
    assert!(!revoked_session_is_active);

    let expired_session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            created_at, last_seen_at, idle_expires_at, absolute_expires_at,
            updated_at
        )
        VALUES (
            $1, $2, decode(repeat('78', 32), 'hex'),
            decode(repeat('90', 32), 'hex'), 1,
            now() - interval '2 hours', now() - interval '2 hours',
            now() - interval '1 hour', now() + interval '1 hour',
            now()
        )
        "#,
    )
    .bind(expired_session_id)
    .bind(user_id)
    .execute(&mut *connection)
    .await
    .expect("insert expired Session history");
    let expired_session_is_active = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT revoked_at IS NULL
               AND idle_expires_at > now()
               AND absolute_expires_at > now()
          FROM sessions
         WHERE id = $1
        "#,
    )
    .bind(expired_session_id)
    .fetch_one(&mut *connection)
    .await
    .expect("classify expired Session");
    assert!(!expired_session_is_active);

    let idempotency_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO idempotency_keys (
            id, actor_id, scope, key, request_hash, expires_at
        )
        VALUES (
            $1, $2, 'projects.create', 'create-project-1',
            decode(repeat('34', 32), 'hex'), now() + interval '1 day'
        )
        "#,
    )
    .bind(idempotency_id)
    .bind(human_actor_id)
    .execute(&mut *connection)
    .await
    .expect("claim idempotency key");
    let duplicate_idempotency_key = sqlx::query(
        r#"
        INSERT INTO idempotency_keys (
            id, actor_id, scope, key, request_hash, expires_at
        )
        VALUES (
            $1, $2, 'projects.create', 'create-project-1',
            decode(repeat('56', 32), 'hex'), now() + interval '1 day'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(human_actor_id)
    .execute(&mut *connection)
    .await;
    assert!(
        duplicate_idempotency_key.is_err(),
        "an Actor cannot reuse an idempotency key within the same scope"
    );

    let invalid_worker_role = sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Invalid human worker', 'human-worker')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(second_human_actor_id)
    .execute(&mut *connection)
    .await;
    assert!(
        invalid_worker_role.is_err(),
        "only Execution Worker Actors may receive the Worker role"
    );

    let invalid_worker_membership =
        sqlx::query("UPDATE project_memberships SET role = 'member' WHERE id = $1")
            .bind(worker_membership_id)
            .execute(&mut *connection)
            .await;
    assert!(
        invalid_worker_membership.is_err(),
        "Execution Worker Actors must retain the Worker role"
    );

    let human_kind_change = sqlx::query("UPDATE actors SET kind = 'system' WHERE id = $1")
        .bind(human_actor_id)
        .execute(&mut *connection)
        .await;
    assert!(
        human_kind_change.is_err(),
        "an Actor referenced by a User must remain human"
    );

    let race_actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(race_actor_id)
        .execute(&mut *connection)
        .await
        .expect("insert Actor for kind race");
    let mut user_connection = pool.acquire().await.expect("acquire User race connection");
    let mut actor_connection = pool.acquire().await.expect("acquire Actor race connection");
    for race_connection in [&mut user_connection, &mut actor_connection] {
        sqlx::query("SELECT set_config('search_path', $1, false)")
            .bind(&schema)
            .execute(&mut **race_connection)
            .await
            .expect("select race-test schema");
    }
    let mut user_transaction = user_connection.begin().await.expect("begin User insert");
    sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, $3, 'Concurrent identity')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(race_actor_id)
    .bind(format!(
        "race-{}",
        &race_actor_id.simple().to_string()[..16]
    ))
    .execute(&mut *user_transaction)
    .await
    .expect("insert uncommitted User while holding Actor lock");
    let mut kind_update = tokio::spawn(async move {
        sqlx::query("UPDATE actors SET kind = 'system' WHERE id = $1")
            .bind(race_actor_id)
            .execute(&mut *actor_connection)
            .await
    });
    assert!(
        timeout(Duration::from_millis(150), &mut kind_update)
            .await
            .is_err(),
        "the concurrent Actor-kind update must wait for the User invariant lock"
    );
    user_transaction
        .commit()
        .await
        .expect("commit concurrent User");
    let kind_update = timeout(Duration::from_secs(2), kind_update)
        .await
        .expect("Actor-kind update resumes after User commit")
        .expect("Actor-kind update task");
    assert!(
        kind_update.is_err(),
        "the resumed Actor-kind update must observe and reject the committed User"
    );

    let second_project_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'NX', 'Navigation Project')
        "#,
    )
    .bind(second_project_id)
    .bind(organization_id)
    .execute(&mut *connection)
    .await
    .expect("insert second Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'admin', 'Upgrade owner', 'upgrade-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(second_project_id)
    .bind(human_actor_id)
    .execute(&mut *connection)
    .await
    .expect("insert expanded Admin Project role");
    sqlx::query("INSERT INTO project_counters (project_id) VALUES ($1)")
        .bind(second_project_id)
        .execute(&mut *connection)
        .await
        .expect("insert second Project counter");
    sqlx::query("INSERT INTO project_instructions (project_id) VALUES ($1)")
        .bind(second_project_id)
        .execute(&mut *connection)
        .await
        .expect("insert second Project instructions");

    let first_channel_id = Uuid::now_v7();
    let first_thread_id = Uuid::now_v7();
    let second_channel_id = Uuid::now_v7();
    let second_thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES
            ($1, $2, 'channel', 'first-channel'),
            ($3, $4, 'channel', 'second-channel')
        "#,
    )
    .bind(first_channel_id)
    .bind(project_id)
    .bind(second_channel_id)
    .bind(second_project_id)
    .execute(&mut *connection)
    .await
    .expect("insert navigation Channels");
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, name
        )
        VALUES
            ($1, $2, 'thread', $3, 'first-thread'),
            ($4, $5, 'thread', $6, 'second-thread')
        "#,
    )
    .bind(first_thread_id)
    .bind(project_id)
    .bind(first_channel_id)
    .bind(second_thread_id)
    .bind(second_project_id)
    .bind(second_channel_id)
    .execute(&mut *connection)
    .await
    .expect("insert navigation Threads");

    let cross_project_navigation = sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, conversation_id
        )
        VALUES ($1, $2, 'thread', $3)
        "#,
    )
    .bind(project_id)
    .bind(human_actor_id)
    .bind(second_thread_id)
    .execute(&mut *connection)
    .await;
    assert!(
        cross_project_navigation.is_err(),
        "navigation must reject a Conversation from another Project"
    );

    let channel_as_thread_navigation = sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, conversation_id
        )
        VALUES ($1, $2, 'thread', $3)
        "#,
    )
    .bind(project_id)
    .bind(human_actor_id)
    .bind(first_channel_id)
    .execute(&mut *connection)
    .await;
    assert!(
        channel_as_thread_navigation.is_err(),
        "a Thread navigation location must reference a Thread"
    );

    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, conversation_id,
            context_drawer_open, filters, pinned_at
        )
        VALUES ($1, $2, 'thread', $3, true, '{"view":"timeline"}', now())
        "#,
    )
    .bind(project_id)
    .bind(human_actor_id)
    .bind(first_thread_id)
    .execute(&mut *connection)
    .await
    .expect("insert same-Project navigation state");

    sqlx::query(
        r#"
        UPDATE projects
           SET project_key = 'NY',
               version = version + 1,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(second_project_id)
    .execute(&mut *connection)
    .await
    .expect("change Project key before first allocation");
    sqlx::query(
        r#"
        UPDATE project_counters
           SET next_work_item_number = next_work_item_number + 1,
               updated_at = now()
         WHERE project_id = $1
        "#,
    )
    .bind(second_project_id)
    .execute(&mut *connection)
    .await
    .expect("allocate first Project counter");
    let allocated_key_change = sqlx::query("UPDATE projects SET project_key = 'NZ' WHERE id = $1")
        .bind(second_project_id)
        .execute(&mut *connection)
        .await;
    assert!(
        allocated_key_change.is_err(),
        "Project keys must become immutable after first allocation"
    );

    let counter_rewind =
        sqlx::query("UPDATE project_counters SET next_work_item_number = 1 WHERE project_id = $1")
            .bind(second_project_id)
            .execute(&mut *connection)
            .await;
    assert!(
        counter_rewind.is_err(),
        "Project counters must be monotonic"
    );

    let unattributed_archive = sqlx::query("UPDATE projects SET archived_at = now() WHERE id = $1")
        .bind(second_project_id)
        .execute(&mut *connection)
        .await;
    assert!(
        unattributed_archive.is_err(),
        "new Project archives must identify the responsible Actor"
    );
    sqlx::query(
        r#"
        UPDATE projects
           SET archived_at = now(),
               archived_by_actor_id = $2,
               version = version + 1,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(second_project_id)
    .bind(human_actor_id)
    .execute(&mut *connection)
    .await
    .expect("archive Project with Actor attribution");

    let duplicate_archived_key = sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'NY', 'Duplicate archived key')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .execute(&mut *connection)
    .await;
    assert!(
        duplicate_archived_key.is_err(),
        "Organization Project keys remain unique across archived Projects"
    );

    let concurrency_project_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'CC', 'Concurrent Counter Project')
        "#,
    )
    .bind(concurrency_project_id)
    .bind(organization_id)
    .execute(&mut *connection)
    .await
    .expect("insert concurrency Project");
    sqlx::query("INSERT INTO project_counters (project_id) VALUES ($1)")
        .bind(concurrency_project_id)
        .execute(&mut *connection)
        .await
        .expect("insert concurrency counter");

    let mut first_allocator = pool.acquire().await.expect("acquire first allocator");
    let mut second_allocator = pool.acquire().await.expect("acquire second allocator");
    for allocator in [&mut first_allocator, &mut second_allocator] {
        sqlx::query("SELECT set_config('search_path', $1, false)")
            .bind(&schema)
            .execute(&mut **allocator)
            .await
            .expect("select allocator schema");
    }
    let first_allocation = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE project_counters
           SET next_work_item_number = next_work_item_number + 1,
               updated_at = now()
         WHERE project_id = $1
        RETURNING next_work_item_number - 1
        "#,
    )
    .bind(concurrency_project_id)
    .fetch_one(&mut *first_allocator);
    let second_allocation = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE project_counters
           SET next_work_item_number = next_work_item_number + 1,
               updated_at = now()
         WHERE project_id = $1
        RETURNING next_work_item_number - 1
        "#,
    )
    .bind(concurrency_project_id)
    .fetch_one(&mut *second_allocator);
    let (first_allocation, second_allocation) = tokio::join!(first_allocation, second_allocation);
    let mut allocated_numbers = vec![
        first_allocation.expect("first concurrent allocation"),
        second_allocation.expect("second concurrent allocation"),
    ];
    allocated_numbers.sort_unstable();
    assert_eq!(allocated_numbers, vec![1, 2]);

    drop(first_allocator);
    drop(second_allocator);
    sqlx::query("SELECT set_config('search_path', 'public', false)")
        .execute(&mut *connection)
        .await
        .expect("restore default schema");
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut *connection)
        .await
        .expect("drop isolated migration schema");
}

async fn integration_pool() -> Option<PgPool> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };

    Some(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("connect to PostgreSQL"),
    )
}
