//! W8 settings and accounts. The costly bugs here are silent: someone keeps
//! a workspace after being removed, an admin promotes themselves, a password
//! alone signs in when two-factor is on, a personal token reaches another
//! workspace, the audit log shows another workspace or a private
//! conversation, or a GitHub installation is linked by someone who only
//! knows its number.

use std::{fs::OpenOptions, io::Write as _, os::unix::fs::OpenOptionsExt as _};

use agent_workspace_server::{
    auth::password::hash_password,
    secrets::{self, FileKeyring},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, PASSWORD, initialize, send, test_app_with},
    mcp_admin::{Setup, create_project, setup},
};

const ORIGIN: &str = "http://127.0.0.1:3100";

/// Sign in with a password; the response, and the browser if it got a session.
async fn sign_in(app: &axum::Router, login_name: &str) -> (Value, Option<Browser>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
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
    session_from(response).await
}

async fn anonymous_post(app: &axum::Router, uri: &str, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("body")))
                .expect("request"),
        )
        .await
        .expect("response")
}

async fn session_from(response: axum::response::Response) -> (Value, Option<Browser>) {
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .filter(|cookie| !cookie.ends_with('='))
        .collect::<Vec<_>>();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let browser = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .filter(|_| {
            cookies
                .iter()
                .any(|cookie| cookie.starts_with("agent_workspace_session="))
        })
        .map(|csrf| Browser {
            cookie: cookies.join("; "),
            csrf: csrf.to_owned(),
        });
    (body, browser)
}

async fn get(app: &axum::Router, browser: &Browser, uri: &str) -> (StatusCode, Value) {
    send(app, browser.request("GET", uri, None, Value::Null)).await
}

/// Invite someone new to the workspace and have them join with a password.
async fn join_workspace(setup: &Setup, role: &str, login_name: &str) -> (Uuid, Browser) {
    let (status, issued) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!(
                "/api/v1/organizations/{}/invitations",
                setup.organization_id
            ),
            None,
            json!({ "role": role, "expiresInHours": 24 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let url = issued["invitationUrl"].as_str().expect("invitation URL");
    let token = url.split("#token=").nth(1).expect("token in fragment");
    assert!(url.contains("/join#token=aw_wsi_"), "{url}");
    let accept = json!({
        "token": token,
        "loginName": login_name,
        "displayName": login_name,
        "handle": login_name,
        "password": PASSWORD,
    });
    let response = anonymous_post(
        &setup.app,
        "/api/v1/workspace-invitations/accept-new",
        accept.clone(),
    )
    .await;
    assert!(response.status().is_success(), "{}", response.status());
    let (_, browser) = session_from(response).await;
    let browser = browser.expect("joined with a session");

    // An invitation works once.
    let mut again = accept;
    again["loginName"] = json!(format!("{login_name}2"));
    again["handle"] = json!(format!("{login_name}2"));
    let response = anonymous_post(
        &setup.app,
        "/api/v1/workspace-invitations/accept-new",
        again,
    )
    .await;
    assert!(response.status().is_client_error(), "{}", response.status());

    let (status, session) = get(&setup.app, &browser, "/api/v1/auth/session").await;
    assert_eq!(status, StatusCode::OK);
    let actor_id = session["user"]["actorId"]
        .as_str()
        .or_else(|| session["actorId"].as_str())
        .unwrap_or_else(|| panic!("actor in {session}"))
        .parse()
        .expect("actor ID");
    (actor_id, browser)
}

async fn member(setup: &Setup, actor_id: Uuid) -> Value {
    let (status, members) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/organizations/{}/members", setup.organization_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    members["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|member| member["actorId"] == json!(actor_id))
        .cloned()
        .unwrap_or_else(|| panic!("{actor_id} in {members}"))
}

async fn patch_member(
    setup: &Setup,
    browser: &Browser,
    actor_id: Uuid,
    version: i64,
    change: Value,
) -> (StatusCode, Value) {
    let mut body = change;
    body["expectedVersion"] = json!(version);
    send(
        &setup.app,
        browser.request(
            "PATCH",
            &format!(
                "/api/v1/organizations/{}/members/{actor_id}",
                setup.organization_id
            ),
            None,
            body,
        ),
    )
    .await
}

async fn owner_actor(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT actor_id FROM users WHERE login_name = 'owner'")
        .fetch_one(pool)
        .await
        .expect("owner actor")
}

/// A second workspace with its own owner, signed in as `other`.
async fn other_workspace(pool: &PgPool) -> Uuid {
    let organization = Uuid::now_v7();
    let actor = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("other actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Other', $2)")
        .bind(organization)
        .bind(format!("other-{}", Uuid::now_v7().simple()))
        .execute(pool)
        .await
        .expect("other organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(organization)
    .bind(actor)
    .execute(pool)
    .await
    .expect("other membership");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, 'other', 'Other')",
    )
    .bind(user)
    .bind(actor)
    .execute(pool)
    .await
    .expect("other user");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user)
        .bind(
            hash_password(PASSWORD.to_owned())
                .await
                .expect("hash password"),
        )
        .execute(pool)
        .await
        .expect("other password");
    organization
}

