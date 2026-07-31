use std::{collections::BTreeMap, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::{RealtimeEvent, RealtimeHub},
    router,
};
use futures_util::StreamExt;
use http::{
    HeaderValue, StatusCode,
    header::{COOKIE, ORIGIN},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Error as WebSocketError, Message as ClientMessage, client::IntoClientRequest,
        protocol::frame::coding::CloseCode,
    },
};
use uuid::Uuid;

const ALLOWED_ORIGIN: &str = "http://127.0.0.1:3100";
const REJECTED_ORIGIN: &str = "http://127.0.0.1:3100.attacker.example";

#[tokio::test]
async fn websocket_upgrade_delivery_isolation_logout_revocation_and_lag_resync() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m1_realtime_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated realtime schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    let fixture = insert_fixture(&pool).await;

    let temporary_root = tempfile::tempdir().expect("temporary web root");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("development config");
    let realtime = RealtimeHub::new(16);
    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: realtime.clone(),
        supervisors,
    });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback application listener");
    let address = listener.local_addr().expect("loopback listener address");
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, application)
            .with_graceful_shutdown(async {
                let _ = shutdown_receiver.await;
            })
            .await
            .expect("serve loopback application");
    });
    let websocket_url = format!("ws://{address}/api/v1/realtime");

    assert_upgrade_rejected(
        &websocket_url,
        Some(ALLOWED_ORIGIN),
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_upgrade_rejected(
        &websocket_url,
        Some(ALLOWED_ORIGIN),
        Some("agent_workspace_session=invalid-synthetic-session"),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_upgrade_rejected(
        &websocket_url,
        Some(REJECTED_ORIGIN),
        Some(&fixture.session_cookie()),
        StatusCode::FORBIDDEN,
    )
    .await;

    let request = websocket_request(
        &websocket_url,
        Some(ALLOWED_ORIGIN),
        Some(&fixture.session_cookie()),
    );
    let (mut socket, response) = connect_async(request)
        .await
        .expect("authenticated WebSocket upgrade");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    let foreign_event_id = Uuid::now_v7();
    realtime.publish(RealtimeEvent {
        event_id: foreign_event_id,
        kind: "message.delta",
        project_id: fixture.foreign_project_id,
        conversation_id: Some(fixture.foreign_conversation_id),
        payload: serde_json::json!({"scope": "foreign"}),
    });
    let own_event_id = Uuid::now_v7();
    realtime.publish(RealtimeEvent {
        event_id: own_event_id,
        kind: "message.delta",
        project_id: fixture.own_project_id,
        conversation_id: Some(fixture.own_conversation_id),
        payload: serde_json::json!({"scope": "own"}),
    });

    let delivered = timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("same-Project realtime event deadline")
        .expect("open WebSocket stream")
        .expect("valid WebSocket message");
    let ClientMessage::Text(delivered) = delivered else {
        panic!("expected a realtime text event");
    };
    let delivered: serde_json::Value =
        serde_json::from_str(delivered.as_str()).expect("decode realtime event");
    assert_eq!(
        delivered["event_id"],
        serde_json::Value::String(own_event_id.to_string())
    );
    assert_ne!(
        delivered["event_id"],
        serde_json::Value::String(foreign_event_id.to_string()),
        "the cross-Project event must be suppressed"
    );

    let logout_response = reqwest::Client::new()
        .post(format!("http://{address}/api/v1/auth/logout"))
        .header(ORIGIN, ALLOWED_ORIGIN)
        .header(COOKIE, fixture.full_cookie_header())
        .header("x-csrf-token", &fixture.csrf_token)
        .send()
        .await
        .expect("logout request");
    assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT revoked_at IS NOT NULL FROM sessions WHERE id = $1")
            .bind(fixture.session_id)
            .fetch_one(&pool)
            .await
            .expect("read revoked session")
    );

    let closed = timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("immediate logout revocation deadline");
    match closed {
        Some(Ok(ClientMessage::Close(_))) | None => {}
        Some(Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed)) => {}
        other => panic!("logout did not close the matching WebSocket: {other:?}"),
    }

    let _ = socket.close(None).await;

    let resync_session_id = Uuid::now_v7();
    let resync_session_token = format!("realtime-resync-{}", resync_session_id.simple());
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, $3, $4, 1,
            now() + interval '1 hour', now() + interval '1 day'
        )
        "#,
    )
    .bind(resync_session_id)
    .bind(fixture.user_id)
    .bind(Sha256::digest(resync_session_token.as_bytes()).to_vec())
    .bind(Sha256::digest(b"realtime-resync-csrf").to_vec())
    .execute(&pool)
    .await
    .expect("insert realtime resync Session");
    let resync_cookie = format!("agent_workspace_session={resync_session_token}");
    let resync_request =
        websocket_request(&websocket_url, Some(ALLOWED_ORIGIN), Some(&resync_cookie));
    let (mut lagged_socket, response) = connect_async(resync_request)
        .await
        .expect("resync WebSocket upgrade");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    // Publishing without yielding deterministically overruns the server's
    // bounded broadcast receiver on this current-thread Tokio test runtime.
    for sequence in 0..32 {
        realtime.publish(RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: fixture.own_project_id,
            conversation_id: Some(fixture.own_conversation_id),
            payload: serde_json::json!({"sequence": sequence}),
        });
    }
    let lagged_close = timeout(Duration::from_secs(2), lagged_socket.next())
        .await
        .expect("lagged realtime close deadline")
        .expect("lagged WebSocket stream")
        .expect("valid lagged WebSocket message");
    let ClientMessage::Close(Some(close)) = lagged_close else {
        panic!("lagged realtime client did not receive a close frame: {lagged_close:?}");
    };
    assert_eq!(close.code, CloseCode::Restart);
    assert_eq!(close.reason.as_str(), "realtime lag; reconnect and refetch");

    let _ = shutdown_sender.send(());
    timeout(Duration::from_secs(3), server)
        .await
        .expect("server shutdown deadline")
        .expect("server task");

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated realtime schema");
}

