use std::{collections::BTreeMap, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    outbox,
    rate_limit::RateLimiters,
    realtime::{RealtimeHub, outbox_worker_id, run_outbox_publisher},
    router,
};
use futures_util::StreamExt;
use http::{
    HeaderValue, StatusCode,
    header::{COOKIE, ORIGIN},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{oneshot, watch},
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message, client::IntoClientRequest},
};
use uuid::Uuid;

const ALLOWED_ORIGIN: &str = "http://127.0.0.1:3100";

type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[tokio::test]
async fn two_actors_converge_privately_across_http_outbox_realtime_and_revocation() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m2_actor_realtime_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M2 Actor/realtime schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    let fixture = insert_fixture(&pool).await;

    let temporary_root = tempfile::tempdir().expect("temporary web root");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("development config");
    let realtime = RealtimeHub::new(64);
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
    let (server_shutdown_sender, server_shutdown_receiver) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, application)
            .with_graceful_shutdown(async {
                let _ = server_shutdown_receiver.await;
            })
            .await
            .expect("serve loopback application");
    });
    let (outbox_shutdown_sender, outbox_shutdown_receiver) = watch::channel(false);
    let outbox_publisher = tokio::spawn(run_outbox_publisher(
        pool.clone(),
        realtime,
        outbox_worker_id(),
        outbox_shutdown_receiver,
    ));

    let base_url = format!("http://{address}");
    let websocket_url = format!("ws://{address}/api/v1/realtime");
    let client = reqwest::Client::new();
    let mut actor_a_socket = connect(&websocket_url, &fixture.actor_a).await;
    let mut actor_b_socket = connect(&websocket_url, &fixture.actor_b).await;

    let actor_a_seen = mark_seen(
        &client,
        &base_url,
        &fixture.actor_a,
        fixture.shared_conversation_id,
        2,
    )
    .await;
    assert_eq!(actor_a_seen["highestSeenSequence"], 2);
    let actor_a_event = receive_event(&mut actor_a_socket).await;
    assert_read_state_event(
        &actor_a_event,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        2,
    );
    assert_no_delivery(&mut actor_b_socket, "Actor A's private read cursor").await;

    let (actor_b_seen_first, actor_b_seen_second) = tokio::join!(
        mark_seen(
            &client,
            &base_url,
            &fixture.actor_b,
            fixture.shared_conversation_id,
            1,
        ),
        mark_seen(
            &client,
            &base_url,
            &fixture.actor_b,
            fixture.shared_conversation_id,
            1,
        )
    );
    assert_eq!(actor_b_seen_first["highestSeenSequence"], 1);
    assert_eq!(actor_b_seen_second["highestSeenSequence"], 1);
    let actor_b_event = receive_event(&mut actor_b_socket).await;
    assert_read_state_event(
        &actor_b_event,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_b.actor_id,
        1,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM outbox_events
            WHERE event_type = 'conversation.read_state.updated'
              AND resource_id = $1
              AND payload->>'highestSeenSequence' = '1'
            "#,
        )
        .bind(fixture.actor_b.actor_id)
        .fetch_one(&pool)
        .await
        .expect("count Actor B equal-first-write outbox events"),
        1,
        "concurrent equal first writes must commit one read-state change"
    );
    assert_no_delivery(&mut actor_a_socket, "Actor B's private read cursor").await;

    let actor_a_timeline = timeline(
        &client,
        &base_url,
        &fixture.actor_a,
        fixture.shared_conversation_id,
    )
    .await;
    let actor_b_timeline = timeline(
        &client,
        &base_url,
        &fixture.actor_b,
        fixture.shared_conversation_id,
    )
    .await;
    assert_eq!(actor_a_timeline["readState"]["highestSeenSequence"], 2);
    assert_eq!(actor_b_timeline["readState"]["highestSeenSequence"], 1);
    assert_eq!(actor_a_timeline["highestSequence"], 2);
    assert_eq!(actor_b_timeline["highestSequence"], 2);
    let actor_a_sidebar = sidebar(
        &client,
        &base_url,
        &fixture.actor_a,
        fixture.shared_project_id,
        fixture.shared_parent_channel_id,
    )
    .await;
    let actor_b_sidebar = sidebar(
        &client,
        &base_url,
        &fixture.actor_b,
        fixture.shared_project_id,
        fixture.shared_parent_channel_id,
    )
    .await;
    let actor_a_thread = sidebar_thread(&actor_a_sidebar, fixture.shared_conversation_id);
    let actor_b_thread = sidebar_thread(&actor_b_sidebar, fixture.shared_conversation_id);
    assert_eq!(actor_a_thread["highestSequence"], 2);
    assert_eq!(actor_a_thread["highestSeenSequence"], 2);
    assert_eq!(actor_a_thread["attention"]["kind"], "settled");
    assert_eq!(actor_b_thread["highestSequence"], 2);
    assert_eq!(actor_b_thread["highestSeenSequence"], 1);
    assert_eq!(actor_b_thread["attention"]["kind"], "unread");

    let (_, foreign_event_id) = append_message_with_outbox(
        &pool,
        fixture.foreign_project_id,
        fixture.foreign_conversation_id,
        fixture.actor_a.actor_id,
        "foreign Project update",
    )
    .await;
    let foreign_event = receive_event(&mut actor_a_socket).await;
    assert_eq!(foreign_event["event_id"], foreign_event_id.to_string());
    assert_eq!(
        foreign_event["project_id"],
        fixture.foreign_project_id.to_string()
    );
    assert_no_delivery(&mut actor_b_socket, "a cross-Project durable event").await;

    actor_a_socket
        .close(None)
        .await
        .expect("disconnect Actor A before the durable event");
    let (missed_message_id, missed_event_id) = append_message_with_outbox(
        &pool,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        "durable update while Actor A is disconnected",
    )
    .await;
    let actor_b_missed_event = receive_event(&mut actor_b_socket).await;
    assert_eq!(
        actor_b_missed_event["event_id"],
        missed_event_id.to_string()
    );
    wait_until_published(&pool, missed_event_id).await;

    let mut reconnected_actor_a = connect(&websocket_url, &fixture.actor_a).await;
    assert_no_delivery(
        &mut reconnected_actor_a,
        "an already-published event after reconnect",
    )
    .await;
    let converged_timeline = timeline(
        &client,
        &base_url,
        &fixture.actor_a,
        fixture.shared_conversation_id,
    )
    .await;
    assert_eq!(converged_timeline["highestSequence"], 3);
    assert_eq!(converged_timeline["readState"]["highestSeenSequence"], 2);
    assert!(
        converged_timeline["entries"]
            .as_array()
            .expect("timeline entries")
            .iter()
            .any(|entry| entry["message"]["id"] == missed_message_id.to_string()),
        "HTTP refetch must converge on the durable Message missed over realtime"
    );
    let converged_sidebar = sidebar(
        &client,
        &base_url,
        &fixture.actor_a,
        fixture.shared_project_id,
        fixture.shared_parent_channel_id,
    )
    .await;
    let converged_thread = sidebar_thread(&converged_sidebar, fixture.shared_conversation_id);
    assert_eq!(converged_thread["highestSequence"], 3);
    assert_eq!(converged_thread["highestSeenSequence"], 2);
    assert_eq!(converged_thread["attention"]["kind"], "unread");

    let (actor_a_high, actor_a_stale) = tokio::join!(
        mark_seen(
            &client,
            &base_url,
            &fixture.actor_a,
            fixture.shared_conversation_id,
            3,
        ),
        mark_seen(
            &client,
            &base_url,
            &fixture.actor_a,
            fixture.shared_conversation_id,
            2,
        )
    );
    assert_eq!(actor_a_high["highestSeenSequence"], 3);
    assert!(
        actor_a_stale["highestSeenSequence"] == 2 || actor_a_stale["highestSeenSequence"] == 3,
        "the stale concurrent response may linearize before or after the advancing write"
    );
    let actor_a_high_event = receive_event(&mut reconnected_actor_a).await;
    assert_read_state_event(
        &actor_a_high_event,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        3,
    );
    assert_no_delivery(
        &mut actor_b_socket,
        "Actor A's concurrent high-water read cursor",
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM outbox_events
            WHERE event_type = 'conversation.read_state.updated'
              AND resource_id = $1
              AND payload->>'highestSeenSequence' = '3'
            "#,
        )
        .bind(fixture.actor_a.actor_id)
        .fetch_one(&pool)
        .await
        .expect("count Actor A high-water outbox events"),
        1,
        "concurrent high-vs-stale writes must emit only the advancing change"
    );

    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.shared_project_id)
    .bind(fixture.actor_b.actor_id)
    .execute(&pool)
    .await
    .expect("disable Actor B membership");
    let (_, post_revocation_event_id) = append_message_with_outbox(
        &pool,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        "membership revocation must suppress delivery",
    )
    .await;
    let actor_a_after_revocation = receive_event(&mut reconnected_actor_a).await;
    assert_eq!(
        actor_a_after_revocation["event_id"],
        post_revocation_event_id.to_string()
    );
    assert_closed_or_suppressed(&mut actor_b_socket, "disabled membership").await;
    let revoked_timeline = client
        .get(format!(
            "{base_url}/api/v1/conversations/{}/timeline",
            fixture.shared_conversation_id
        ))
        .header(COOKIE, fixture.actor_b.cookie_header())
        .send()
        .await
        .expect("request timeline after membership revocation");
    assert_eq!(revoked_timeline.status(), StatusCode::NOT_FOUND);

    let _ = actor_b_socket.close(None).await;
    sqlx::query(
        "UPDATE project_memberships SET enabled = true WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.shared_project_id)
    .bind(fixture.actor_b.actor_id)
    .execute(&pool)
    .await
    .expect("re-enable Actor B membership");
    let mut resumed_actor_b = connect(&websocket_url, &fixture.actor_b).await;
    let resumed_timeline = timeline(
        &client,
        &base_url,
        &fixture.actor_b,
        fixture.shared_conversation_id,
    )
    .await;
    assert_eq!(resumed_timeline["highestSequence"], 4);
    assert_eq!(
        resumed_timeline["readState"]["highestSeenSequence"], 1,
        "re-enable resumes the Actor's durable prior cursor"
    );
    let (_, resumed_event_id) = append_message_with_outbox(
        &pool,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        "re-enabled member resumes authorized invalidations",
    )
    .await;
    let actor_a_resumed_event = receive_event(&mut reconnected_actor_a).await;
    let actor_b_resumed_event = receive_event(&mut resumed_actor_b).await;
    assert_eq!(
        actor_a_resumed_event["event_id"],
        resumed_event_id.to_string()
    );
    assert_eq!(
        actor_b_resumed_event["event_id"],
        resumed_event_id.to_string()
    );
    let resumed_sidebar = sidebar(
        &client,
        &base_url,
        &fixture.actor_b,
        fixture.shared_project_id,
        fixture.shared_parent_channel_id,
    )
    .await;
    let resumed_thread = sidebar_thread(&resumed_sidebar, fixture.shared_conversation_id);
    assert_eq!(resumed_thread["highestSequence"], 5);
    assert_eq!(resumed_thread["highestSeenSequence"], 1);
    assert_eq!(resumed_thread["attention"]["kind"], "unread");

    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(fixture.organization_id)
    .bind(fixture.actor_b.actor_id)
    .execute(&pool)
    .await
    .expect("disable Actor B Organization membership");
    let (_, organization_revoked_event_id) = append_message_with_outbox(
        &pool,
        fixture.shared_project_id,
        fixture.shared_conversation_id,
        fixture.actor_a.actor_id,
        "organization membership revocation fails closed",
    )
    .await;
    let actor_a_organization_event = receive_event(&mut reconnected_actor_a).await;
    assert_eq!(
        actor_a_organization_event["event_id"],
        organization_revoked_event_id.to_string()
    );
    assert_no_delivery(
        &mut resumed_actor_b,
        "disabled Organization member Project invalidation",
    )
    .await;
    let organization_revoked_timeline = client
        .get(format!(
            "{base_url}/api/v1/conversations/{}/timeline",
            fixture.shared_conversation_id
        ))
        .header(COOKIE, fixture.actor_b.cookie_header())
        .send()
        .await
        .expect("request timeline after Organization membership revocation");
    assert_eq!(
        organization_revoked_timeline.status(),
        StatusCode::NOT_FOUND
    );

    let logout = client
        .post(format!("{base_url}/api/v1/auth/logout"))
        .header(ORIGIN, ALLOWED_ORIGIN)
        .header(COOKIE, fixture.actor_a.cookie_header())
        .header("x-csrf-token", &fixture.actor_a.csrf_token)
        .send()
        .await
        .expect("logout Actor A");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_socket_closed(&mut reconnected_actor_a, "revoked browser session").await;

    let _ = resumed_actor_b.close(None).await;
    let _ = outbox_shutdown_sender.send(true);
    timeout(Duration::from_secs(3), outbox_publisher)
        .await
        .expect("outbox publisher shutdown deadline")
        .expect("outbox publisher task")
        .expect("outbox publisher result");
    let _ = server_shutdown_sender.send(());
    timeout(Duration::from_secs(3), server)
        .await
        .expect("server shutdown deadline")
        .expect("server task");

    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M2 Actor/realtime schema");
}

