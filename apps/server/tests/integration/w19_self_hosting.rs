//! W19: email for people running their own server. The costly failures are
//! silent: a reset link that works twice, outlives its 30 minutes or a
//! password change, or skips two-factor; a reset request that says whether
//! an account exists; mail sent to an address nobody confirmed; the SMTP
//! password stored or returned in the clear; and email reaching someone who
//! chose a digest, or who lost access to what it's about.

use std::sync::{Arc, Mutex};

use agent_workspace_server::{
    jobs::{self, WorkerOptions},
    notification_email, password_reset,
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, PASSWORD, send},
    mcp_admin::{Setup, create_project},
    message_edits::{add_person, copies, create_channel, owner_id, say},
    notifications::drain,
    w9_capacity_and_push::{Fixture, call, fixture},
};

const SMTP_PASSWORD: &str = "w19-relay-password-do-not-store";
const ORIGIN: &str = "http://127.0.0.1:3100";

#[derive(Clone, Debug)]
struct Captured {
    to: String,
    auth: Option<String>,
    data: String,
}

impl Captured {
    fn subject(&self) -> &str {
        self.data
            .lines()
            .find_map(|line| line.strip_prefix("Subject: "))
            .unwrap_or_default()
    }

    fn body(&self) -> String {
        let encoded = self
            .data
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .replace("\r\n", "");
        String::from_utf8(
            STANDARD
                .decode(encoded.trim_end_matches('.'))
                .expect("base64 body"),
        )
        .expect("utf-8 body")
    }

    /// The token in a `#token=` link.
    fn token(&self) -> String {
        let body = self.body();
        let start = body.find("#token=").expect("a link with a token") + "#token=".len();
        body[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect()
    }
}

type Inbox = Arc<Mutex<Vec<Captured>>>;

/// A mail server on this machine that accepts everything and keeps it.
async fn fake_smtp() -> (u16, Inbox) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp");
    let port = listener.local_addr().expect("address").port();
    let inbox: Inbox = Arc::default();
    let kept = Arc::clone(&inbox);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read);
                let _ = write.write_all(b"220 fake ready\r\n").await;
                let mut to = String::new();
                let mut auth = None;
                let mut line = String::new();
                loop {
                    line.clear();
                    if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let command = line.trim_end().to_owned();
                    let upper = command.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
                    } else if let Some(token) = command.strip_prefix("AUTH PLAIN ") {
                        let decoded = STANDARD.decode(token).expect("auth plain");
                        auth = Some(String::from_utf8_lossy(&decoded).replace('\0', "|"));
                        b"235 ok\r\n"
                    } else if let Some(rest) = upper.strip_prefix("RCPT TO:<") {
                        to = command["RCPT TO:<".len()..command.len() - 1].to_owned();
                        let _ = rest;
                        b"250 ok\r\n"
                    } else if upper.starts_with("MAIL FROM") {
                        b"250 ok\r\n"
                    } else if upper == "DATA" {
                        let _ = write.write_all(b"354 go on\r\n").await;
                        let mut data = String::new();
                        loop {
                            line.clear();
                            if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                                return;
                            }
                            if line == ".\r\n" {
                                break;
                            }
                            data.push_str(&line);
                        }
                        kept.lock().expect("inbox").push(Captured {
                            to: to.clone(),
                            auth: auth.clone(),
                            data,
                        });
                        b"250 queued\r\n"
                    } else if upper == "QUIT" {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"502 no\r\n"
                    };
                    let _ = write.write_all(reply).await;
                }
            });
        }
    });
    (port, inbox)
}

fn mail_count(inbox: &Inbox) -> usize {
    inbox.lock().expect("inbox").len()
}

fn last_mail(inbox: &Inbox) -> Captured {
    inbox
        .lock()
        .expect("inbox")
        .last()
        .cloned()
        .expect("an email")
}

/// Run every queued job now, including delayed ones.
async fn run_jobs(fixture: &Fixture) {
    sqlx::query("UPDATE background_jobs SET run_at = now() WHERE state = 'queued'")
        .execute(&fixture.setup.pool)
        .await
        .expect("make jobs due");
    jobs::run_due(
        &fixture.state,
        &jobs::default_registry(),
        &WorkerOptions::default(),
    )
    .await
    .expect("run jobs");
}

