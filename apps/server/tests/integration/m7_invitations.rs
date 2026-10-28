use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    auth::password,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::dev_seed,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const OWNER_LOGIN: &str = "m7-owner";
const OWNER_PASSWORD: &str = "correct horse battery staple";
const ADMIN_LOGIN: &str = "m7-admin";
const ADMIN_PASSWORD: &str = "another correct horse battery staple";

#[tokio::test]
async fn project_invitations_are_digest_only_role_scoped_and_revocable() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m7_invitations_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M7 schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M7 migrations");

    let temporary = tempfile::tempdir().expect("temporary M7 root");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary.path().join("data").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary
                .path()
                .join("data/codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M7 configuration");
    let seed = dev_seed(&pool, &config).await.expect("seed M7 fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "M7 Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        OWNER_PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M7 owner");

    let admin_actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(admin_actor_id)
        .execute(&pool)
        .await
        .expect("insert M7 admin Actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'admin')",
    )
    .bind(seed.organization_id)
    .bind(admin_actor_id)
    .execute(&pool)
    .await
    .expect("insert M7 admin Organization membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2, $3, 'admin', 'M7 Admin', 'm7_admin')",
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .bind(admin_actor_id)
    .execute(&pool)
    .await
    .expect("insert M7 admin Project membership");
    bootstrap_development_member(
        &pool,
        BootstrapCommand {
            login_name: ADMIN_LOGIN.to_owned(),
            display_name: "M7 Admin".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(admin_actor_id),
            organization_id: Some(seed.organization_id),
        },
        ADMIN_PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M7 admin");

    let application = test_app(pool.clone(), config);
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN, OWNER_PASSWORD).await;
    let (admin_cookie, admin_csrf) = login(&application, ADMIN_LOGIN, ADMIN_PASSWORD).await;
    let invitations_uri = format!("/api/v1/projects/{}/invitations", seed.project_id);

    let missing_csrf = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", owner_cookie.as_str()),
            ],
            json!({"role":"viewer","expiresInHours":24}),
        ))
        .await
        .expect("missing CSRF invitation response");
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let invalid_expiry = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"role":"viewer","expiresInHours":169}),
        ))
        .await
        .expect("invalid expiry response");
    assert_eq!(invalid_expiry.status(), StatusCode::BAD_REQUEST);

    let issued_response = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"role":"admin","expiresInHours":24}),
        ))
        .await
        .expect("owner invitation response");
    assert_eq!(issued_response.status(), StatusCode::CREATED);
    let issued = response_json(issued_response).await;
    let invitation_id = issued["id"]
        .as_str()
        .expect("invitation ID")
        .parse::<Uuid>()
        .expect("invitation UUID");
    let invitation_url = issued["invitationUrl"]
        .as_str()
        .expect("one-shot invitation URL");
    let token = invitation_url
        .strip_prefix("http://127.0.0.1:3100/join#token=")
        .expect("fragment-only invitation token");
    assert!(token.starts_with("aw_inv_"));
    assert_eq!(token.len(), 50);

    let stored_digest = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT token_digest FROM project_invitations WHERE id = $1",
    )
    .bind(invitation_id)
    .fetch_one(&pool)
    .await
    .expect("stored invitation digest");
    assert_eq!(stored_digest.len(), 32);
    assert!(!String::from_utf8_lossy(&stored_digest).contains(token));

    let listed_response = application
        .clone()
        .oneshot(request(
            "GET",
            &invitations_uri,
            &[("cookie", owner_cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("list invitations response");
    assert_eq!(listed_response.status(), StatusCode::OK);
    let listed = response_json(listed_response).await;
    assert_eq!(listed.as_array().expect("invitation list").len(), 1);
    assert_eq!(listed[0]["id"], issued["id"]);
    assert_eq!(listed[0]["tokenPrefix"], issued["tokenPrefix"]);
    let safe_list = serde_json::to_string(&listed).expect("serialize safe list");
    assert!(!safe_list.contains(token));
    assert!(!safe_list.contains("invitationUrl"));

    let admin_member = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&admin_cookie, &admin_csrf),
            json!({"role":"member","expiresInHours":24}),
        ))
        .await
        .expect("admin member invitation response");
    assert_eq!(admin_member.status(), StatusCode::CREATED);
    let admin_member = response_json(admin_member).await;
    let admin_member_id = admin_member["id"]
        .as_str()
        .expect("admin member invitation ID")
        .parse::<Uuid>()
        .expect("admin member invitation UUID");
    let admin_member_token = admin_member["invitationUrl"]
        .as_str()
        .expect("admin member invitation URL")
        .split_once("#token=")
        .expect("admin member token fragment")
        .1
        .to_owned();

    let replace_uri = format!("/api/v1/project-invitations/{admin_member_id}/replace");
    let replacement_response = application
        .clone()
        .oneshot(request(
            "POST",
            &replace_uri,
            &write_headers(&admin_cookie, &admin_csrf),
            json!({"expectedVersion":1,"expiresInHours":48}),
        ))
        .await
        .expect("replace invitation response");
    assert_eq!(replacement_response.status(), StatusCode::CREATED);
    let replacement = response_json(replacement_response).await;
    let replacement_id = replacement["id"]
        .as_str()
        .expect("replacement invitation ID")
        .parse::<Uuid>()
        .expect("replacement invitation UUID");
    let replacement_token = replacement["invitationUrl"]
        .as_str()
        .expect("replacement invitation URL")
        .split_once("#token=")
        .expect("replacement token fragment")
        .1;
    assert_ne!(replacement_token, admin_member_token);
    assert_eq!(replacement["role"], "member");
    let replacement_shape = sqlx::query_as::<_, (String, i64, Option<Uuid>)>(
        "SELECT status, version, replaced_by_invitation_id FROM project_invitations WHERE id = $1",
    )
    .bind(admin_member_id)
    .fetch_one(&pool)
    .await
    .expect("replaced invitation shape");
    assert_eq!(
        replacement_shape,
        ("replaced".to_owned(), 2, Some(replacement_id))
    );
    let replayed_replace = application
        .clone()
        .oneshot(request(
            "POST",
            &replace_uri,
            &write_headers(&admin_cookie, &admin_csrf),
            json!({"expectedVersion":1,"expiresInHours":48}),
        ))
        .await
        .expect("replayed replace response");
    assert_eq!(replayed_replace.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM project_invitations WHERE project_id = $1 AND role = 'member'",
        )
        .bind(seed.project_id)
        .fetch_one(&pool)
        .await
        .expect("member invitation count"),
        2
    );

    let admin_admin = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&admin_cookie, &admin_csrf),
            json!({"role":"admin","expiresInHours":24}),
        ))
        .await
        .expect("admin elevation invitation response");
    assert_eq!(admin_admin.status(), StatusCode::FORBIDDEN);

    sqlx::query(
        "UPDATE organization_memberships SET role = 'member', version = version + 1, updated_at = now() WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(admin_actor_id)
    .execute(&pool)
    .await
    .expect("demote Organization admin fixture");
    let project_role_is_not_authority = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&admin_cookie, &admin_csrf),
            json!({"role":"viewer","expiresInHours":24}),
        ))
        .await
        .expect("Project-role-only invitation response");
    assert_eq!(
        project_role_is_not_authority.status(),
        StatusCode::FORBIDDEN
    );

    let revoke_uri = format!("/api/v1/project-invitations/{invitation_id}/revoke");
    let stale_revoke = application
        .clone()
        .oneshot(request(
            "POST",
            &revoke_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"expectedVersion":2}),
        ))
        .await
        .expect("stale revoke response");
    assert_eq!(stale_revoke.status(), StatusCode::CONFLICT);

    let revoked_response = application
        .clone()
        .oneshot(request(
            "POST",
            &revoke_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"expectedVersion":1}),
        ))
        .await
        .expect("revoke invitation response");
    assert_eq!(revoked_response.status(), StatusCode::OK);
    let revoked = response_json(revoked_response).await;
    assert_eq!(revoked["status"], "revoked");
    assert_eq!(revoked["version"], 2);

    let replay_response = application
        .clone()
        .oneshot(request(
            "POST",
            &revoke_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"expectedVersion":1}),
        ))
        .await
        .expect("replayed revoke response");
    assert_eq!(replay_response.status(), StatusCode::OK);
    assert_eq!(response_json(replay_response).await["version"], 2);

    let audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE target_id = $1 AND action IN ('project.invitation.issue', 'project.invitation.revoke')",
    )
    .bind(invitation_id)
    .fetch_one(&pool)
    .await
    .expect("invitation audit count");
    assert_eq!(audit_count, 2);

    drop(application);
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated M7 schema");
}