fn assert_read_state_event(
    event: &Value,
    project_id: Uuid,
    conversation_id: Uuid,
    actor_id: Uuid,
    highest_seen_sequence: i64,
) {
    assert_eq!(event["type"], "conversation.read_state.updated");
    assert_eq!(event["project_id"], project_id.to_string());
    assert_eq!(event["conversationId"], conversation_id.to_string());
    assert_eq!(
        event["payload"]["conversationId"],
        conversation_id.to_string()
    );
    assert_eq!(event["payload"]["actorId"], actor_id.to_string());
    assert_eq!(
        event["payload"]["highestSeenSequence"],
        highest_seen_sequence
    );
}

async fn mark_seen(
    client: &reqwest::Client,
    base_url: &str,
    actor: &ActorSession,
    conversation_id: Uuid,
    highest_seen_sequence: i64,
) -> Value {
    let response = client
        .post(format!(
            "{base_url}/api/v1/conversations/{conversation_id}/mark-seen"
        ))
        .header(ORIGIN, ALLOWED_ORIGIN)
        .header(COOKIE, actor.cookie_header())
        .header("x-csrf-token", &actor.csrf_token)
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&json!({"highestSeenSequence": highest_seen_sequence}))
                .expect("serialize mark-seen request"),
        )
        .send()
        .await
        .expect("mark Conversation seen");
    let status = response.status();
    let body: Value = serde_json::from_slice(
        &response
            .bytes()
            .await
            .expect("read mark-seen response body"),
    )
    .expect("mark-seen JSON response");
    assert_eq!(status, StatusCode::OK, "mark-seen response: {body}");
    body
}