async fn set_up_mail(setup: &Setup, port: u16) -> Value {
    let (status, body) = call(
        setup,
        &setup.browser,
        "PUT",
        &format!("/api/v1/organizations/{}/mail", setup.organization_id),
        None,
        json!({
            "provider": "smtp",
            "smtp": {"host": "127.0.0.1", "port": port, "security": "none", "username": "relay"},
            "fromAddress": "workspace@example.com",
            "fromName": "W19 Workspace",
            "password": SMTP_PASSWORD,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// A request with no session, as from a signed-out browser.
async fn public(setup: &Setup, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", ORIGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(serde_json::to_vec(&body).expect("body"))
            })
            .expect("request"),
    )
    .await
}

async fn sign_in(setup: &Setup, login: &str, password: &str) -> StatusCode {
    public(
        setup,
        "POST",
        "/api/v1/auth/login",
        json!({"loginName": login, "password": password}),
    )
    .await
    .0
}

async fn user_of(setup: &Setup, actor_id: Uuid) -> (Uuid, i64) {
    sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, authentication_epoch FROM users WHERE actor_id = $1",
    )
    .bind(actor_id)
    .fetch_one(&setup.pool)
    .await
    .expect("user")
}

/// A reset link made directly, for the expiry and epoch cases.
async fn plant_reset(setup: &Setup, user_id: Uuid, epoch: i64, expires: &str) -> String {
    let token = format!("aw_reset_{}", Uuid::now_v7().simple());
    sqlx::query(&format!(
        "INSERT INTO password_reset_tokens (id, user_id, token_digest, authentication_epoch, expires_at) \
         VALUES ($1, $2, $3, $4, now() + interval '{expires}')"
    ))
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(password_reset::digest(&token).to_vec())
    .bind(epoch)
    .execute(&setup.pool)
    .await
    .expect("plant reset");
    token
}

async fn inspect(setup: &Setup, token: &str) -> (StatusCode, Value) {
    public(
        setup,
        "POST",
        "/api/v1/auth/password-reset/inspect",
        json!({"token": token}),
    )
    .await
}

async fn complete(setup: &Setup, token: &str, password: &str, code: Option<&str>) -> StatusCode {
    let mut body = json!({"token": token, "newPassword": password});
    if let Some(code) = code {
        body["code"] = json!(code);
    }
    public(setup, "POST", "/api/v1/auth/password-reset/complete", body)
        .await
        .0
}