#[tokio::test]
async fn workspace_roles_are_explicit_and_removal_takes_everything_away() {
    let Some(setup) = setup("w8_roles").await else {
        return;
    };
    let project = create_project(&setup, "WORK", "Work").await;
    let owner = owner_actor(&setup.pool).await;
    let (mira, mira_browser) = join_workspace(&setup, "member", "mira").await;
    assert_eq!(member(&setup, mira).await["role"], "member");
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/members"),
            Some("add-mira"),
            json!({ "actorId": mira, "role": "member" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (status, _) = get(
        &setup.app,
        &mira_browser,
        &format!("/api/v1/projects/{project}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A member can't invite, change roles or read the audit log.
    let org = setup.organization_id;
    let (status, _) = send(
        &setup.app,
        mira_browser.request(
            "POST",
            &format!("/api/v1/organizations/{org}/invitations"),
            None,
            json!({ "role": "member", "expiresInHours": 24 }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let owner_row = member(&setup, owner).await;
    let (status, _) = patch_member(
        &setup,
        &mira_browser,
        owner,
        owner_row["version"].as_i64().expect("version"),
        json!({ "enabled": false }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let (status, _) = get(
        &setup.app,
        &mira_browser,
        &format!("/api/v1/organizations/{org}/audit-events"),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    // The owner makes them an admin. An admin still can't touch the owner,
    // change their own role or promote anyone.
    let row = member(&setup, mira).await;
    let (status, body) = patch_member(
        &setup,
        &setup.browser,
        mira,
        row["version"].as_i64().expect("version"),
        json!({ "role": "admin" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = member(&setup, mira).await;
    assert_eq!(row["role"], "admin");
    let (status, _) = patch_member(
        &setup,
        &mira_browser,
        mira,
        row["version"].as_i64().expect("version"),
        json!({ "role": "owner" }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let owner_row = member(&setup, owner).await;
    let (status, _) = patch_member(
        &setup,
        &mira_browser,
        owner,
        owner_row["version"].as_i64().expect("version"),
        json!({ "role": "member" }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert_eq!(member(&setup, owner).await["role"], "owner");

    // The role sticks: syncing project roles doesn't undo it.
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT role_explicit FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
        )
        .bind(org)
        .bind(mira)
        .fetch_one(&setup.pool)
        .await
        .expect("explicit role")
    );

    // The owner removes them: the workspace and its projects are gone for
    // them at once.
    let row = member(&setup, mira).await;
    let (status, body) = patch_member(
        &setup,
        &setup.browser,
        mira,
        row["version"].as_i64().expect("version"),
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = get(
        &setup.app,
        &mira_browser,
        &format!("/api/v1/organizations/{org}"),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let (status, _) = get(
        &setup.app,
        &mira_browser,
        &format!("/api/v1/projects/{project}"),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let still_member = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(mira)
    .fetch_one(&setup.pool)
    .await
    .expect("memberships");
    assert_eq!(still_member, 0);

    // Nobody can remove the last owner, including themselves.
    let owner_row = member(&setup, owner).await;
    let (status, _) = patch_member(
        &setup,
        &setup.browser,
        owner,
        owner_row["version"].as_i64().expect("version"),
        json!({ "enabled": false }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    // Each change is in the workspace's audit log.
    let (status, log) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/organizations/{org}/audit-events"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{log}");
    let actions = log["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|event| event["action"].as_str())
        .collect::<Vec<_>>();
    assert!(
        actions
            .iter()
            .any(|action| action.starts_with("workspace.invitation")),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"workspace_membership.removed"),
        "{actions:?}"
    );
}

fn base32_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0_u64;
    let mut count = 0;
    let mut bytes = Vec::new();
    for character in text.trim_end_matches('=').bytes() {
        let value = ALPHABET
            .iter()
            .position(|candidate| *candidate == character.to_ascii_uppercase())
            .expect("base32 character") as u64;
        bits = (bits << 5) | value;
        count += 5;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    bytes
}

/// The six-digit code an authenticator app shows at `offset` steps from now.
fn totp(secret: &str, offset: i64) -> String {
    use aws_lc_rs::hmac;
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &base32_decode(secret));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let step = (now / 30 + offset) as u64;
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    let at = (digest[digest.len() - 1] & 0x0f) as usize;
    let value = u32::from_be_bytes([
        digest[at] & 0x7f,
        digest[at + 1],
        digest[at + 2],
        digest[at + 3],
    ]) % 1_000_000;
    format!("{value:06}")
}

#[tokio::test]
async fn with_two_factor_on_a_password_alone_never_signs_in() {
    let Some(setup) = setup("w8_two_factor").await else {
        return;
    };
    // Two-factor secrets are sealed with the secrets keyring.
    let keys = tempfile::tempdir().expect("key directory");
    let keyring = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring, &FileKeyring::generate()).expect("keyring");
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let app = test_app_with(
        setup.pool.clone(),
        &database_url,
        &[(
            "AGENT_WORKSPACE_SECRETS_KEY_PATH",
            keyring.to_string_lossy().into_owned(),
        )],
    );
    let setup = Setup { app, ..setup };
    let (status, started) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/account/two-factor/setup",
            None,
            json!({ "currentPassword": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let secret = started["secret"].as_str().expect("secret").to_owned();
    assert!(
        started["otpauthUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("otpauth://totp/"))
    );
    assert!(
        started["qrSvg"]
            .as_str()
            .is_some_and(|svg| svg.contains("<svg") && !svg.contains("<script"))
    );

    // A wrong code doesn't turn it on.
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/account/two-factor/confirm",
            None,
            json!({ "code": "abcdef" }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let (status, confirmed) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/account/two-factor/confirm",
            None,
            json!({ "code": totp(&secret, 0) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    let recovery = confirmed["recoveryCodes"]
        .as_array()
        .expect("recovery codes")
        .iter()
        .map(|code| code.as_str().expect("code").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(recovery.len(), 10);

    // The password alone gets a challenge, not a session.
    let (challenge, browser) = sign_in(&setup.app, "owner").await;
    assert!(browser.is_none(), "a session without the code: {challenge}");
    assert_eq!(challenge["twoFactorRequired"], true);
    let challenge_token = challenge["challenge"].as_str().expect("challenge");

    let response = anonymous_post(
        &setup.app,
        "/api/v1/auth/login/two-factor",
        json!({ "challenge": challenge_token, "code": "999999" }),
    )
    .await;
    // (A one-in-a-million chance the wrong code is right; use the code for
    // a step that can't be current to rule it out.)
    let wrong_status = response.status();
    let (_, wrong_browser) = session_from(response).await;
    if totp(&secret, 0) != "999999" && totp(&secret, 1) != "999999" && totp(&secret, -1) != "999999"
    {
        assert!(wrong_status.is_client_error(), "{wrong_status}");
        assert!(wrong_browser.is_none());
    }

    // The next step's code works (the current one was used to confirm and
    // can't be replayed).
    let response = anonymous_post(
        &setup.app,
        "/api/v1/auth/login/two-factor",
        json!({ "challenge": challenge_token, "code": totp(&secret, 1) }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, browser) = session_from(response).await;
    let browser = browser.expect("session after the code");
    let (status, _) = get(&setup.app, &browser, "/api/v1/account/security").await;
    assert_eq!(status, StatusCode::OK);

    // A challenge is used once.
    let response = anonymous_post(
        &setup.app,
        "/api/v1/auth/login/two-factor",
        json!({ "challenge": challenge_token, "code": recovery[0] }),
    )
    .await;
    assert!(response.status().is_client_error());

    // A recovery code signs in once.
    let (challenge, _) = sign_in(&setup.app, "owner").await;
    let token = challenge["challenge"].as_str().expect("challenge");
    let response = anonymous_post(
        &setup.app,
        "/api/v1/auth/login/two-factor",
        json!({ "challenge": token, "code": recovery[0] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (challenge, _) = sign_in(&setup.app, "owner").await;
    let token = challenge["challenge"].as_str().expect("challenge");
    let response = anonymous_post(
        &setup.app,
        "/api/v1/auth/login/two-factor",
        json!({ "challenge": token, "code": recovery[0] }),
    )
    .await;
    assert!(response.status().is_client_error());

    // The factor's secret is sealed, never stored as written.
    let stored = sqlx::query_scalar::<_, Vec<u8>>("SELECT ciphertext FROM user_totp_factors")
        .fetch_one(&setup.pool)
        .await
        .expect("factor");
    assert!(
        !stored
            .windows(secret.len().min(16))
            .any(|window| window == &secret.as_bytes()[..secret.len().min(16)])
    );
}

#[tokio::test]
async fn personal_tokens_reach_only_the_person_s_own_projects() {
    let Some(setup) = setup("w8_tokens").await else {
        return;
    };
    let project = create_project(&setup, "WORK", "Work").await;
    let other = other_workspace(&setup.pool).await;
    let (_, other_browser) = sign_in(&setup.app, "other").await;
    let other_browser = other_browser.expect("other session");
    let other_setup = Setup {
        app: setup.app.clone(),
        browser: other_browser,
        organization_id: other,
        pool: setup.pool.clone(),
    };
    let foreign = create_project(&other_setup, "ELSE", "Elsewhere").await;

    // Not for a project in someone else's workspace.
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/account/api-tokens",
            None,
            json!({ "projectId": foreign, "name": "laptop", "scopes": ["read"], "expiresInDays": 30 }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    let (status, created) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/account/api-tokens",
            None,
            json!({ "projectId": project, "name": "laptop", "scopes": ["read"], "expiresInDays": 30 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().expect("token").to_owned();
    assert!(token.starts_with("aw_mcp_"));
    let credential = created["credential"]["id"]
        .as_str()
        .expect("credential")
        .to_owned();
    let version = created["credential"]["version"].as_i64().expect("version");
    assert!(initialize(&setup.app, &token).await.is_some());

    // The listing never shows the token again, and the other person never
    // sees it.
    let (_, listed) = get(&setup.app, &setup.browser, "/api/v1/account/api-tokens").await;
    assert!(!listed.to_string().contains(&token));
    assert_eq!(listed["tokens"].as_array().expect("tokens").len(), 1);
    let (_, theirs) = get(
        &setup.app,
        &other_setup.browser,
        "/api/v1/account/api-tokens",
    )
    .await;
    assert!(theirs["tokens"].as_array().expect("tokens").is_empty());
    let (status, _) = send(
        &setup.app,
        other_setup.browser.request(
            "POST",
            &format!("/api/v1/account/api-tokens/{credential}/revoke"),
            None,
            json!({ "expectedVersion": version }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert!(initialize(&setup.app, &token).await.is_some());

    // Revoked, it stops working.
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/account/api-tokens/{credential}/revoke"),
            None,
            json!({ "expectedVersion": version }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(initialize(&setup.app, &token).await.is_none());
}

#[tokio::test]
async fn the_audit_log_shows_one_workspace_and_hides_private_conversations() {
    let Some(setup) = setup("w8_audit").await else {
        return;
    };
    let org = setup.organization_id;
    let owner = owner_actor(&setup.pool).await;
    let other = other_workspace(&setup.pool).await;
    let hidden_conversation = Uuid::now_v7();
    let visible = Uuid::now_v7();
    let private = Uuid::now_v7();
    let private_by_metadata = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    for (id, organization, target_kind, target, metadata) in [
        (visible, org, "workspace", Some(org), json!({})),
        (
            private,
            org,
            "conversation",
            Some(hidden_conversation),
            json!({}),
        ),
        (
            private_by_metadata,
            org,
            "agent_run",
            None,
            json!({ "conversation_id": hidden_conversation }),
        ),
        (foreign, other, "workspace", None, json!({})),
    ] {
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, organization_id, authentication_class, action,
                target_kind, target_id, outcome, metadata, channel
            ) VALUES ($1, $2, $3, 'system', 'test.event', $4, $5, 'succeeded', $6, 'system')
            "#,
        )
        .bind(id)
        .bind(owner)
        .bind(organization)
        .bind(target_kind)
        .bind(target)
        .bind(metadata)
        .execute(&setup.pool)
        .await
        .expect("audit event");
    }

    let (status, log) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/organizations/{org}/audit-events?kind=test"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{log}");
    let ids = log["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![visible.to_string()], "{log}");

    // Another workspace's log isn't readable at all.
    let (status, _) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/organizations/{}/audit-events", other),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
}

fn write_secret(path: &std::path::Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("create secret fixture");
    file.write_all(bytes).expect("write secret fixture");
}

#[tokio::test]
async fn github_installations_need_a_claim_and_the_state_needs_its_browser() {
    let Some(setup) = setup("w8_github").await else {
        return;
    };
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let secrets = tempfile::tempdir().expect("secret directory");
    let private_key = secrets.path().join("github-app.pem");
    let webhook_secret = secrets.path().join("github-webhook-secret");
    let client_secret = secrets.path().join("github-client-secret");
    write_secret(&private_key, b"test-private-key-placeholder");
    write_secret(&webhook_secret, b"webhook-secret-for-tests-only");
    write_secret(&client_secret, b"client-secret-for-tests-only");
    let app = test_app_with(
        setup.pool.clone(),
        &database_url,
        &[
            ("AGENT_WORKSPACE_GITHUB_APP_ID", "12345".to_owned()),
            (
                "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH",
                private_key.to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH",
                webhook_secret.to_string_lossy().into_owned(),
            ),
            ("AGENT_WORKSPACE_GITHUB_APP_SLUG", "aw-test".to_owned()),
            (
                "AGENT_WORKSPACE_GITHUB_CLIENT_ID",
                "Iv1.testclient".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_GITHUB_CLIENT_SECRET_PATH",
                client_secret.to_string_lossy().into_owned(),
            ),
        ],
    );
    let setup = Setup { app, ..setup };
    let project = create_project(&setup, "WORK", "Work").await;

    let (status, connection) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/projects/{project}/integrations/github/connection"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{connection}");
    assert_eq!(connection["connectAvailable"], true);
    assert_eq!(connection["installations"], json!([]));

    // Start connecting: a GitHub URL, and a cookie only this browser holds.
    let response = setup
        .app
        .clone()
        .oneshot(setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/integrations/github/connect"),
            None,
            json!({ "mode": "install" }),
        ))
        .await
        .expect("connect");
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("aw_github_connect="))
        .expect("connect cookie")
        .to_owned();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
    let cookie = cookie.split(';').next().expect("cookie pair").to_owned();
    let bytes = to_bytes(response.into_body(), 100_000).await.expect("body");
    let started: Value = serde_json::from_slice(&bytes).expect("JSON");
    let url = started["url"].as_str().expect("URL");
    assert!(
        url.starts_with("https://github.com/apps/aw-test/installations/new?state="),
        "{url}"
    );
    let state = url.split("state=").nth(1).expect("state");

    let callback = |query: String, cookie: Option<String>| {
        let app = setup.app.clone();
        async move {
            let mut request = Request::builder()
                .method("GET")
                .uri(format!("/api/v1/integrations/github/callback?{query}"));
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            let response = app
                .oneshot(request.body(Body::empty()).expect("request"))
                .await
                .expect("callback");
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .expect("location")
                .to_owned()
        }
    };
    let unused = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM github_connect_attempts WHERE used_at IS NULL",
        )
        .fetch_one(&setup.pool)
        .await
        .expect("attempts")
    };

    // Someone else's browser (no cookie, or a different one) gets nowhere
    // and doesn't even use the attempt up.
    let query = format!("state={state}&code=abc&installation_id=42&setup_action=install");
    assert_eq!(
        callback(query.clone(), None).await,
        "/settings?github=error&reason=expired"
    );
    assert_eq!(
        callback(
            query.clone(),
            Some("aw_github_connect=someone-elses-browser".to_owned())
        )
        .await,
        "/settings?github=error&reason=expired"
    );
    assert_eq!(unused().await, 1);

    // The right browser, but GitHub said no: the attempt is used up.
    assert_eq!(
        callback(
            format!("state={state}&error=access_denied"),
            Some(cookie.clone())
        )
        .await,
        format!("/settings/projects/{project}/repositories?github=error&reason=denied")
    );
    assert_eq!(unused().await, 0);
    assert_eq!(
        callback(query, Some(cookie)).await,
        "/settings?github=error&reason=expired"
    );

    // No claims were made by any of that.
    let claims = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM github_installation_claims")
        .fetch_one(&setup.pool)
        .await
        .expect("claims");
    assert_eq!(claims, 0);

    // An installation another workspace claimed can't be linked or used
    // here, by typing its number or through the picker, and nothing is
    // asked of GitHub on the way.
    let other = other_workspace(&setup.pool).await;
    sqlx::query(
        "INSERT INTO github_installation_claims (organization_id, installation_id, account_login, target_type) VALUES ($1, 42, 'someone', 'Organization')",
    )
    .bind(other)
    .execute(&setup.pool)
    .await
    .expect("claim");
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/integrations/github/repositories"),
            None,
            json!({ "installationId": 42, "githubRepositoryId": 7 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let repository = super::mcp_task_board::insert_repository(
        &setup,
        &project,
        "acme/widgets",
        true,
        true,
        None,
    )
    .await;
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/integrations/github/sync"),
            None,
            json!({ "installationId": 42, "repositoryIds": [repository] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (_, connection) = get(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/projects/{project}/integrations/github/connection"),
    )
    .await;
    assert_eq!(connection["installations"], json!([]));
}