async fn timeline(
    client: &reqwest::Client,
    base_url: &str,
    actor: &ActorSession,
    conversation_id: Uuid,
) -> Value {
    let response = client
        .get(format!(
            "{base_url}/api/v1/conversations/{conversation_id}/timeline"
        ))
        .header(COOKIE, actor.cookie_header())
        .send()
        .await
        .expect("read Conversation timeline");
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&response.bytes().await.expect("read timeline response body"))
            .expect("timeline JSON response");
    assert_eq!(status, StatusCode::OK, "timeline response: {body}");
    body
}

async fn sidebar(
    client: &reqwest::Client,
    base_url: &str,
    actor: &ActorSession,
    project_id: Uuid,
    selected_channel_id: Uuid,
) -> Value {
    let response = client
        .get(format!(
            "{base_url}/api/v1/projects/{project_id}/conversation-sidebar?selectedChannelId={selected_channel_id}"
        ))
        .header(COOKIE, actor.cookie_header())
        .send()
        .await
        .expect("read Conversation sidebar");
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&response.bytes().await.expect("read sidebar response body"))
            .expect("sidebar JSON response");
    assert_eq!(status, StatusCode::OK, "sidebar response: {body}");
    body
}

fn sidebar_thread(sidebar: &Value, conversation_id: Uuid) -> &Value {
    sidebar["channels"]
        .as_array()
        .expect("sidebar Channels")
        .iter()
        .flat_map(|channel| {
            channel["threads"]
                .as_array()
                .expect("sidebar Threads")
                .iter()
        })
        .find(|thread| thread["id"] == conversation_id.to_string())
        .expect("Thread in selected Channel sidebar")
}

