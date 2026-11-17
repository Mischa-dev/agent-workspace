//! K1.23: secrets are made, rotated and revoked through operations, and
//! exchange at the token endpoint for an hour-long token that stops the
//! moment its secret does.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::support::{
    app::{initialize, send},
    world::{Workspace, WorldBuilder},
};

async fn exchange(app: &axum::Router, form: &[(&str, &str)]) -> (StatusCode, Value) {
    let body = serde_urlencoded::to_string(form).expect("form");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .expect("token request"),
        )
        .await
        .expect("token response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn token_for(app: &axum::Router, secret: &str) -> Option<String> {
    let (status, body) = exchange(
        app,
        &[
            ("grant_type", "client_credentials"),
            ("client_secret", secret),
        ],
    )
    .await;
    (status == StatusCode::OK).then(|| body["access_token"].as_str().expect("token").to_owned())
}

async fn op(world: &Workspace, person: &str, name: &str, key: &str, input: Value) -> Value {
    let browser = if person == "owner" {
        &world.owner.browser
    } else {
        &world.people[person].browser
    };
    let route = format!("/api/v2/w/{}/ops/{name}", world.organization);
    let (status, body) = send(
        &world.app,
        browser.request("POST", &route, Some(key), input),
    )
    .await;
    json!({ "status": status.as_u16(), "body": body })
}

fn made(answer: &Value) -> (Uuid, String) {
    assert_eq!(answer["body"]["outcome"], "ok", "{answer}");
    let value = &answer["body"]["value"];
    (
        value["credential"]["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
        value["secret"].as_str().expect("secret").to_owned(),
    )
}

#[tokio::test]
async fn secrets_exchange_rotate_and_revoke() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("k123_credentials")
        .await
    else {
        return;
    };
    let pool = world.pool();

    // A secret of her own exchanges for an hour-long token that works.
    let (first, secret) = made(
        &op(
            &world,
            "lena",
            "credential_create",
            "lena-1",
            json!({"name": "laptop"}),
        )
        .await,
    );
    assert!(secret.starts_with("aw_sk_"), "{secret}");
    let token = token_for(&world.app, &secret).await.expect("exchanged");
    assert!(token.starts_with("aw_at_"), "{token}");
    assert!(
        initialize(&world.app, &token).await.is_some(),
        "the token works"
    );
    // A retry answers the same secret without its value.
    let again = op(
        &world,
        "lena",
        "credential_create",
        "lena-1",
        json!({"name": "laptop"}),
    )
    .await;
    assert_eq!(
        again["body"]["value"]["credential"]["id"],
        first.to_string(),
        "{again}"
    );
    assert!(again["body"]["value"]["secret"].is_null(), "{again}");
    // A short-lived token never exchanges for another.
    let (status, _) = exchange(
        &world.app,
        &[
            ("grant_type", "client_credentials"),
            ("client_secret", &token),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Nor does a federated assertion nobody trusts.
    let (status, body) = exchange(
        &world.app,
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:jwt"),
            ("subject_token", "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0."),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // A space secret reaches only its own workspace.
    let (status, body) = exchange(
        &world.app,
        &[
            ("grant_type", "client_credentials"),
            ("client_secret", &secret),
            ("workspace", &Uuid::now_v7().to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Revoking the secret stops the token at once.
    let revoked = op(
        &world,
        "lena",
        "credential_revoke",
        "lena-2",
        json!({"credential": first}),
    )
    .await;
    assert_eq!(revoked["body"]["outcome"], "ok", "{revoked}");
    assert!(
        initialize(&world.app, &token).await.is_none(),
        "a revoked secret's token still works"
    );
    assert!(token_for(&world.app, &secret).await.is_none());

    // Rotating with no grace stops the old value at once.
    let (second, old) = made(
        &op(
            &world,
            "lena",
            "credential_create",
            "lena-3",
            json!({"name": "ci"}),
        )
        .await,
    );
    let rotated = op(
        &world,
        "lena",
        "credential_rotate",
        "lena-4",
        json!({"credential": second, "graceMinutes": 0}),
    )
    .await;
    let (_, new) = made(&rotated);
    assert!(
        rotated["body"]["value"]["oldSecretValidUntil"].is_null(),
        "{rotated}"
    );
    assert!(token_for(&world.app, &old).await.is_none());
    assert!(token_for(&world.app, &new).await.is_some());

    // With a grace period the old value works until it ends, and not after.
    let (third, old) = made(
        &op(
            &world,
            "lena",
            "credential_create",
            "lena-5",
            json!({"name": "build"}),
        )
        .await,
    );
    let rotated = op(
        &world,
        "lena",
        "credential_rotate",
        "lena-6",
        json!({"credential": third, "graceMinutes": 30}),
    )
    .await;
    let (_, new) = made(&rotated);
    assert!(
        rotated["body"]["value"]["oldSecretValidUntil"].is_string(),
        "{rotated}"
    );
    let old_token = token_for(&world.app, &old)
        .await
        .expect("the old value still works");
    assert!(token_for(&world.app, &new).await.is_some());
    sqlx::query(
        "UPDATE account_credentials SET created_at = now() - interval '1 hour', \
         expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(third)
    .execute(pool)
    .await
    .expect("the grace ends");
    assert!(token_for(&world.app, &old).await.is_none());
    assert!(
        initialize(&world.app, &old_token).await.is_none(),
        "a lapsed secret's token works"
    );

    // A member can't see or stop an agent's secrets; the owner, who may
    // manage every agent here, can.
    let rook = world.agents["rook"].actor;
    let listed = op(
        &world,
        "lena",
        "credential_list",
        "lena-7",
        json!({"account": rook}),
    )
    .await;
    assert_ne!(listed["body"]["outcome"], "ok", "{listed}");
    let listed = op(
        &world,
        "owner",
        "credential_list",
        "owner-1",
        json!({"account": rook}),
    )
    .await;
    assert_eq!(listed["body"]["outcome"], "ok", "{listed}");
    let rooks = listed["body"]["value"]["credentials"]
        .as_array()
        .expect("list");
    assert!(!rooks.is_empty(), "{listed}");
    let rooks_secret = rooks[0]["id"].as_str().expect("id").to_owned();
    let stopped = op(
        &world,
        "lena",
        "credential_revoke",
        "lena-8",
        json!({"credential": rooks_secret}),
    )
    .await;
    assert_ne!(stopped["body"]["outcome"], "ok", "{stopped}");
    let lenas = op(&world, "lena", "credential_list", "lena-9", json!({})).await;
    assert!(
        lenas["body"]["value"]["credentials"]
            .as_array()
            .expect("list")
            .iter()
            .all(|credential| credential["id"] != rooks_secret.as_str()),
        "{lenas}"
    );
    // An agent makes a secret of its own, holding what its connection does.
    let rook = &world.agents["rook"];
    let own = super::support::app::tool(
        &world.app,
        &rook.token,
        &rook.session,
        "credential_create",
        json!({"name": "rook box", "clientRequestId": "rook-1"}),
    )
    .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    let rook_secret = own["value"]["secret"].as_str().expect("secret").to_owned();
    assert!(token_for(&world.app, &rook_secret).await.is_some());
    world.finish().await;
}
