//! Who may see the server and point it at other machines. A workspace's
//! owner on a shared server isn't the person who runs it: they mustn't read
//! the server's checks, backups and job queue, or have it send their mail to
//! a machine on its private network. Only the server's operators (whoever set
//! it up from its terminal, and anyone it names) can, and private addresses
//! need the operator's switch.

use std::sync::Arc;

use agent_workspace_server::{AppState, router};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, send},
    mcp_admin::create_project,
    message_edits::add_person,
    w9_capacity_and_push::{call, fixture},
};

async fn get(app: &axum::Router, browser: &Browser, uri: &str) -> (StatusCode, Value) {
    send(app, browser.request("GET", uri, None, Value::Null)).await
}

#[tokio::test]
async fn the_server_page_is_for_the_servers_operators() {
    let Some(fixture) = fixture("server_operators").await else {
        return;
    };
    let setup = &fixture.setup;
    let server = |organization: Uuid| format!("/api/v1/organizations/{organization}/server");

    // The owner set this server up from its terminal.
    let (status, body) = get(&setup.app, &setup.browser, &server(setup.organization_id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, session) = get(&setup.app, &setup.browser, "/api/v1/auth/session").await;
    assert_eq!(session["user"]["capabilities"]["serverStatus"], true);

    // Mara owns a workspace of her own on the same server.
    let shared = create_project(setup, "OPS", "Shared").await;
    let (mara, mara_browser) = add_person(setup, &[&shared], "mara").await;
    let own = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Mara', $2)")
        .bind(own)
        .bind(format!("mara-{}", own.simple()))
        .execute(&setup.pool)
        .await
        .expect("Mara's workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(own)
    .bind(mara)
    .execute(&setup.pool)
    .await
    .expect("Mara owns it");
    let (status, body) = get(&setup.app, &mara_browser, &server(own)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!body.to_string().contains("backup"), "{body}");
    let (_, session) = get(&setup.app, &mara_browser, "/api/v1/auth/session").await;
    assert_eq!(session["user"]["capabilities"]["serverStatus"], false);
    // Running the server doesn't open other people's workspaces: to the
    // operator, Mara's isn't there at all.
    let (status, _) = get(&setup.app, &setup.browser, &server(own)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Named as an operator, she sees it for the workspace she owns.
    let mut config = (*fixture.state.config).clone();
    config.operator_logins = vec!["mara".to_owned()];
    let app = router(AppState {
        config: Arc::new(config),
        ..fixture.state.clone()
    });
    let (status, body) = get(&app, &mara_browser, &server(own)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, session) = get(&app, &mara_browser, "/api/v1/auth/session").await;
    assert_eq!(session["user"]["capabilities"]["serverStatus"], true);
}

#[tokio::test]
async fn mail_goes_to_public_mail_servers_unless_the_operator_allows_private_ones() {
    let Some(fixture) = fixture("server_mail_addresses").await else {
        return;
    };
    let setup = &fixture.setup;
    let mail = format!("/api/v1/organizations/{}/mail", setup.organization_id);
    let save = |smtp: Value| {
        json!({
            "provider": "smtp",
            "smtp": smtp,
            "fromAddress": "workspace@example.com",
            "fromName": null,
        })
    };
    for smtp in [
        json!({"host": "127.0.0.1", "port": 2525, "security": "none", "username": null}),
        json!({"host": "10.0.0.25", "port": 587, "security": "starttls", "username": null}),
        json!({"host": "mail.internal", "port": 465, "security": "tls", "username": null}),
        json!({"host": "smtp.example.com", "port": 6379, "security": "starttls", "username": null}),
    ] {
        let (status, body) = call(
            setup,
            &setup.browser,
            "PUT",
            &mail,
            None,
            save(smtp.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{smtp}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("public address")),
            "{body}"
        );
    }
    let (status, body) = call(
        setup,
        &setup.browser,
        "PUT",
        &mail,
        None,
        save(json!({"host": "smtp.example.com", "port": 587, "security": "starttls", "username": "me"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // With the operator's switch, a relay on this machine is fine.
    let mut config = (*fixture.state.config).clone();
    config.allow_private_addresses = true;
    let app = router(AppState {
        config: Arc::new(config),
        ..fixture.state.clone()
    });
    let (status, body) = send(
        &app,
        setup.browser.request(
            "PUT",
            &mail,
            None,
            save(json!({"host": "127.0.0.1", "port": 2525, "security": "none", "username": null})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