#[tokio::test]
async fn mail_settings_confirmations_and_reset_links_follow_their_rules() {
    let Some(fixture) = fixture("w19_reset").await else {
        return;
    };
    let setup = &fixture.setup;
    let (port, inbox) = fake_smtp().await;
    let organization = setup.organization_id;

    // Nothing to offer before mail is set up.
    let (_, available) = public(setup, "GET", "/api/v1/auth/password-reset", Value::Null).await;
    assert_eq!(available["available"], false);

    // The password is stored encrypted and never returned.
    let saved = set_up_mail(setup, port).await;
    assert_eq!(saved["passwordSet"], true);
    assert!(!saved.to_string().contains(SMTP_PASSWORD));
    let stored = sqlx::query_scalar::<_, String>(
        "SELECT provider_config::text FROM workspace_mail_settings WHERE organization_id = $1",
    )
    .bind(organization)
    .fetch_one(&setup.pool)
    .await
    .expect("stored settings");
    assert!(!stored.contains(SMTP_PASSWORD));
    for table in [
        "secrets",
        "workspace_mail_settings",
        "audit_events",
        "background_jobs",
    ] {
        assert_eq!(
            copies(&setup.pool, table, SMTP_PASSWORD).await,
            0,
            "{table}"
        );
    }

    // Plain SMTP is only for a relay on this machine.
    let (status, _) = call(
        setup,
        &setup.browser,
        "PUT",
        &format!("/api/v1/organizations/{organization}/mail"),
        None,
        json!({
            "provider": "smtp",
            "smtp": {"host": "mail.example.com", "port": 25, "security": "none", "username": null},
            "fromAddress": "workspace@example.com",
            "fromName": null,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A test reaches the server, signed in with the stored password.
    let (status, test) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/organizations/{organization}/mail/test"),
        None,
        json!({"to": "someone@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{test}");
    assert_eq!(test["ok"], true, "{test}");
    let sent = last_mail(&inbox);
    assert_eq!(sent.to, "someone@example.com");
    assert_eq!(
        sent.auth.as_deref(),
        Some(format!("|relay|{SMTP_PASSWORD}").as_str())
    );
    assert!(sent.subject().starts_with("Test email from"));

    // Members can't see or change how the workspace sends mail.
    let project = create_project(setup, "RST", "Reset").await;
    let (mara_id, mara) = add_person(setup, &[&project], "mara").await;
    let (status, _) = call(
        setup,
        &mara,
        "GET",
        &format!("/api/v1/organizations/{organization}/mail"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        setup,
        &mara,
        "GET",
        &format!("/api/v1/organizations/{organization}/server"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An address gets mail only once confirmed.
    let owner = owner_id(&setup.pool).await;
    let (owner_user, _) = user_of(setup, owner).await;
    let (status, email) = call(
        setup,
        &setup.browser,
        "PUT",
        "/api/v1/account/email",
        None,
        json!({"address": "Owner@Example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{email}");
    assert_eq!(email["verified"], false);
    run_jobs(&fixture).await;
    let confirmation = last_mail(&inbox);
    assert_eq!(confirmation.to, "Owner@example.com");
    let token = confirmation.token();
    assert!(token.starts_with("aw_email_"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM background_jobs WHERE payload::text LIKE '%' || $1 || '%'"
        )
        .bind(&token)
        .fetch_one(&setup.pool)
        .await
        .expect("jobs"),
        0,
        "no job holds the link"
    );
    // Before confirming, a reset for the owner sends nothing.
    let before = mail_count(&inbox);
    let (status, _) = public(
        setup,
        "POST",
        "/api/v1/auth/password-reset",
        json!({"login": "owner"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    run_jobs(&fixture).await;
    assert_eq!(mail_count(&inbox), before);
    let (status, verified) = public(
        setup,
        "POST",
        "/api/v1/auth/verify-email",
        json!({"token": token}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{verified}");
    let (status, _) = public(
        setup,
        "POST",
        "/api/v1/auth/verify-email",
        json!({"token": token}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a confirmation link works once"
    );

    // Asking for a reset says the same thing whether or not the account exists.
    let (_, available) = public(setup, "GET", "/api/v1/auth/password-reset", Value::Null).await;
    assert_eq!(available["available"], true);
    let before = mail_count(&inbox);
    let (unknown_status, unknown) = public(
        setup,
        "POST",
        "/api/v1/auth/password-reset",
        json!({"login": "nobody-here"}),
    )
    .await;
    let (known_status, known) = public(
        setup,
        "POST",
        "/api/v1/auth/password-reset",
        json!({"login": "owner@example.com"}),
    )
    .await;
    assert_eq!(unknown_status, known_status);
    assert_eq!(unknown, known);
    run_jobs(&fixture).await;
    assert_eq!(
        mail_count(&inbox),
        before + 1,
        "only the real account gets mail"
    );
    let reset = last_mail(&inbox);
    assert_eq!(reset.to, "Owner@example.com");
    let link = reset.token();
    assert!(link.starts_with("aw_reset_"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM password_reset_tokens WHERE token_digest = $1"
        )
        .bind(Sha256::digest(link.as_bytes()).to_vec())
        .fetch_one(&setup.pool)
        .await
        .expect("digest"),
        1,
        "only the digest is stored"
    );

    // Asking again right away is throttled: no second email.
    let before = mail_count(&inbox);
    public(
        setup,
        "POST",
        "/api/v1/auth/password-reset",
        json!({"login": "owner"}),
    )
    .await;
    run_jobs(&fixture).await;
    assert_eq!(mail_count(&inbox), before);

    let (status, seen) = inspect(setup, &link).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["loginName"], "owner");
    assert_eq!(seen["twoFactorRequired"], false);
    assert_eq!(
        complete(setup, &link, "short", None).await,
        StatusCode::BAD_REQUEST
    );

    // Resetting signs out everywhere, and the link then stops working.
    let new_password = "a brand new password for w19";
    assert_eq!(
        complete(setup, &link, new_password, None).await,
        StatusCode::NO_CONTENT
    );
    let (_, session) = call(
        setup,
        &setup.browser,
        "GET",
        "/api/v1/auth/session",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false, "{session}");
    assert_eq!(
        complete(setup, &link, "another password entirely", None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sign_in(setup, "owner", PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(sign_in(setup, "owner", new_password).await, StatusCode::OK);

    // Expired links and links from before a sign-out everywhere don't work.
    let (_, epoch) = user_of(setup, owner).await;
    let expired = plant_reset(setup, owner_user, epoch, "-1 minute").await;
    assert_eq!(inspect(setup, &expired).await.0, StatusCode::NOT_FOUND);
    let stale = plant_reset(setup, owner_user, epoch - 1, "30 minutes").await;
    assert_eq!(inspect(setup, &stale).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        complete(setup, &stale, "yet another long password", None).await,
        StatusCode::NOT_FOUND
    );

    // With two-factor on, a mailbox alone isn't enough.
    sqlx::query(
        r#"
        INSERT INTO user_totp_factors (user_id, ciphertext, nonce, wrapped_key, master_key_id, confirmed_at)
        VALUES ($1, $2, $3, $4, 'test-key', now())
        "#,
    )
    .bind(owner_user)
    .bind(vec![7_u8; 32])
    .bind(vec![1_u8; 12])
    .bind(vec![2_u8; 40])
    .execute(&setup.pool)
    .await
    .expect("two-factor factor");
    sqlx::query("INSERT INTO user_recovery_codes (id, user_id, code_digest) VALUES ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(owner_user)
        .bind(Sha256::digest(format!("aw-recovery:v1:{owner_user}:abcde12345").as_bytes()).to_vec())
        .execute(&setup.pool)
        .await
        .expect("recovery code");
    let guarded = plant_reset(setup, owner_user, epoch, "30 minutes").await;
    let (_, seen) = inspect(setup, &guarded).await;
    assert_eq!(seen["twoFactorRequired"], true);
    let other_password = "the password after two-factor";
    assert_eq!(
        complete(setup, &guarded, other_password, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        complete(setup, &guarded, other_password, Some("zzzzz-99999")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        complete(setup, &guarded, other_password, Some("abcde-12345")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        sign_in(setup, "owner", other_password).await,
        StatusCode::OK
    );
    // Too many wrong codes end the link.
    let (_, epoch) = user_of(setup, owner).await;
    let guessed = plant_reset(setup, owner_user, epoch, "30 minutes").await;
    for _ in 0..5 {
        assert_eq!(
            complete(
                setup,
                &guessed,
                "guessing a password here",
                Some("zzzzz-99999")
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(inspect(setup, &guessed).await.0, StatusCode::NOT_FOUND);

    // Someone else's confirmed address can't be confirmed again.
    let _ = mara_id;
    let (status, _) = call(
        setup,
        &mara,
        "PUT",
        "/api/v1/account/email",
        None,
        json!({"address": "owner@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    run_jobs(&fixture).await;
    let taken = last_mail(&inbox).token();
    let (status, _) = public(
        setup,
        "POST",
        "/api/v1/auth/verify-email",
        json!({"token": taken}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

async fn browser_settings(setup: &Setup, browser: &Browser, change: Value) -> Value {
    let (status, body) = call(
        setup,
        browser,
        "PUT",
        "/api/v1/notification-settings/channels",
        None,
        change,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn notification_email_waits_for_a_confirmed_address_and_respects_the_digest() {
    let Some(fixture) = fixture("w19_notify").await else {
        return;
    };
    let setup = &fixture.setup;
    let (port, inbox) = fake_smtp().await;
    set_up_mail(setup, port).await;
    let project = create_project(setup, "EML", "Email").await;
    let general = create_channel(setup, &project, "general").await;
    let (mara_id, mara) = add_person(setup, &[&project], "mara").await;
    let (mara_user, _) = user_of(setup, mara_id).await;

    // Without a confirmed address, email can't be chosen to any effect.
    let settings = browser_settings(
        setup,
        &mara,
        json!({"group": "mentions", "channel": "email", "enabled": true}),
    )
    .await;
    assert_eq!(settings["emailAvailable"], false);
    assert_eq!(settings["mailConfigured"], true);
    let mention = |key: &'static str| {
        json!({
            "version": 1,
            "nodes": [
                {"type": "text", "text": format!("Look at this {key} ")},
                {"type": "mention", "actorId": mara_id}
            ]
        })
    };
    say(setup, &setup.browser, &general, "mention-1", mention("one")).await;
    drain(setup).await;
    let queued = |kind: &'static str| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM background_jobs WHERE kind = $1 AND state = 'queued'",
        )
        .bind(kind)
        .fetch_one(&setup.pool)
        .await
        .expect("queued")
    };
    assert_eq!(queued(notification_email::SEND_KIND).await, 0);

    // Confirmed: each mention is emailed, with a link back to the app.
    sqlx::query(
        "INSERT INTO user_email_addresses (user_id, address, verified_at) VALUES ($1, 'mara@example.com', now())",
    )
    .bind(mara_user)
    .execute(&setup.pool)
    .await
    .expect("confirmed address");
    say(setup, &setup.browser, &general, "mention-2", mention("two")).await;
    drain(setup).await;
    assert_eq!(queued(notification_email::SEND_KIND).await, 1);
    run_jobs(&fixture).await;
    let email = last_mail(&inbox);
    assert_eq!(email.to, "mara@example.com");
    assert!(
        email.body().contains("Look at this two"),
        "{}",
        email.body()
    );
    assert!(email.body().contains("http"), "{}", email.body());

    // A mention read in the app before its email goes sends nothing.
    let before = mail_count(&inbox);
    say(
        setup,
        &setup.browser,
        &general,
        "mention-3",
        mention("three"),
    )
    .await;
    drain(setup).await;
    sqlx::query("UPDATE notifications SET read_at = now() WHERE recipient_actor_id = $1")
        .bind(mara_id)
        .execute(&setup.pool)
        .await
        .expect("read");
    run_jobs(&fixture).await;
    assert_eq!(mail_count(&inbox), before);

    // With a daily digest, nothing goes one at a time; the digest collects it.
    let (status, body) = call(
        setup,
        &mara,
        "PUT",
        "/api/v1/settings/values/notifications.digest",
        None,
        json!({"scope": "user", "organizationId": setup.organization_id, "value": "daily"}),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    say(
        setup,
        &setup.browser,
        &general,
        "mention-4",
        mention("four"),
    )
    .await;
    drain(setup).await;
    assert_eq!(queued(notification_email::SEND_KIND).await, 0);
    let before = mail_count(&inbox);
    jobs::enqueue(
        &setup.pool,
        jobs::NewJob::new(notification_email::DIGEST_KIND).unique("w19-digest"),
    )
    .await
    .expect("queue digest");
    run_jobs(&fixture).await;
    assert_eq!(mail_count(&inbox), before + 1);
    let digest = last_mail(&inbox);
    assert!(
        digest.subject().starts_with("Daily digest"),
        "{}",
        digest.subject()
    );
    assert!(
        digest.body().contains("Look at this four"),
        "{}",
        digest.body()
    );
    assert!(
        !digest.body().contains("Look at this three"),
        "read ones stay out"
    );
    // And not again until tomorrow.
    jobs::enqueue(
        &setup.pool,
        jobs::NewJob::new(notification_email::DIGEST_KIND).unique("w19-digest-again"),
    )
    .await
    .expect("queue digest");
    run_jobs(&fixture).await;
    assert_eq!(mail_count(&inbox), before + 1);
}