#[tokio::test]
async fn invitation_acceptance_creates_or_extends_one_human_identity_atomically() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m7_acceptance_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M7 acceptance schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M7 acceptance migrations");

    let temporary = tempfile::tempdir().expect("temporary M7 acceptance root");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary.path().join("data").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary
                .path()
                .join("data/codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M7 acceptance configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M7 acceptance fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "M7 Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        OWNER_PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M7 acceptance owner");
    let application = test_app(pool.clone(), config);
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN, OWNER_PASSWORD).await;
    let invitations_uri = format!("/api/v1/projects/{}/invitations", seed.project_id);

    let new_user_invitation = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"role":"viewer","expiresInHours":24}),
        ))
        .await
        .expect("new-user invitation response");
    assert_eq!(new_user_invitation.status(), StatusCode::CREATED);
    let new_user_invitation = response_json(new_user_invitation).await;
    let new_user_token = new_user_invitation["invitationUrl"]
        .as_str()
        .expect("new-user invitation URL")
        .split_once("#token=")
        .expect("new-user token fragment")
        .1
        .to_owned();
    let new_user_invitation_id = new_user_invitation["id"]
        .as_str()
        .expect("new-user invitation ID")
        .parse::<Uuid>()
        .expect("new-user invitation UUID");

    let resolved_response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/project-invitations/resolve",
            &[("origin", "http://127.0.0.1:3100")],
            json!({"token":new_user_token}),
        ))
        .await
        .expect("resolve invitation response");
    assert_eq!(resolved_response.status(), StatusCode::OK);
    let resolved = response_json(resolved_response).await;
    assert_eq!(resolved["organizationName"], "Mischa Personal");
    assert_eq!(resolved["projectName"], "Agent Workspace");
    assert_eq!(resolved["role"], "viewer");
    assert!(resolved.get("id").is_none());
    assert!(resolved.get("tokenPrefix").is_none());

    let new_password = "new human correct horse battery staple";
    let accepted_new_response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/project-invitations/accept-new",
            &[("origin", "http://127.0.0.1:3100")],
            json!({
                "token":new_user_token,
                "loginName":"new.human",
                "displayName":"New Human",
                "handle":"new_human",
                "password":new_password,
            }),
        ))
        .await
        .expect("accept new-user invitation response");
    assert_eq!(accepted_new_response.status(), StatusCode::OK);
    let (new_user_cookie, new_user_csrf) = response_cookies(&accepted_new_response);
    let accepted_new = response_json(accepted_new_response).await;
    assert_eq!(accepted_new["authenticated"], true);
    assert_eq!(accepted_new["user"]["loginName"], "new.human");
    assert_eq!(accepted_new["organizations"][0]["role"], "member");
    assert!(!new_user_cookie.is_empty());
    assert!(!new_user_csrf.is_empty());

    let accepted_shape =
        sqlx::query_as::<_, (String, String, String, String, String, String, i64, String)>(
            r#"
        SELECT user_account.login_name, user_account.display_name,
               organization_membership.role, project_membership.role,
               project_membership.handle, invitation.status,
               invitation.version, credential.password_phc
        FROM users user_account
        JOIN user_password_credentials credential ON credential.user_id = user_account.id
        JOIN organization_memberships organization_membership
          ON organization_membership.actor_id = user_account.actor_id
         AND organization_membership.organization_id = $1
        JOIN project_memberships project_membership
          ON project_membership.actor_id = user_account.actor_id
         AND project_membership.project_id = $2
        JOIN project_invitations invitation
          ON invitation.accepted_by_actor_id = user_account.actor_id
         AND invitation.id = $3
        WHERE user_account.login_name = 'new.human'
        "#,
        )
        .bind(seed.organization_id)
        .bind(seed.project_id)
        .bind(new_user_invitation_id)
        .fetch_one(&pool)
        .await
        .expect("accepted new-user shape");
    assert_eq!(accepted_shape.0, "new.human");
    assert_eq!(accepted_shape.1, "New Human");
    assert_eq!(accepted_shape.2, "member");
    assert_eq!(accepted_shape.3, "viewer");
    assert_eq!(accepted_shape.4, "new_human");
    assert_eq!(accepted_shape.5, "accepted");
    assert_eq!(accepted_shape.6, 2);
    assert!(
        accepted_shape
            .7
            .starts_with("$argon2id$v=19$m=65536,t=3,p=1$")
    );
    assert!(!accepted_shape.7.contains(new_password));

    for (uri, body) in [
        (
            "/api/v1/project-invitations/resolve",
            json!({"token":new_user_token}),
        ),
        (
            "/api/v1/project-invitations/accept-new",
            json!({
                "token":new_user_token,
                "loginName":"duplicate.human",
                "displayName":"Duplicate Human",
                "handle":"duplicate_human",
                "password":"duplicate correct horse battery staple",
            }),
        ),
    ] {
        let replay = application
            .clone()
            .oneshot(request(
                "POST",
                uri,
                &[("origin", "http://127.0.0.1:3100")],
                body,
            ))
            .await
            .expect("consumed invitation response");
        assert_eq!(replay.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE login_name LIKE '%.human'")
            .fetch_one(&pool)
            .await
            .expect("invited User count"),
        1
    );

    let existing_actor_id = Uuid::now_v7();
    let existing_user_id = Uuid::now_v7();
    let existing_password = "existing correct horse battery staple";
    let existing_password_phc = password::hash_password(existing_password.to_owned())
        .await
        .expect("hash existing-user password");
    let mut existing_transaction = pool.begin().await.expect("existing-user transaction");
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(existing_actor_id)
        .execute(&mut *existing_transaction)
        .await
        .expect("insert existing Actor");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, 'existing.human', 'Existing Human')",
    )
    .bind(existing_user_id)
    .bind(existing_actor_id)
    .execute(&mut *existing_transaction)
    .await
    .expect("insert existing User");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(existing_user_id)
        .bind(existing_password_phc)
        .execute(&mut *existing_transaction)
        .await
        .expect("insert existing password credential");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(seed.organization_id)
    .bind(existing_actor_id)
    .execute(&mut *existing_transaction)
    .await
    .expect("insert existing Organization membership");
    existing_transaction
        .commit()
        .await
        .expect("commit existing User fixture");
    let (existing_cookie, existing_csrf) =
        login(&application, "existing.human", existing_password).await;

    let existing_invitation_response = application
        .clone()
        .oneshot(request(
            "POST",
            &invitations_uri,
            &write_headers(&owner_cookie, &owner_csrf),
            json!({"role":"admin","expiresInHours":24}),
        ))
        .await
        .expect("existing-user invitation response");
    assert_eq!(existing_invitation_response.status(), StatusCode::CREATED);
    let existing_invitation = response_json(existing_invitation_response).await;
    let existing_token = existing_invitation["invitationUrl"]
        .as_str()
        .expect("existing-user invitation URL")
        .split_once("#token=")
        .expect("existing-user token fragment")
        .1
        .to_owned();
    let existing_invitation_id = existing_invitation["id"]
        .as_str()
        .expect("existing-user invitation ID")
        .parse::<Uuid>()
        .expect("existing-user invitation UUID");

    let missing_csrf = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/project-invitations/accept-existing",
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", existing_cookie.as_str()),
            ],
            json!({"token":existing_token,"handle":"existing_human"}),
        ))
        .await
        .expect("existing-user missing CSRF response");
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let accepted_existing_response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/project-invitations/accept-existing",
            &write_headers(&existing_cookie, &existing_csrf),
            json!({"token":existing_token,"handle":"existing_human"}),
        ))
        .await
        .expect("accept existing-user invitation response");
    assert_eq!(accepted_existing_response.status(), StatusCode::OK);
    let (rotated_cookie, rotated_csrf) = response_cookies(&accepted_existing_response);
    assert_ne!(rotated_cookie, existing_cookie);
    assert_ne!(rotated_csrf, existing_csrf);
    let accepted_existing = response_json(accepted_existing_response).await;
    assert_eq!(accepted_existing["user"]["loginName"], "existing.human");
    assert_eq!(accepted_existing["organizations"][0]["role"], "admin");

    let existing_shape = sqlx::query_as::<_, (String, i64, String, String, i64, bool)>(
        r#"
        SELECT organization_membership.role, organization_membership.version,
               project_membership.role, project_membership.handle,
               invitation.version,
               EXISTS (
                   SELECT 1 FROM sessions prior_session
                   JOIN sessions rotated_session
                     ON rotated_session.rotated_from_session_id = prior_session.id
                   WHERE prior_session.user_id = user_account.id
                     AND prior_session.revoked_at IS NOT NULL
               ) AS rotated
        FROM users user_account
        JOIN organization_memberships organization_membership
          ON organization_membership.actor_id = user_account.actor_id
         AND organization_membership.organization_id = $1
        JOIN project_memberships project_membership
          ON project_membership.actor_id = user_account.actor_id
         AND project_membership.project_id = $2
        JOIN project_invitations invitation
          ON invitation.accepted_by_actor_id = user_account.actor_id
         AND invitation.id = $3
        WHERE user_account.id = $4
        "#,
    )
    .bind(seed.organization_id)
    .bind(seed.project_id)
    .bind(existing_invitation_id)
    .bind(existing_user_id)
    .fetch_one(&pool)
    .await
    .expect("accepted existing-user shape");
    assert_eq!(existing_shape.0, "admin");
    assert_eq!(existing_shape.1, 2);
    assert_eq!(existing_shape.2, "admin");
    assert_eq!(existing_shape.3, "existing_human");
    assert_eq!(existing_shape.4, 2);
    assert!(existing_shape.5);

    let old_session = application
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/auth/session",
            &[("cookie", existing_cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("old session response");
    assert_eq!(response_json(old_session).await["authenticated"], false);
    let rotated_session = application
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/auth/session",
            &[("cookie", rotated_cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("rotated session response");
    assert_eq!(response_json(rotated_session).await["authenticated"], true);

    let acceptance_audits = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM audit_events
        WHERE action = 'project.invitation.accept'
          AND target_id IN ($1, $2)
          AND metadata::text NOT LIKE '%aw_inv_%'
        "#,
    )
    .bind(new_user_invitation_id)
    .bind(existing_invitation_id)
    .fetch_one(&pool)
    .await
    .expect("acceptance audit count");
    assert_eq!(acceptance_audits, 2);

    drop(application);
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated M7 acceptance schema");
}

fn test_app(pool: PgPool, config: AppConfig) -> axum::Router {
    let supervisors = SupervisorRegistry::new(&config).expect("M7 supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

async fn login(application: &axum::Router, login_name: &str, password: &str) -> (String, String) {
    let response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", "http://127.0.0.1:3100")],
            json!({"loginName":login_name,"password":password}),
        ))
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response_cookies(&response);
    let _ = response_json(response).await;
    cookies
}

fn response_cookies(response: &axum::response::Response) -> (String, String) {
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .expect("cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    (cookies.join("; "), csrf)
}

fn write_headers<'a>(cookie: &'a str, csrf: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie),
        ("x-csrf-token", csrf),
    ]
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: Value) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize request body"))
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read response body"),
    )
    .expect("parse response JSON")
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema-scoped M7 pool")
}