async fn connect(websocket_url: &str, actor: &ActorSession) -> TestSocket {
    let mut request = websocket_url
        .into_client_request()
        .expect("construct WebSocket request");
    request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static(ALLOWED_ORIGIN));
    request.headers_mut().insert(
        COOKIE,
        HeaderValue::from_str(&actor.cookie_header()).expect("valid synthetic Cookie"),
    );
    let (socket, response) = connect_async(request)
        .await
        .expect("authenticated WebSocket upgrade");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket
}

async fn receive_event(socket: &mut TestSocket) -> Value {
    let delivered = timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("realtime event deadline")
        .expect("open WebSocket stream")
        .expect("valid WebSocket message");
    let Message::Text(delivered) = delivered else {
        panic!("expected a realtime text event, got {delivered:?}");
    };
    serde_json::from_str(delivered.as_str()).expect("decode realtime event")
}

async fn assert_no_delivery(socket: &mut TestSocket, description: &str) {
    match timeout(Duration::from_millis(400), socket.next()).await {
        Err(_) => {}
        Ok(other) => panic!("unexpected delivery for {description}: {other:?}"),
    }
}

async fn assert_closed_or_suppressed(socket: &mut TestSocket, description: &str) {
    match timeout(Duration::from_millis(500), socket.next()).await {
        Err(_) | Ok(None) | Ok(Some(Ok(Message::Close(_)))) => {}
        Ok(Some(Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed))) => {}
        Ok(other) => panic!("revoked {description} still received realtime data: {other:?}"),
    }
}