async fn assert_upgrade_rejected(
    websocket_url: &str,
    origin: Option<&str>,
    cookie: Option<&str>,
    expected_status: StatusCode,
) {
    let request = websocket_request(websocket_url, origin, cookie);
    match connect_async(request).await {
        Err(WebSocketError::Http(response)) => {
            assert_eq!(response.status(), expected_status);
        }
        Ok((mut socket, _)) => {
            let _ = socket.close(None).await;
            panic!("WebSocket upgrade unexpectedly succeeded");
        }
        Err(error) => panic!("unexpected WebSocket rejection: {error}"),
    }
}

fn websocket_request(
    websocket_url: &str,
    origin: Option<&str>,
    cookie: Option<&str>,
) -> http::Request<()> {
    let mut request = websocket_url
        .into_client_request()
        .expect("construct WebSocket request");
    if let Some(origin) = origin {
        request.headers_mut().insert(
            ORIGIN,
            HeaderValue::from_str(origin).expect("valid synthetic Origin"),
        );
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(cookie).expect("valid synthetic Cookie"),
        );
    }
    request
}

struct Fixture {
    session_id: Uuid,
    user_id: Uuid,
    own_project_id: Uuid,
    own_conversation_id: Uuid,
    foreign_project_id: Uuid,
    foreign_conversation_id: Uuid,
    session_token: String,
    csrf_token: String,
}

impl Fixture {
    fn session_cookie(&self) -> String {
        format!("agent_workspace_session={}", self.session_token)
    }

    fn full_cookie_header(&self) -> String {
        format!(
            "agent_workspace_session={}; agent_workspace_csrf={}",
            self.session_token, self.csrf_token
        )
    }
}

async fn insert_fixture(pool: &PgPool) -> Fixture {
    let actor_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let own_project_id = Uuid::now_v7();
    let foreign_project_id = Uuid::now_v7();
    let own_conversation_id = Uuid::now_v7();
    let foreign_conversation_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let suffix = actor_id.simple().to_string();
    let session_token = format!("realtime-session-{suffix}");
    let csrf_token = format!("realtime-csrf-{suffix}");

    let mut transaction = pool.begin().await.expect("fixture transaction");
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&mut *transaction)
        .await
        .expect("insert Actor");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, 'Realtime User')",
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(format!("realtime{}", &suffix[..20]))
    .execute(&mut *transaction)
    .await
    .expect("insert User");
    sqlx::query(
        "INSERT INTO organizations (id, name, slug) VALUES ($1, 'Realtime organization', $2)",
    )
    .bind(organization_id)
    .bind(format!("realtime-{}", &suffix[..20]))
    .execute(&mut *transaction)
    .await
    .expect("insert Organization");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES
            ($1, $2, 'RT', 'Realtime Project'),
            ($3, $2, 'RX', 'Foreign Realtime Project')
        "#,
    )
    .bind(own_project_id)
    .bind(organization_id)
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Projects");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'viewer', 'Realtime User', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(own_project_id)
    .bind(actor_id)
    .bind(format!("realtime-{}", &suffix[..12]))
    .execute(&mut *transaction)
    .await
    .expect("insert Project membership");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES
            ($1, $2, 'channel', 'Realtime channel'),
            ($3, $4, 'channel', 'Foreign realtime channel')
        "#,
    )
    .bind(own_conversation_id)
    .bind(own_project_id)
    .bind(foreign_conversation_id)
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Conversations");
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, $3, $4, 1,
            now() + interval '1 hour', now() + interval '1 day'
        )
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(Sha256::digest(session_token.as_bytes()).to_vec())
    .bind(Sha256::digest(csrf_token.as_bytes()).to_vec())
    .execute(&mut *transaction)
    .await
    .expect("insert session");
    transaction.commit().await.expect("commit fixture");

    Fixture {
        session_id,
        user_id,
        own_project_id,
        own_conversation_id,
        foreign_project_id,
        foreign_conversation_id,
        session_token,
        csrf_token,
    }
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
        .max_connections(4)
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
        .expect("connect to isolated realtime schema")
}