async fn assert_socket_closed(socket: &mut TestSocket, description: &str) {
    let closed = timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("session revocation close deadline");
    match closed {
        None | Some(Ok(Message::Close(_))) => {}
        Some(Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed)) => {}
        other => panic!("{description} did not close the WebSocket: {other:?}"),
    }
}

async fn wait_until_published(pool: &PgPool, event_id: Uuid) {
    timeout(Duration::from_secs(3), async {
        loop {
            let published = sqlx::query_scalar::<_, bool>(
                "SELECT published_at IS NOT NULL FROM outbox_events WHERE event_id = $1",
            )
            .bind(event_id)
            .fetch_one(pool)
            .await
            .expect("read outbox publication state");
            if published {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("outbox publication deadline");
}

async fn append_message_with_outbox(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
    author_actor_id: Uuid,
    text: &str,
) -> (Uuid, Uuid) {
    let mut transaction = pool.begin().await.expect("append transaction");
    let sequence = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE conversations
        SET next_entry_sequence = next_entry_sequence + 1
        WHERE id = $1 AND project_id = $2
        RETURNING next_entry_sequence - 1
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("allocate Conversation sequence");
    let message_id = Uuid::now_v7();
    insert_message_entry(
        &mut transaction,
        project_id,
        conversation_id,
        author_actor_id,
        message_id,
        sequence,
        text,
    )
    .await;
    let event_id = outbox::enqueue_message_created(
        &mut transaction,
        project_id,
        conversation_id,
        message_id,
        sequence,
        author_actor_id,
    )
    .await
    .expect("enqueue durable Message event");
    transaction.commit().await.expect("commit appended Message");
    (message_id, event_id)
}

async fn insert_message_entry(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    author_actor_id: Uuid,
    message_id: Uuid,
    sequence: i64,
    text: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text
        )
        VALUES ($1, $2, $3, $4, 1, $5, $6)
        "#,
    )
    .bind(message_id)
    .bind(project_id)
    .bind(conversation_id)
    .bind(author_actor_id)
    .bind(json!({"version": 1, "nodes": [{"type": "text", "text": text}]}))
    .bind(text)
    .execute(&mut **transaction)
    .await
    .expect("insert Message");
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, message_id
        )
        VALUES ($1, $2, $3, 'message', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(sequence)
    .bind(message_id)
    .execute(&mut **transaction)
    .await
    .expect("insert Conversation entry");
}

#[derive(Clone)]
struct ActorSession {
    actor_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    session_token: String,
    csrf_token: String,
}

impl ActorSession {
    fn cookie_header(&self) -> String {
        format!(
            "agent_workspace_session={}; agent_workspace_csrf={}",
            self.session_token, self.csrf_token
        )
    }
}

struct Fixture {
    actor_a: ActorSession,
    actor_b: ActorSession,
    organization_id: Uuid,
    shared_project_id: Uuid,
    shared_parent_channel_id: Uuid,
    shared_conversation_id: Uuid,
    foreign_project_id: Uuid,
    foreign_conversation_id: Uuid,
}

async fn insert_fixture(pool: &PgPool) -> Fixture {
    let actor_a = actor_session("actor-a");
    let actor_b = actor_session("actor-b");
    let organization_id = Uuid::now_v7();
    let shared_project_id = Uuid::now_v7();
    let foreign_project_id = Uuid::now_v7();
    let shared_parent_channel_id = Uuid::now_v7();
    let shared_conversation_id = Uuid::now_v7();
    let foreign_conversation_id = Uuid::now_v7();
    let suffix = organization_id.simple().to_string();
    let initial_message_one = Uuid::now_v7();
    let initial_message_two = Uuid::now_v7();

    let mut transaction = pool.begin().await.expect("fixture transaction");
    for actor in [&actor_a, &actor_b] {
        let actor_label = if actor.actor_id == actor_a.actor_id {
            "a"
        } else {
            "b"
        };
        let actor_suffix = actor.actor_id.simple().to_string();
        sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
            .bind(actor.actor_id)
            .execute(&mut *transaction)
            .await
            .expect("insert human Actor");
        sqlx::query(
            r#"
            INSERT INTO users (id, actor_id, login_name, display_name)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(actor.user_id)
        .bind(actor.actor_id)
        .bind(format!("m2{actor_label}{}", &actor_suffix[12..]))
        .bind(if actor.actor_id == actor_a.actor_id {
            "Actor A"
        } else {
            "Actor B"
        })
        .execute(&mut *transaction)
        .await
        .expect("insert User");
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
        .bind(actor.session_id)
        .bind(actor.user_id)
        .bind(Sha256::digest(actor.session_token.as_bytes()).to_vec())
        .bind(Sha256::digest(actor.csrf_token.as_bytes()).to_vec())
        .execute(&mut *transaction)
        .await
        .expect("insert browser Session");
    }
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
        .bind(organization_id)
        .bind("M2 Actor Realtime Organization")
        .bind(format!("m2-actor-realtime-{}", &suffix[..20]))
        .execute(&mut *transaction)
        .await
        .expect("insert Organization");
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, actor_id, role)
        VALUES ($1, $2, 'owner'), ($1, $3, 'member')
        "#,
    )
    .bind(organization_id)
    .bind(actor_a.actor_id)
    .bind(actor_b.actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Organization memberships");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES
            ($1, $2, 'SHR', 'Shared Project'),
            ($3, $2, 'FOR', 'Foreign Project')
        "#,
    )
    .bind(shared_project_id)
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
        VALUES
            ($1, $2, $3, 'owner', 'Actor A', 'actor-a'),
            ($4, $2, $5, 'member', 'Actor B', 'actor-b'),
            ($6, $7, $3, 'owner', 'Actor A', 'foreign-a')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(shared_project_id)
    .bind(actor_a.actor_id)
    .bind(Uuid::now_v7())
    .bind(actor_b.actor_id)
    .bind(Uuid::now_v7())
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Project memberships");
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, name, next_entry_sequence
        )
        VALUES
            ($1, $2, 'channel', NULL, 'Shared Channel', 1),
            ($3, $2, 'thread', $1, 'Shared Thread', 3),
            ($4, $5, 'channel', NULL, 'Foreign Channel', 1)
        "#,
    )
    .bind(shared_parent_channel_id)
    .bind(shared_project_id)
    .bind(shared_conversation_id)
    .bind(foreign_conversation_id)
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Conversations");
    insert_message_entry(
        &mut transaction,
        shared_project_id,
        shared_conversation_id,
        actor_a.actor_id,
        initial_message_one,
        1,
        "first durable Message",
    )
    .await;
    insert_message_entry(
        &mut transaction,
        shared_project_id,
        shared_conversation_id,
        actor_a.actor_id,
        initial_message_two,
        2,
        "second durable Message",
    )
    .await;
    transaction.commit().await.expect("commit fixture");

    Fixture {
        actor_a,
        actor_b,
        organization_id,
        shared_project_id,
        shared_parent_channel_id,
        shared_conversation_id,
        foreign_project_id,
        foreign_conversation_id,
    }
}

fn actor_session(label: &str) -> ActorSession {
    let actor_id = Uuid::now_v7();
    let suffix = actor_id.simple();
    ActorSession {
        actor_id,
        user_id: Uuid::now_v7(),
        session_id: Uuid::now_v7(),
        session_token: format!("m2-{label}-session-{suffix}"),
        csrf_token: format!("m2-{label}-csrf-{suffix}"),
    }
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(12)
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
        .expect("connect schema-scoped M2 Actor/realtime pool")
}
