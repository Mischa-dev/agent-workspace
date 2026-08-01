use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    domain::allocate_conversation_sequence,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash},
    message_content::{MessageContent, MessageContentError},
    outbox::{
        self, ConversationArchivedPayload, ConversationCreatedPayload, MessageCreatedPayload,
        NewOutboxEvent, OutboxConversationKind, OutboxEventPayload, ReadStateUpdatedPayload,
    },
    policy::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, Membership,
        MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
};

// A valid v1 Message can contain 20,000 four-byte Unicode scalar values plus
// bounded AST structure. Keep the transport cap above that exact domain
// maximum while remaining far below Axum's general default.
const MAX_HTTP_BODY_BYTES: usize = 131_072;
const MAX_CONVERSATION_NAME_CHARACTERS: usize = 120;
const MAX_MENTION_SEARCH_CHARACTERS: usize = 120;
const DEFAULT_TIMELINE_PAGE_SIZE: u16 = 50;
const MAX_TIMELINE_PAGE_SIZE: u16 = 100;
const DEFAULT_ACTOR_PAGE_SIZE: u16 = 20;
const MAX_ACTOR_PAGE_SIZE: u16 = 50;
const MAX_CHANNELS_PER_PROJECT: i64 = 100;
const MAX_THREADS_PER_SIDEBAR: i64 = 1_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{id}/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/v1/projects/{id}/mentionable-actors",
            get(search_mentionable_actors),
        )
        .route(
            "/api/v1/projects/{id}/conversation-sidebar",
            get(project_conversation_sidebar),
        )
        .route("/api/v1/channels/{id}/messages", post(send_channel_message))
        .route("/api/v1/threads/{id}/messages", post(send_thread_message))
        .route("/api/v1/channels/{id}/threads", post(create_thread))
        .route("/api/v1/channels/{id}/archive", post(archive_channel))
        .route("/api/v1/threads/{id}/archive", post(archive_thread))
        .route(
            "/api/v1/conversations/{id}/timeline",
            get(conversation_timeline),
        )
        .route(
            "/api/v1/conversations/{id}/mark-seen",
            post(mark_conversation_seen),
        )
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
}

#[derive(Debug, FromRow)]
struct ProjectAccessRow {
    project_id: Uuid,
    project_archived: bool,
    membership_role: String,
    membership_enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct ConversationAccessRow {
    id: Uuid,
    project_id: Uuid,
    kind: String,
    parent_channel_id: Option<Uuid>,
    origin_message_id: Option<Uuid>,
    name: String,
    version: i64,
    archived_at: Option<String>,
    project_archived: bool,
    parent_archived: bool,
    next_entry_sequence: i64,
    membership_role: String,
    membership_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateChannelRequest {
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateThreadRequest {
    name: String,
    origin_message_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendMessageRequest {
    content: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveConversationRequest {
    expected_version: i64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MarkSeenRequest {
    highest_seen_sequence: i64,
    expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TimelineQuery {
    cursor: Option<String>,
    limit: Option<u16>,
    include_archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MentionSearchQuery {
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SidebarQuery {
    selected_channel_id: Option<Uuid>,
    include_archived: Option<bool>,
    expanded_channel_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub kind: String,
    pub parent_channel_id: Option<Uuid>,
    pub origin_message_id: Option<Uuid>,
    pub name: String,
    pub archived_at: Option<String>,
    pub version: i64,
}

impl From<&ConversationAccessRow> for ConversationView {
    fn from(row: &ConversationAccessRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            kind: row.kind.clone(),
            parent_channel_id: row.parent_channel_id,
            origin_message_id: row.origin_message_id,
            name: row.name.clone(),
            archived_at: row.archived_at.clone(),
            version: row.version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorView {
    pub id: Uuid,
    pub kind: String,
    pub display_name: String,
    pub handle: String,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: Uuid,
    pub author: ActorView,
    pub content: serde_json::Value,
    pub plain_text: String,
    pub mentioned_actors: Vec<ActorView>,
    pub referenced_work_items: Vec<ConversationWorkItemReferenceView>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationWorkItemReferenceView {
    pub id: Uuid,
    pub key: String,
    pub title: String,
    pub status_category: String,
    pub archived_at: Option<String>,
    pub href: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntryView {
    pub id: Uuid,
    pub sequence: i64,
    pub kind: String,
    pub created_at: String,
    pub message: Option<MessageView>,
    pub worker_event: Option<WorkerEventView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEventView {
    pub id: Uuid,
    pub agent_run_id: Uuid,
    pub kind: String,
    pub summary: String,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadStateView {
    pub highest_seen_sequence: i64,
    pub version: i64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCapabilities {
    pub view: bool,
    pub send_message: bool,
    pub archive: bool,
    pub mark_seen: bool,
    pub worker_invoke: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAppendResponse {
    pub conversation_id: Uuid,
    pub message: MessageView,
    pub entry: TimelineEntryView,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCreateResponse {
    pub conversation: ConversationView,
    pub read_state: ReadStateView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelListResponse {
    channels: Vec<ChannelListItem>,
    capabilities: ChannelListCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelListItem {
    #[serde(flatten)]
    conversation: ConversationView,
    highest_sequence: i64,
    read_state: ReadStateView,
    unread: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelListCapabilities {
    create_channel: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineResponse {
    conversation: ConversationView,
    linked_work_items: Vec<ConversationWorkItemReferenceView>,
    entries: Vec<TimelineEntryView>,
    next_cursor: Option<String>,
    highest_sequence: i64,
    read_state: ReadStateView,
    active_run: Option<RunView>,
    latest_terminal_run: Option<RunView>,
    approvals: Vec<ApprovalView>,
    capabilities: ConversationCapabilities,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunRow {
    pub(crate) id: Uuid,
    pub(crate) state: String,
    pub(crate) repository_id: Uuid,
    pub(crate) repository_workspace_id: Option<Uuid>,
    pub(crate) provider_session_id: Option<Uuid>,
    pub(crate) provider_turn_id: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base_commit: Option<String>,
    pub(crate) published_commit: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) publish_failure_category: Option<String>,
    pub(crate) queue_position: Option<i64>,
    pub(crate) failure_reason: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunView {
    #[serde(flatten)]
    pub(crate) run: RunRow,
    pub(crate) failure_acknowledged: bool,
    pub(crate) capabilities: RunCapabilities,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunCapabilities {
    pub(crate) interrupt: bool,
    pub(crate) retry: bool,
    pub(crate) publish: bool,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRow {
    pub(crate) id: Uuid,
    pub(crate) agent_run_id: Uuid,
    pub(crate) kind: String,
    pub(crate) summary: String,
    pub(crate) options: serde_json::Value,
    pub(crate) status: String,
    pub(crate) response: Option<serde_json::Value>,
    pub(crate) created_at: String,
    #[serde(skip_serializing)]
    pub(crate) repository_id: Uuid,
    #[serde(skip_serializing)]
    pub(crate) requesting_actor_id: Uuid,
    #[serde(skip_serializing)]
    pub(crate) eligible_role: String,
    #[serde(skip_serializing)]
    pub(crate) human_required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalView {
    #[serde(flatten)]
    pub(crate) approval: ApprovalRow,
    pub(crate) capabilities: ApprovalCapabilities,
}

#[derive(Debug, Serialize)]
pub(crate) struct ApprovalCapabilities {
    pub(crate) respond: bool,
}

#[derive(Debug, FromRow)]
struct TimelineEntryRow {
    id: Uuid,
    sequence: i64,
    kind: String,
    created_at: String,
    message_id: Option<Uuid>,
    message_created_at: Option<String>,
    author_actor_id: Option<Uuid>,
    author_kind: Option<String>,
    author_name: Option<String>,
    author_handle: Option<String>,
    content: Option<serde_json::Value>,
    plain_text: Option<String>,
    mentioned_actors: serde_json::Value,
    referenced_work_items: serde_json::Value,
    agent_run_event_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    event_kind: Option<String>,
    event_summary: Option<String>,
    event_created_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct MessageResultRow {
    message_id: Uuid,
    message_created_at: String,
    entry_id: Uuid,
    entry_created_at: String,
    author_kind: String,
    author_name: String,
    author_handle: String,
}

#[derive(Debug, FromRow)]
struct ChannelRow {
    id: Uuid,
    project_id: Uuid,
    name: String,
    origin_message_id: Option<Uuid>,
    archived_at: Option<String>,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    read_version: i64,
}

#[derive(Debug, FromRow)]
struct ReadStateRow {
    highest_seen_sequence: i64,
    version: i64,
}

#[derive(Debug, FromRow)]
struct MentionActorRow {
    id: Uuid,
    kind: String,
    display_name: String,
    handle: String,
    normalized_name: String,
    normalized_handle: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionActorResponse {
    actors: Vec<ActorView>,
    next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MentionCursor {
    query: String,
    name: String,
    handle: String,
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct SidebarChannelRow {
    id: Uuid,
    name: String,
    archived_at: Option<String>,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
}

#[derive(Debug, FromRow)]
struct SidebarThreadRow {
    id: Uuid,
    parent_channel_id: Uuid,
    origin_message_id: Option<Uuid>,
    name: String,
    archived_at: Option<String>,
    parent_archived_at: Option<String>,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    latest_activity_sequence: i64,
    latest_run_state: Option<String>,
    latest_run_entry_sequence: Option<i64>,
    has_pending_response: bool,
    failure_acknowledged: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidebarResponse {
    project_id: Uuid,
    channels: Vec<SidebarChannelView>,
    capabilities: SidebarCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidebarCapabilities {
    create_channel: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidebarChannelView {
    id: Uuid,
    name: String,
    archived_at: Option<String>,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    unread: bool,
    threads: Vec<SidebarThreadView>,
    hidden_thread_count: usize,
    has_more_threads: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SidebarThreadView {
    id: Uuid,
    parent_channel_id: Uuid,
    origin_message_id: Option<Uuid>,
    name: String,
    archived_at: Option<String>,
    parent_archived_at: Option<String>,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    attention: ThreadAttention,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadAttention {
    kind: &'static str,
    label: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveConversationResponse {
    conversation: ConversationView,
    fallback: ArchiveFallback,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveFallback {
    kind: &'static str,
    conversation_id: Option<Uuid>,
}

async fn list_channels(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ChannelListResponse>, AppError> {
    let access = authorized_project(&state, &principal, project_id, Action::ProjectView).await?;
    if access.project_archived {
        return Err(AppError::NotFound("project"));
    }

    let rows = sqlx::query_as::<_, ChannelRow>(
        r#"
        SELECT
            conversation.id,
            conversation.project_id,
            conversation.name,
            conversation.origin_message_id,
            conversation.archived_at::text AS archived_at,
            conversation.version,
            conversation.next_entry_sequence - 1 AS highest_sequence,
            COALESCE(read_state.highest_seen_sequence, 0) AS highest_seen_sequence,
            COALESCE(read_state.version, 0) AS read_version
        FROM conversations conversation
        LEFT JOIN conversation_read_states read_state
          ON read_state.project_id = conversation.project_id
         AND read_state.conversation_id = conversation.id
         AND read_state.actor_id = $2
        WHERE conversation.project_id = $1
          AND conversation.kind = 'channel'
          AND conversation.archived_at IS NULL
        ORDER BY lower(conversation.name), conversation.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(MAX_CHANNELS_PER_PROJECT)
    .fetch_all(&state.pool)
    .await?;
    let channels = rows
        .into_iter()
        .map(|row| {
            let highest_sequence = row.highest_sequence;
            let read_state = ReadStateView {
                highest_seen_sequence: row.highest_seen_sequence,
                version: row.read_version,
            };
            ChannelListItem {
                conversation: ConversationView {
                    id: row.id,
                    project_id: row.project_id,
                    kind: "channel".to_owned(),
                    parent_channel_id: None,
                    origin_message_id: row.origin_message_id,
                    name: row.name,
                    archived_at: row.archived_at,
                    version: row.version,
                },
                highest_sequence,
                unread: highest_sequence > read_state.highest_seen_sequence,
                read_state,
            }
        })
        .collect();
    let create_channel = policy_allows(
        &principal,
        &access.membership_role,
        access.membership_enabled,
        project_id,
        Action::ChannelCreate,
        ProjectScope::project(project_id),
    )?;
    Ok(Json(ChannelListResponse {
        channels,
        capabilities: ChannelListCapabilities { create_channel },
    }))
}

async fn create_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateChannelRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let name = normalize_conversation_name(&request.name)?;
    let command = CreateChannelRequest { name };
    let request_hash = RequestHash::for_value(&command)?;
    let scope = idempotency::resource_bound_scope("project", project_id, "channel_create")?;

    let mut transaction = state.pool.begin().await?;
    let access = lock_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::ChannelCreate,
    )
    .await?;
    if access.project_archived {
        return Err(AppError::NotFound("project"));
    }
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: ConversationCreateResponse = replay.decode()?;
            transaction.commit().await?;
            return created_conversation_response(replay.status(), &body);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    let conversation_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES ($1, $2, 'channel', $3)
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(&command.name)
    .execute(&mut *transaction)
    .await;
    map_conversation_insert(inserted)?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        )
        VALUES ($1, $2, $3, 0)
        "#,
    )
    .bind(project_id)
    .bind(conversation_id)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    enqueue_event(
        &mut transaction,
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id,
            kind: OutboxConversationKind::Channel,
            version: 1,
        }),
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "conversation.channel_created",
        conversation_id,
        serde_json::json!({}),
    )
    .await?;
    let body = ConversationCreateResponse {
        conversation: ConversationView {
            id: conversation_id,
            project_id,
            kind: "channel".to_owned(),
            parent_channel_id: None,
            origin_message_id: None,
            name: command.name,
            archived_at: None,
            version: 1,
        },
        read_state: ReadStateView {
            highest_seen_sequence: 0,
            version: 1,
        },
    };
    finish_idempotent_command(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    created_conversation_response(StatusCode::CREATED, &body)
}

async fn create_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(channel_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateThreadRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let command = CreateThreadRequest {
        name: normalize_conversation_name(&request.name)?,
        origin_message_id: request.origin_message_id,
    };
    let request_hash = RequestHash::for_value(&command)?;
    let scope = idempotency::resource_bound_scope("channel", channel_id, "thread_create")?;

    let mut transaction = state.pool.begin().await?;
    let channel = lock_conversation_access(
        &mut transaction,
        &principal,
        channel_id,
        Action::ThreadCreate,
        false,
    )
    .await?;
    require_conversation_kind(&channel, "channel")?;
    require_active_conversation(&channel)?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: ConversationCreateResponse = replay.decode()?;
            transaction.commit().await?;
            return created_conversation_response(replay.status(), &body);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    if let Some(origin_message_id) = command.origin_message_id {
        let origin_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM messages
                WHERE id = $1
                  AND project_id = $2
                  AND conversation_id = $3
            )
            "#,
        )
        .bind(origin_message_id)
        .bind(channel.project_id)
        .bind(channel.id)
        .fetch_one(&mut *transaction)
        .await?;
        if !origin_exists {
            return Err(AppError::Validation("Thread origin Message"));
        }
    }

    let thread_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, origin_message_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, $5)
        "#,
    )
    .bind(thread_id)
    .bind(channel.project_id)
    .bind(channel.id)
    .bind(command.origin_message_id)
    .bind(&command.name)
    .execute(&mut *transaction)
    .await;
    map_conversation_insert(inserted)?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        )
        VALUES ($1, $2, $3, 0)
        "#,
    )
    .bind(channel.project_id)
    .bind(thread_id)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    enqueue_event(
        &mut transaction,
        channel.project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: thread_id,
            kind: OutboxConversationKind::Thread,
            version: 1,
        }),
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        channel.project_id,
        "conversation.thread_created",
        thread_id,
        serde_json::json!({"parent_channel_id": channel.id}),
    )
    .await?;
    let body = ConversationCreateResponse {
        conversation: ConversationView {
            id: thread_id,
            project_id: channel.project_id,
            kind: "thread".to_owned(),
            parent_channel_id: Some(channel.id),
            origin_message_id: command.origin_message_id,
            name: command.name,
            archived_at: None,
            version: 1,
        },
        read_state: ReadStateView {
            highest_seen_sequence: 0,
            version: 1,
        },
    };
    finish_idempotent_command(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    created_conversation_response(StatusCode::CREATED, &body)
}

async fn send_channel_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SendMessageRequest>,
) -> Result<Response, AppError> {
    send_message(
        &state,
        &headers,
        &principal,
        conversation_id,
        "channel",
        request,
    )
    .await
}

async fn send_thread_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SendMessageRequest>,
) -> Result<Response, AppError> {
    send_message(
        &state,
        &headers,
        &principal,
        conversation_id,
        "thread",
        request,
    )
    .await
}

async fn send_message(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    expected_kind: &'static str,
    request: SendMessageRequest,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let idempotency_key = IdempotencyKey::from_headers(headers)?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let request_hash = RequestHash::for_value(&content)?;
    let scope = idempotency::resource_bound_scope("conversation", conversation_id, "message_send")?;

    let mut transaction = state.pool.begin().await?;
    let conversation = lock_conversation_access(
        &mut transaction,
        principal,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    require_conversation_kind(&conversation, expected_kind)?;
    require_active_conversation(&conversation)?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: MessageAppendResponse = replay.decode()?;
            transaction.commit().await?;
            return created_message_response(replay.status(), &body);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    let mention_ids = content.mentioned_actor_ids();
    let mention_rows = if mention_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT membership.actor_id, membership.handle
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1
              AND membership.actor_id = ANY($2)
              AND membership.enabled
              AND actor.status = 'active'
            FOR SHARE OF membership, actor
            "#,
        )
        .bind(conversation.project_id)
        .bind(&mention_ids)
        .fetch_all(&mut *transaction)
        .await?
    };
    let handles = mention_rows.into_iter().collect::<HashMap<_, _>>();
    let work_item_references = content.work_item_references();
    let work_item_ids = work_item_references
        .iter()
        .map(|(_, work_item_id)| *work_item_id)
        .collect::<Vec<_>>();
    let work_item_rows = if work_item_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, (Uuid, String, String, String, Option<String>)>(
            r#"
            SELECT item.id, project.project_key || '-' || item.number::text,
                   item.title, status.semantic_category,
                   item.archived_at::text AS archived_at
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            WHERE item.project_id = $1 AND item.id = ANY($2)
            FOR SHARE OF item
            "#,
        )
        .bind(conversation.project_id)
        .bind(&work_item_ids)
        .fetch_all(&mut *transaction)
        .await?
    };
    let work_item_labels = work_item_rows
        .iter()
        .map(|row| (row.0, row.1.clone()))
        .collect::<HashMap<_, _>>();
    let (content, plain_text, mentioned_actor_ids) = content
        .validate_with_resources(
            |actor_id| handles.get(&actor_id).cloned(),
            |work_item_id| work_item_labels.get(&work_item_id).cloned(),
        )
        .map_err(message_content_error)?
        .into_parts();
    let content_json = serde_json::to_value(&content)
        .map_err(|_| AppError::Internal("serialize validated Message content"))?;

    let message_id = Uuid::now_v7();
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
    .bind(conversation.project_id)
    .bind(conversation.id)
    .bind(principal.actor_id)
    .bind(&content_json)
    .bind(&plain_text)
    .execute(&mut *transaction)
    .await?;
    for actor_id in &mentioned_actor_ids {
        sqlx::query(
            r#"
            INSERT INTO message_mentions (project_id, message_id, actor_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(conversation.project_id)
        .bind(message_id)
        .bind(actor_id)
        .execute(&mut *transaction)
        .await?;
    }
    for (node_index, work_item_id) in &work_item_references {
        sqlx::query(
            r#"
            INSERT INTO message_work_item_references (
                project_id, message_id, work_item_id, node_index
            )
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(conversation.project_id)
        .bind(message_id)
        .bind(work_item_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("message content"))?)
        .execute(&mut *transaction)
        .await?;
    }
    let sequence = allocate_conversation_sequence(&mut transaction, conversation.id).await?;
    let entry_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, message_id
        )
        VALUES ($1, $2, $3, 'message', $4)
        "#,
    )
    .bind(entry_id)
    .bind(conversation.id)
    .bind(sequence)
    .bind(message_id)
    .execute(&mut *transaction)
    .await?;
    enqueue_event(
        &mut transaction,
        conversation.project_id,
        OutboxEventPayload::MessageCreated(MessageCreatedPayload {
            conversation_id: conversation.id,
            message_id,
            sequence,
            author_actor_id: principal.actor_id,
        }),
    )
    .await?;
    let result = sqlx::query_as::<_, MessageResultRow>(
        r#"
        SELECT
            message.id AS message_id,
            message.created_at::text AS message_created_at,
            entry.id AS entry_id,
            entry.created_at::text AS entry_created_at,
            actor.kind AS author_kind,
            membership.display_name AS author_name,
            membership.handle AS author_handle
        FROM messages message
        JOIN conversation_entries entry ON entry.message_id = message.id
        JOIN actors actor ON actor.id = message.author_actor_id
        JOIN project_memberships membership
          ON membership.project_id = message.project_id
         AND membership.actor_id = message.author_actor_id
        WHERE message.id = $1
        "#,
    )
    .bind(message_id)
    .fetch_one(&mut *transaction)
    .await?;
    let mentioned_actors = fetch_actor_views(
        &mut transaction,
        conversation.project_id,
        &mentioned_actor_ids,
    )
    .await?;
    let referenced_work_items = work_item_rows
        .into_iter()
        .map(
            |(id, key, title, status_category, archived_at)| ConversationWorkItemReferenceView {
                id,
                key,
                title,
                status_category,
                archived_at,
                href: format!("/projects/{}/work-items/{id}", conversation.project_id),
            },
        )
        .collect();
    let message = MessageView {
        id: result.message_id,
        author: ActorView {
            id: principal.actor_id,
            kind: result.author_kind,
            display_name: result.author_name,
            handle: result.author_handle,
            avatar_url: None,
        },
        content: content_json,
        plain_text,
        mentioned_actors,
        referenced_work_items,
        created_at: result.message_created_at,
    };
    let body = MessageAppendResponse {
        conversation_id: conversation.id,
        message: message.clone(),
        entry: TimelineEntryView {
            id: result.entry_id,
            sequence,
            kind: "message".to_owned(),
            created_at: result.entry_created_at,
            message: Some(message),
            worker_event: None,
        },
    };
    finish_idempotent_command(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    created_message_response(StatusCode::CREATED, &body)
}

async fn conversation_timeline(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TimelineQuery>,
) -> Result<Json<TimelineResponse>, AppError> {
    let include_archived = query.include_archived.unwrap_or(false);
    let conversation = authorized_conversation(
        &state,
        &principal,
        conversation_id,
        Action::ProjectView,
        include_archived,
    )
    .await?;
    if !include_archived {
        require_active_conversation(&conversation)?;
    }
    let limit = i64::from(
        query
            .limit
            .unwrap_or(DEFAULT_TIMELINE_PAGE_SIZE)
            .clamp(1, MAX_TIMELINE_PAGE_SIZE),
    );
    let before_sequence = query
        .cursor
        .as_deref()
        .map(|cursor| decode_timeline_cursor(cursor, conversation.id))
        .transpose()?
        .unwrap_or(conversation.next_entry_sequence);
    let mut rows = sqlx::query_as::<_, TimelineEntryRow>(
        r#"
        SELECT
            entry.id,
            entry.sequence,
            entry.kind,
            entry.created_at::text AS created_at,
            message.id AS message_id,
            message.created_at::text AS message_created_at,
            message.author_actor_id,
            author.kind AS author_kind,
            author_membership.display_name AS author_name,
            author_membership.handle AS author_handle,
            message.content,
            message.plain_text,
            COALESCE(mentions.actors, '[]'::jsonb) AS mentioned_actors,
            COALESCE(work_item_references.items, '[]'::jsonb) AS referenced_work_items,
            event.id AS agent_run_event_id,
            event.agent_run_id,
            event.kind AS event_kind,
            event.summary AS event_summary,
            event.created_at::text AS event_created_at
        FROM conversation_entries entry
        LEFT JOIN messages message ON message.id = entry.message_id
        LEFT JOIN actors author ON author.id = message.author_actor_id
        LEFT JOIN project_memberships author_membership
          ON author_membership.project_id = $2
         AND author_membership.actor_id = message.author_actor_id
        LEFT JOIN LATERAL (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', mentioned_actor.id,
                    'kind', mentioned_actor.kind,
                    'displayName', mentioned_membership.display_name,
                    'handle', mentioned_membership.handle,
                    'avatarUrl', NULL
                )
                ORDER BY lower(mentioned_membership.display_name), mentioned_actor.id
            ) AS actors
            FROM message_mentions mention
            JOIN actors mentioned_actor ON mentioned_actor.id = mention.actor_id
            JOIN project_memberships mentioned_membership
              ON mentioned_membership.project_id = mention.project_id
             AND mentioned_membership.actor_id = mention.actor_id
            WHERE mention.message_id = message.id
        ) mentions ON true
        LEFT JOIN LATERAL (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', item.id,
                    'key', project.project_key || '-' || item.number::text,
                    'title', item.title,
                    'statusCategory', status.semantic_category,
                    'archivedAt', item.archived_at::text,
                    'href', '/projects/' || item.project_id::text || '/work-items/' || item.id::text
                ) ORDER BY min_reference.first_node_index, item.id
            ) AS items
            FROM (
                SELECT reference.work_item_id AS referenced_work_item_id,
                       min(reference.node_index) AS first_node_index
                FROM message_work_item_references reference
                WHERE reference.message_id = message.id
                GROUP BY reference.work_item_id
            ) min_reference
            JOIN work_items item ON item.id = min_reference.referenced_work_item_id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
        ) work_item_references ON true
        LEFT JOIN agent_run_events event ON event.id = entry.agent_run_event_id
        WHERE entry.conversation_id = $1
          AND entry.sequence < $3
        ORDER BY entry.sequence DESC
        LIMIT $4
        "#,
    )
    .bind(conversation.id)
    .bind(conversation.project_id)
    .bind(before_sequence)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_older = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if has_older {
        rows.pop();
    }
    rows.reverse();
    let next_cursor = if has_older {
        rows.first()
            .map(|row| encode_timeline_cursor(conversation.id, row.sequence))
            .transpose()?
    } else {
        None
    };
    let entries = rows
        .into_iter()
        .map(timeline_entry_view)
        .collect::<Result<Vec<_>, _>>()?;
    let read_state = fetch_read_state(&state, conversation.id, principal.actor_id).await?;
    let linked_work_items = if conversation.kind == "thread" {
        let value = sqlx::query_scalar::<_, serde_json::Value>(
            r#"
            SELECT COALESCE(jsonb_agg(
                jsonb_build_object(
                    'id', item.id,
                    'key', project.project_key || '-' || item.number::text,
                    'title', item.title,
                    'statusCategory', status.semantic_category,
                    'archivedAt', item.archived_at::text,
                    'href', '/projects/' || item.project_id::text || '/work-items/' || item.id::text
                ) ORDER BY item.number, item.id
            ), '[]'::jsonb)
            FROM work_item_conversation_links link
            JOIN work_items item
              ON item.project_id = link.project_id AND item.id = link.work_item_id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            WHERE link.project_id = $1 AND link.conversation_id = $2
            "#,
        )
        .bind(conversation.project_id)
        .bind(conversation.id)
        .fetch_one(&state.pool)
        .await?;
        serde_json::from_value(value)
            .map_err(|_| AppError::Internal("invalid linked Work Item projection"))?
    } else {
        Vec::new()
    };
    let (active_run, latest_terminal_run, approvals) =
        fetch_run_projection(&state, &principal, &conversation).await?;
    let capabilities = conversation_capabilities(&principal, &conversation)?;
    Ok(Json(TimelineResponse {
        conversation: ConversationView::from(&conversation),
        linked_work_items,
        entries,
        next_cursor,
        highest_sequence: conversation.next_entry_sequence - 1,
        read_state,
        active_run,
        latest_terminal_run,
        approvals,
        capabilities,
    }))
}

async fn mark_conversation_seen(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<MarkSeenRequest>,
) -> Result<Json<ReadStateView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.highest_seen_sequence < 0 {
        return Err(AppError::Validation("highest seen sequence"));
    }
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_conversation_access(
        &mut transaction,
        &principal,
        conversation_id,
        Action::ReadStateUpdate,
        true,
    )
    .await?;
    if request.highest_seen_sequence >= conversation.next_entry_sequence {
        return Err(AppError::Validation("highest seen sequence"));
    }
    let current = sqlx::query_as::<_, ReadStateRow>(
        r#"
        SELECT highest_seen_sequence, version
        FROM conversation_read_states
        WHERE project_id = $1 AND conversation_id = $2 AND actor_id = $3
        FOR UPDATE
        "#,
    )
    .bind(conversation.project_id)
    .bind(conversation.id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(expected_version) = request.expected_version {
        let current_version = current.as_ref().map_or(0, |row| row.version);
        if expected_version != current_version
            && request.highest_seen_sequence
                > current.as_ref().map_or(0, |row| row.highest_seen_sequence)
        {
            return Err(AppError::VersionConflict {
                resource: "conversation_read_state",
                current_version,
            });
        }
    }
    if current
        .as_ref()
        .is_some_and(|row| request.highest_seen_sequence <= row.highest_seen_sequence)
    {
        let row = current.ok_or(AppError::Internal("read-state branch"))?;
        transaction.commit().await?;
        return Ok(Json(ReadStateView {
            highest_seen_sequence: row.highest_seen_sequence,
            version: row.version,
        }));
    }

    let row = sqlx::query_as::<_, ReadStateRow>(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence, version
        )
        VALUES ($1, $2, $3, $4, 1)
        ON CONFLICT (project_id, conversation_id, actor_id) DO UPDATE
        SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
            version = conversation_read_states.version + 1,
            updated_at = now()
        WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
        RETURNING highest_seen_sequence, version
        "#,
    )
    .bind(conversation.project_id)
    .bind(conversation.id)
    .bind(principal.actor_id)
    .bind(request.highest_seen_sequence)
    .fetch_one(&mut *transaction)
    .await?;
    enqueue_event(
        &mut transaction,
        conversation.project_id,
        OutboxEventPayload::ReadStateUpdated(ReadStateUpdatedPayload {
            conversation_id: conversation.id,
            actor_id: principal.actor_id,
            highest_seen_sequence: row.highest_seen_sequence,
            version: row.version,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(ReadStateView {
        highest_seen_sequence: row.highest_seen_sequence,
        version: row.version,
    }))
}

async fn archive_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ArchiveConversationRequest>,
) -> Result<Json<ArchiveConversationResponse>, AppError> {
    archive_conversation(
        &state,
        &headers,
        &principal,
        conversation_id,
        "channel",
        request,
    )
    .await
}

async fn archive_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ArchiveConversationRequest>,
) -> Result<Json<ArchiveConversationResponse>, AppError> {
    archive_conversation(
        &state,
        &headers,
        &principal,
        conversation_id,
        "thread",
        request,
    )
    .await
}

async fn archive_conversation(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    expected_kind: &'static str,
    request: ArchiveConversationRequest,
) -> Result<Json<ArchiveConversationResponse>, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_conversation_access(
        &mut transaction,
        principal,
        conversation_id,
        Action::ConversationArchive,
        true,
    )
    .await?;
    require_conversation_kind(&conversation, expected_kind)?;
    if conversation.archived_at.is_some() {
        return Err(AppError::NotFound("conversation"));
    }
    if conversation.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "conversation",
            current_version: conversation.version,
        });
    }
    if conversation.kind == "channel" {
        // Serialize Channel archive with run starts in existing child Threads.
        // The parent lock already serializes concurrent Thread creation.
        let _owned_conversation_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM conversations
            WHERE id = $1 OR parent_channel_id = $1
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .bind(conversation.id)
        .fetch_all(&mut *transaction)
        .await?;
    }
    let active_run = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_runs run
            JOIN conversations owned ON owned.id = run.conversation_id
            WHERE run.state IN (
                'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
            )
              AND (
                  owned.id = $1
                  OR ($2 = 'channel' AND owned.parent_channel_id = $1)
              )
        )
        "#,
    )
    .bind(conversation.id)
    .bind(&conversation.kind)
    .fetch_one(&mut *transaction)
    .await?;
    if active_run {
        return Err(AppError::Conflict(
            "conversation",
            "non-terminal Agent Run prevents archive",
        ));
    }
    let archived = sqlx::query_as::<_, ConversationAccessRow>(
        r#"
        UPDATE conversations
        SET archived_at = now(),
            archived_by_actor_id = $2,
            version = version + 1,
            updated_at = now()
        WHERE id = $1
        RETURNING
            id,
            project_id,
            kind,
            parent_channel_id,
            origin_message_id,
            name,
            version,
            archived_at::text AS archived_at,
            false AS project_archived,
            false AS parent_archived,
            next_entry_sequence,
            $3::text AS membership_role,
            true AS membership_enabled
        "#,
    )
    .bind(conversation.id)
    .bind(principal.actor_id)
    .bind(&conversation.membership_role)
    .fetch_one(&mut *transaction)
    .await?;
    enqueue_event(
        &mut transaction,
        conversation.project_id,
        OutboxEventPayload::ConversationArchived(ConversationArchivedPayload {
            conversation_id: conversation.id,
            version: archived.version,
        }),
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        headers,
        principal,
        conversation.project_id,
        "conversation.archived",
        conversation.id,
        serde_json::json!({"previous_version": conversation.version}),
    )
    .await?;
    transaction.commit().await?;
    let fallback = if conversation.kind == "thread" {
        ArchiveFallback {
            kind: "channel",
            conversation_id: conversation.parent_channel_id,
        }
    } else {
        ArchiveFallback {
            kind: "project_overview",
            conversation_id: None,
        }
    };
    Ok(Json(ArchiveConversationResponse {
        conversation: ConversationView::from(&archived),
        fallback,
    }))
}

async fn search_mentionable_actors(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<MentionSearchQuery>,
) -> Result<Json<MentionActorResponse>, AppError> {
    let access = authorized_project(&state, &principal, project_id, Action::MentionSearch).await?;
    if access.project_archived {
        return Err(AppError::NotFound("project"));
    }
    let search = query.q.unwrap_or_default().trim().to_lowercase();
    if search.chars().count() > MAX_MENTION_SEARCH_CHARACTERS
        || search.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Actor search"));
    }
    let cursor = query
        .cursor
        .as_deref()
        .map(decode_mention_cursor)
        .transpose()?;
    if cursor.as_ref().is_some_and(|cursor| cursor.query != search) {
        return Err(AppError::Invalid("mention cursor"));
    }
    let limit = i64::from(
        query
            .limit
            .unwrap_or(DEFAULT_ACTOR_PAGE_SIZE)
            .clamp(1, MAX_ACTOR_PAGE_SIZE),
    );
    let (cursor_name, cursor_handle, cursor_id) = cursor
        .map(|cursor| (Some(cursor.name), Some(cursor.handle), Some(cursor.id)))
        .unwrap_or((None, None, None));
    let mut rows = sqlx::query_as::<_, MentionActorRow>(
        r#"
        SELECT
            actor.id,
            actor.kind,
            membership.display_name,
            membership.handle,
            lower(membership.display_name) AS normalized_name,
            lower(membership.handle) AS normalized_handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.kind IN ('human', 'execution_worker')
          AND (
              $2 = ''
              OR strpos(lower(membership.display_name), $2) > 0
              OR strpos(lower(membership.handle), $2) > 0
          )
          AND (
              $3::text IS NULL
              OR (lower(membership.display_name), lower(membership.handle), actor.id)
                 > ($3, $4, $5)
          )
        ORDER BY lower(membership.display_name), lower(membership.handle), actor.id
        LIMIT $6
        "#,
    )
    .bind(project_id)
    .bind(&search)
    .bind(cursor_name)
    .bind(cursor_handle)
    .bind(cursor_id)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if has_more {
        rows.pop();
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|row| {
                encode_mention_cursor(&MentionCursor {
                    query: search.clone(),
                    name: row.normalized_name.clone(),
                    handle: row.normalized_handle.clone(),
                    id: row.id,
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(Json(MentionActorResponse {
        actors: rows
            .into_iter()
            .map(|row| ActorView {
                id: row.id,
                kind: row.kind,
                display_name: row.display_name,
                handle: row.handle,
                avatar_url: None,
            })
            .collect(),
        next_cursor,
    }))
}

async fn project_conversation_sidebar(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SidebarQuery>,
) -> Result<Json<SidebarResponse>, AppError> {
    let access = authorized_project(&state, &principal, project_id, Action::ProjectView).await?;
    if access.project_archived {
        return Err(AppError::NotFound("project"));
    }
    let include_archived = query.include_archived.unwrap_or(false);
    let channels = sqlx::query_as::<_, SidebarChannelRow>(
        r#"
        SELECT
            channel.id,
            channel.name,
            channel.archived_at::text AS archived_at,
            channel.version,
            channel.next_entry_sequence - 1 AS highest_sequence,
            COALESCE(read_state.highest_seen_sequence, 0) AS highest_seen_sequence
        FROM conversations channel
        LEFT JOIN conversation_read_states read_state
          ON read_state.project_id = channel.project_id
         AND read_state.conversation_id = channel.id
         AND read_state.actor_id = $2
        WHERE channel.project_id = $1
          AND channel.kind = 'channel'
          AND ($3 OR channel.archived_at IS NULL)
        ORDER BY channel.archived_at NULLS FIRST, lower(channel.name), channel.id
        LIMIT $4
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(include_archived)
    .bind(MAX_CHANNELS_PER_PROJECT)
    .fetch_all(&state.pool)
    .await?;
    let threads = sqlx::query_as::<_, SidebarThreadRow>(
        r#"
        SELECT
            thread.id,
            thread.parent_channel_id,
            thread.origin_message_id,
            thread.name,
            thread.archived_at::text AS archived_at,
            parent.archived_at::text AS parent_archived_at,
            thread.version,
            thread.next_entry_sequence - 1 AS highest_sequence,
            COALESCE(read_state.highest_seen_sequence, 0) AS highest_seen_sequence,
            COALESCE(
                (extract(epoch FROM latest_entry.created_at) * 1000000)::bigint,
                (extract(epoch FROM thread.created_at) * 1000000)::bigint
            ) AS latest_activity_sequence,
            latest_run.state AS latest_run_state,
            latest_run.entry_sequence AS latest_run_entry_sequence,
            COALESCE(latest_run.has_pending_response, false) AS has_pending_response,
            COALESCE(latest_run.failure_acknowledged, false) AS failure_acknowledged
        FROM conversations thread
        JOIN conversations parent
          ON parent.project_id = thread.project_id
         AND parent.id = thread.parent_channel_id
        LEFT JOIN conversation_read_states read_state
          ON read_state.project_id = thread.project_id
         AND read_state.conversation_id = thread.id
         AND read_state.actor_id = $2
        LEFT JOIN LATERAL (
            SELECT entry.created_at
            FROM conversation_entries entry
            WHERE entry.conversation_id = thread.id
            ORDER BY entry.sequence DESC
            LIMIT 1
        ) latest_entry ON true
        LEFT JOIN LATERAL (
            SELECT
                run.id,
                run.state,
                (
                    SELECT max(entry.sequence)
                    FROM conversation_entries entry
                    JOIN agent_run_events event ON event.id = entry.agent_run_event_id
                    WHERE event.agent_run_id = run.id
                ) AS entry_sequence,
                EXISTS (
                    SELECT 1
                    FROM agent_approvals approval
                    WHERE approval.agent_run_id = run.id
                      AND approval.status = 'pending'
                      AND approval.eligible_role = $3
                      AND $3 IN ('owner', 'admin', 'member')
                      AND (NOT approval.human_required OR $4 = 'human')
                ) AS has_pending_response,
                EXISTS (
                    SELECT 1
                    FROM agent_run_acknowledgements acknowledgement
                    WHERE acknowledgement.agent_run_id = run.id
                      AND acknowledgement.actor_id = $2
                ) AS failure_acknowledged
            FROM agent_runs run
            WHERE run.conversation_id = thread.id
            ORDER BY run.created_at DESC, run.id DESC
            LIMIT 1
        ) latest_run ON true
        WHERE thread.project_id = $1
          AND thread.kind = 'thread'
          AND ($5 OR (thread.archived_at IS NULL AND parent.archived_at IS NULL))
        ORDER BY latest_activity_sequence DESC, thread.id DESC
        LIMIT $6
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(&access.membership_role)
    .bind(&principal.actor_kind)
    .bind(include_archived)
    .bind(MAX_THREADS_PER_SIDEBAR)
    .fetch_all(&state.pool)
    .await?;

    let expanded_channel_id = query.expanded_channel_id;
    let selected_channel_id = query.selected_channel_id;
    let thread_views = project_sidebar_threads(
        threads,
        selected_channel_id,
        expanded_channel_id,
        include_archived,
    );
    let channels = channels
        .into_iter()
        .map(|channel| {
            let (threads, hidden_thread_count) =
                thread_views.get(&channel.id).cloned().unwrap_or_default();
            SidebarChannelView {
                id: channel.id,
                name: channel.name,
                archived_at: channel.archived_at,
                version: channel.version,
                highest_sequence: channel.highest_sequence,
                highest_seen_sequence: channel.highest_seen_sequence,
                unread: channel.highest_sequence > channel.highest_seen_sequence,
                threads,
                hidden_thread_count,
                has_more_threads: hidden_thread_count > 0,
            }
        })
        .collect();
    let create_channel = policy_allows(
        &principal,
        &access.membership_role,
        access.membership_enabled,
        project_id,
        Action::ChannelCreate,
        ProjectScope::project(project_id),
    )?;
    Ok(Json(SidebarResponse {
        project_id,
        channels,
        capabilities: SidebarCapabilities { create_channel },
    }))
}

fn project_sidebar_threads(
    rows: Vec<SidebarThreadRow>,
    selected_channel_id: Option<Uuid>,
    expanded_channel_id: Option<Uuid>,
    include_archived: bool,
) -> HashMap<Uuid, (Vec<SidebarThreadView>, usize)> {
    let mut grouped: HashMap<Uuid, Vec<(SidebarThreadView, i64, bool)>> = HashMap::new();
    for row in rows {
        let attention = thread_attention(&row);
        let is_attention = matches!(
            attention.kind,
            "response_required" | "active" | "failed" | "completed"
        );
        grouped.entry(row.parent_channel_id).or_default().push((
            SidebarThreadView {
                id: row.id,
                parent_channel_id: row.parent_channel_id,
                origin_message_id: row.origin_message_id,
                name: row.name,
                archived_at: row.archived_at,
                parent_archived_at: row.parent_archived_at,
                version: row.version,
                highest_sequence: row.highest_sequence,
                highest_seen_sequence: row.highest_seen_sequence,
                attention,
            },
            row.latest_activity_sequence,
            is_attention,
        ));
    }

    grouped
        .into_iter()
        .map(|(channel_id, candidates)| {
            let total = candidates.len();
            let expanded = expanded_channel_id == Some(channel_id);
            let selected = selected_channel_id == Some(channel_id);
            let recent_ids = if selected && !expanded {
                let mut by_activity = candidates
                    .iter()
                    .filter(|(thread, _, _)| thread.archived_at.is_none())
                    .collect::<Vec<_>>();
                by_activity.sort_by_key(|(thread, activity, _)| {
                    (Reverse(*activity), Reverse(thread.id.as_u128()))
                });
                by_activity
                    .into_iter()
                    .take(5)
                    .map(|(thread, _, _)| thread.id)
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            let mut visible = candidates
                .into_iter()
                .filter(|(thread, _, attention)| {
                    expanded
                        || *attention
                        || (selected && recent_ids.contains(&thread.id))
                        || (include_archived
                            && (thread.archived_at.is_some()
                                || thread.parent_archived_at.is_some()))
                })
                .collect::<Vec<_>>();
            visible.sort_by_key(|(thread, activity, attention)| {
                (
                    Reverse(*attention),
                    Reverse(*activity),
                    Reverse(thread.id.as_u128()),
                )
            });
            let views = visible
                .into_iter()
                .map(|(thread, _, _)| thread)
                .collect::<Vec<_>>();
            let hidden = total.saturating_sub(views.len());
            (channel_id, (views, hidden))
        })
        .collect()
}

fn thread_attention(row: &SidebarThreadRow) -> ThreadAttention {
    match row.latest_run_state.as_deref() {
        Some("blocked") if row.has_pending_response => ThreadAttention {
            kind: "response_required",
            label: "Needs your response",
        },
        Some("created" | "queued" | "starting" | "working" | "recovering") => ThreadAttention {
            kind: "active",
            label: "Run active",
        },
        Some("failed") if !row.failure_acknowledged => ThreadAttention {
            kind: "failed",
            label: "Run failed",
        },
        Some("completed")
            if row
                .latest_run_entry_sequence
                .is_some_and(|sequence| sequence > row.highest_seen_sequence) =>
        {
            ThreadAttention {
                kind: "completed",
                label: "Run completed",
            }
        }
        _ if row.highest_sequence > row.highest_seen_sequence => ThreadAttention {
            kind: "unread",
            label: "Unread Messages",
        },
        _ => ThreadAttention {
            kind: "settled",
            label: "Seen",
        },
    }
}

async fn authorized_project(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<ProjectAccessRow, AppError> {
    let row = sqlx::query_as::<_, ProjectAccessRow>(
        r#"
        SELECT
            project.id AS project_id,
            project.archived_at IS NOT NULL AS project_archived,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    require_policy(
        principal,
        &row.membership_role,
        row.membership_enabled,
        row.project_id,
        action,
        ProjectScope::project(row.project_id),
    )?;
    Ok(row)
}

async fn lock_project_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<ProjectAccessRow, AppError> {
    let row = sqlx::query_as::<_, ProjectAccessRow>(
        r#"
        SELECT
            project.id AS project_id,
            project.archived_at IS NOT NULL AS project_archived,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
        WHERE project.id = $1
        FOR SHARE OF project, membership
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    require_policy(
        principal,
        &row.membership_role,
        row.membership_enabled,
        row.project_id,
        action,
        ProjectScope::project(row.project_id),
    )?;
    Ok(row)
}

async fn authorized_conversation(
    state: &AppState,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    action: Action,
    include_archived: bool,
) -> Result<ConversationAccessRow, AppError> {
    let row = fetch_conversation_access(&state.pool, principal.actor_id, conversation_id)
        .await?
        .ok_or(AppError::NotFound("conversation"))?;
    authorize_conversation_row(principal, &row, action, include_archived)?;
    Ok(row)
}

async fn fetch_conversation_access(
    pool: &sqlx::PgPool,
    actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<Option<ConversationAccessRow>, AppError> {
    Ok(sqlx::query_as::<_, ConversationAccessRow>(
        r#"
        SELECT
            conversation.id,
            conversation.project_id,
            conversation.kind,
            conversation.parent_channel_id,
            conversation.origin_message_id,
            conversation.name,
            conversation.version,
            conversation.archived_at::text AS archived_at,
            project.archived_at IS NOT NULL AS project_archived,
            COALESCE(parent.archived_at IS NOT NULL, false) AS parent_archived,
            conversation.next_entry_sequence,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = $2
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
        "#,
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?)
}

async fn lock_conversation_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    action: Action,
    include_archived: bool,
) -> Result<ConversationAccessRow, AppError> {
    let row = sqlx::query_as::<_, ConversationAccessRow>(
        r#"
        SELECT
            conversation.id,
            conversation.project_id,
            conversation.kind,
            conversation.parent_channel_id,
            conversation.origin_message_id,
            conversation.name,
            conversation.version,
            conversation.archived_at::text AS archived_at,
            project.archived_at IS NOT NULL AS project_archived,
            COALESCE(parent.archived_at IS NOT NULL, false) AS parent_archived,
            conversation.next_entry_sequence,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = $2
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
        FOR UPDATE OF conversation
        FOR SHARE OF project, membership
        "#,
    )
    .bind(conversation_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    authorize_conversation_row(principal, &row, action, include_archived)?;
    if let Some(parent_channel_id) = row.parent_channel_id {
        let parent_active = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT archived_at IS NULL
            FROM conversations
            WHERE id = $1 AND project_id = $2 AND kind = 'channel'
            FOR SHARE
            "#,
        )
        .bind(parent_channel_id)
        .bind(row.project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("conversation"))?;
        if !include_archived && !parent_active {
            return Err(AppError::NotFound("conversation"));
        }
    }
    Ok(row)
}

fn authorize_conversation_row(
    principal: &AuthenticatedActor,
    row: &ConversationAccessRow,
    action: Action,
    include_archived: bool,
) -> Result<(), AppError> {
    if row.project_archived
        || (!include_archived
            && (row.archived_at.is_some() || (row.kind == "thread" && row.parent_archived)))
    {
        return Err(AppError::NotFound("conversation"));
    }
    let scope = if row.kind == "thread" {
        ProjectScope::thread(row.project_id, row.id)
    } else {
        ProjectScope::project(row.project_id)
    };
    require_policy(
        principal,
        &row.membership_role,
        row.membership_enabled,
        row.project_id,
        action,
        scope,
    )
}

fn require_active_conversation(row: &ConversationAccessRow) -> Result<(), AppError> {
    if row.project_archived
        || row.archived_at.is_some()
        || (row.kind == "thread" && row.parent_archived)
    {
        Err(AppError::NotFound("conversation"))
    } else {
        Ok(())
    }
}

fn require_conversation_kind(
    row: &ConversationAccessRow,
    expected: &'static str,
) -> Result<(), AppError> {
    if row.kind == expected {
        Ok(())
    } else {
        Err(AppError::NotFound("conversation"))
    }
}

fn require_policy(
    principal: &AuthenticatedActor,
    role: &str,
    enabled: bool,
    project_id: Uuid,
    action: Action,
    scope: ProjectScope,
) -> Result<(), AppError> {
    if policy_allows(principal, role, enabled, project_id, action, scope)? {
        Ok(())
    } else {
        // Resource lookup and authorization intentionally collapse to the same
        // response at the Conversation HTTP boundary.
        Err(AppError::NotFound("conversation"))
    }
}

fn policy_allows(
    principal: &AuthenticatedActor,
    role: &str,
    enabled: bool,
    project_id: Uuid,
    action: Action,
    scope: ProjectScope,
) -> Result<bool, AppError> {
    let subject = Subject::new(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            project_id,
            parse_membership_role(role)?,
            enabled,
        )),
        None,
    );
    Ok(matches!(
        authorize(&subject, action, Resource::Project(scope)),
        Decision::Allow
    ))
}

fn parse_actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

fn parse_membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

fn conversation_capabilities(
    principal: &AuthenticatedActor,
    conversation: &ConversationAccessRow,
) -> Result<ConversationCapabilities, AppError> {
    let active = !conversation.project_archived
        && conversation.archived_at.is_none()
        && !(conversation.kind == "thread" && conversation.parent_archived);
    let scope = if conversation.kind == "thread" {
        ProjectScope::thread(conversation.project_id, conversation.id)
    } else {
        ProjectScope::project(conversation.project_id)
    };
    Ok(ConversationCapabilities {
        view: policy_allows(
            principal,
            &conversation.membership_role,
            conversation.membership_enabled,
            conversation.project_id,
            Action::ProjectView,
            scope,
        )?,
        send_message: active
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::MessageSend,
                scope,
            )?,
        archive: active
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::ConversationArchive,
                scope,
            )?,
        mark_seen: policy_allows(
            principal,
            &conversation.membership_role,
            conversation.membership_enabled,
            conversation.project_id,
            Action::ReadStateUpdate,
            scope,
        )?,
        worker_invoke: active
            && conversation.kind == "thread"
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::WorkerInvoke,
                scope,
            )?,
    })
}

async fn fetch_read_state(
    state: &AppState,
    conversation_id: Uuid,
    actor_id: Uuid,
) -> Result<ReadStateView, AppError> {
    let row = sqlx::query_as::<_, ReadStateRow>(
        r#"
        SELECT highest_seen_sequence, version
        FROM conversation_read_states
        WHERE conversation_id = $1 AND actor_id = $2
        "#,
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map_or(
        ReadStateView {
            highest_seen_sequence: 0,
            version: 0,
        },
        |row| ReadStateView {
            highest_seen_sequence: row.highest_seen_sequence,
            version: row.version,
        },
    ))
}

async fn fetch_actor_views(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<ActorView>, AppError> {
    if actor_ids.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(FromRow)]
    struct Row {
        id: Uuid,
        kind: String,
        display_name: String,
        handle: String,
    }
    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.actor_id = ANY($2)
        ORDER BY lower(membership.display_name), actor.id
        "#,
    )
    .bind(project_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ActorView {
            id: row.id,
            kind: row.kind,
            display_name: row.display_name,
            handle: row.handle,
            avatar_url: None,
        })
        .collect())
}

fn timeline_entry_view(row: TimelineEntryRow) -> Result<TimelineEntryView, AppError> {
    let message = match row.message_id {
        Some(id) => {
            let mentioned_actors =
                serde_json::from_value::<Vec<ActorView>>(row.mentioned_actors)
                    .map_err(|_| AppError::Internal("decode timeline Message mentions"))?;
            let referenced_work_items = serde_json::from_value::<
                Vec<ConversationWorkItemReferenceView>,
            >(row.referenced_work_items)
            .map_err(|_| AppError::Internal("decode timeline Work Item references"))?;
            Some(MessageView {
                id,
                author: ActorView {
                    id: row
                        .author_actor_id
                        .ok_or(AppError::Internal("timeline Message missing author Actor"))?,
                    kind: row
                        .author_kind
                        .ok_or(AppError::Internal("timeline Message missing author kind"))?,
                    display_name: row
                        .author_name
                        .unwrap_or_else(|| "Former member".to_owned()),
                    handle: row
                        .author_handle
                        .unwrap_or_else(|| "former-member".to_owned()),
                    avatar_url: None,
                },
                content: row
                    .content
                    .ok_or(AppError::Internal("timeline Message missing content"))?,
                plain_text: row.plain_text.unwrap_or_default(),
                mentioned_actors,
                referenced_work_items,
                created_at: row
                    .message_created_at
                    .unwrap_or_else(|| row.created_at.clone()),
            })
        }
        None => None,
    };
    let worker_event = match row.agent_run_event_id {
        Some(id) => Some(WorkerEventView {
            id,
            agent_run_id: row.agent_run_id.ok_or(AppError::Internal(
                "timeline Worker event missing Agent Run",
            ))?,
            kind: row
                .event_kind
                .ok_or(AppError::Internal("timeline Worker event missing kind"))?,
            summary: row.event_summary.unwrap_or_default(),
            created_at: row
                .event_created_at
                .unwrap_or_else(|| row.created_at.clone()),
        }),
        None => None,
    };
    Ok(TimelineEntryView {
        id: row.id,
        sequence: row.sequence,
        kind: row.kind,
        created_at: row.created_at,
        message,
        worker_event,
    })
}

async fn fetch_run_projection(
    state: &AppState,
    principal: &AuthenticatedActor,
    conversation: &ConversationAccessRow,
) -> Result<(Option<RunView>, Option<RunView>, Vec<ApprovalView>), AppError> {
    if conversation.kind != "thread" {
        return Ok((None, None, Vec::new()));
    }
    let active_run = sqlx::query_as::<_, RunRow>(
        r#"
        SELECT
            run.id, run.state, run.repository_id, run.repository_workspace_id,
            run.provider_session_id, run.provider_turn_id,
            workspace.branch_name AS branch,
            workspace.base_commit,
            workspace.published_commit,
            workspace.published_at::text AS published_at,
            workspace.publish_failure_category,
            CASE WHEN run.state = 'queued' THEN (
                SELECT count(*) FROM agent_runs queued
                WHERE queued.state = 'queued'
                  AND (queued.created_at, queued.id) <= (run.created_at, run.id)
            ) END AS queue_position,
            run.failure_reason,
            run.created_at::text AS created_at,
            run.updated_at::text AS updated_at
        FROM agent_runs run
        LEFT JOIN repository_workspaces workspace
          ON workspace.id = run.repository_workspace_id
        WHERE run.conversation_id = $1
          AND run.state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT 1
        "#,
    )
    .bind(conversation.id)
    .fetch_optional(&state.pool)
    .await?;
    let latest_terminal_run = sqlx::query_as::<_, RunRow>(
        r#"
        SELECT
            run.id, run.state, run.repository_id, run.repository_workspace_id,
            run.provider_session_id, run.provider_turn_id,
            workspace.branch_name AS branch,
            workspace.base_commit,
            workspace.published_commit,
            workspace.published_at::text AS published_at,
            workspace.publish_failure_category,
            NULL::bigint AS queue_position,
            run.failure_reason,
            run.created_at::text AS created_at,
            run.updated_at::text AS updated_at
        FROM agent_runs run
        LEFT JOIN repository_workspaces workspace
          ON workspace.id = run.repository_workspace_id
        WHERE run.conversation_id = $1
          AND run.state IN ('completed', 'failed', 'canceled', 'interrupted')
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT 1
        "#,
    )
    .bind(conversation.id)
    .fetch_optional(&state.pool)
    .await?;
    let latest_failure_acknowledged = match latest_terminal_run.as_ref() {
        Some(run) if run.state == "failed" => {
            sqlx::query_scalar::<_, bool>(
                r#"
            SELECT EXISTS (
                SELECT 1
                FROM agent_run_acknowledgements acknowledgement
                WHERE acknowledgement.agent_run_id = $1
                  AND acknowledgement.actor_id = $2
            )
            "#,
            )
            .bind(run.id)
            .bind(principal.actor_id)
            .fetch_one(&state.pool)
            .await?
        }
        _ => false,
    };
    let approvals = sqlx::query_as::<_, ApprovalRow>(
        r#"
        SELECT
            approval.id,
            approval.agent_run_id,
            approval.kind,
            approval.summary,
            approval.options,
            approval.status,
            approval.response,
            approval.created_at::text AS created_at,
            run.repository_id,
            approval.requesting_actor_id,
            approval.eligible_role,
            approval.human_required
        FROM agent_approvals approval
        JOIN agent_runs run ON run.id = approval.agent_run_id
        WHERE run.conversation_id = $1
        ORDER BY approval.created_at, approval.id
        LIMIT 200
        "#,
    )
    .bind(conversation.id)
    .fetch_all(&state.pool)
    .await?;
    let active_run = active_run.map(|run| {
        let scope = ProjectScope::run(
            conversation.project_id,
            conversation.id,
            run.id,
            run.repository_id,
        );
        let interrupt = policy_allows(
            principal,
            &conversation.membership_role,
            conversation.membership_enabled,
            conversation.project_id,
            Action::RunInterrupt,
            scope,
        )
        .unwrap_or(false);
        RunView {
            run,
            failure_acknowledged: false,
            capabilities: RunCapabilities {
                interrupt,
                retry: false,
                publish: false,
            },
        }
    });
    let latest_terminal_run = latest_terminal_run.map(|run| {
        let retry = active_run.is_none()
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::WorkerInvoke,
                ProjectScope::run(
                    conversation.project_id,
                    conversation.id,
                    run.id,
                    run.repository_id,
                ),
            )
            .unwrap_or(false);
        let publish = active_run.is_none()
            && run.repository_workspace_id.is_some()
            && state.config.github.is_some()
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::RepositoryPublish,
                ProjectScope::run(
                    conversation.project_id,
                    conversation.id,
                    run.id,
                    run.repository_id,
                ),
            )
            .unwrap_or(false);
        RunView {
            run,
            failure_acknowledged: latest_failure_acknowledged,
            capabilities: RunCapabilities {
                interrupt: false,
                retry,
                publish,
            },
        }
    });
    let subject = Subject::new(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            conversation.project_id,
            parse_membership_role(&conversation.membership_role)?,
            conversation.membership_enabled,
        )),
        None,
    );
    let approvals = approvals
        .into_iter()
        .map(|approval| {
            let eligibility = parse_membership_role(&approval.eligible_role)?;
            let respond = approval.status == "pending"
                && matches!(
                    authorize(
                        &subject,
                        Action::ApprovalRespond,
                        Resource::Approval {
                            scope: ProjectScope::approval(
                                conversation.project_id,
                                conversation.id,
                                approval.agent_run_id,
                                approval.repository_id,
                                approval.id,
                            ),
                            requirements: ApprovalRequirements {
                                requesting_actor_id: approval.requesting_actor_id,
                                eligibility: ApprovalEligibility::Role(eligibility),
                                human_required: approval.human_required,
                                separation_of_duty: false,
                            },
                        },
                    ),
                    Decision::Allow
                );
            Ok(ApprovalView {
                approval,
                capabilities: ApprovalCapabilities { respond },
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok((active_run, latest_terminal_run, approvals))
}

async fn enqueue_event(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    payload: OutboxEventPayload,
) -> Result<(), AppError> {
    let event = NewOutboxEvent::new(project_id, payload)
        .map_err(|_| AppError::Internal("construct Conversation outbox event"))?;
    outbox::enqueue(transaction, &event).await?;
    Ok(())
}

async fn finish_idempotent_command<T: Serialize>(
    transaction: &mut Transaction<'_, Postgres>,
    reservation: &IdempotencyReservation,
    status: StatusCode,
    body: &T,
) -> Result<(), AppError> {
    idempotency::complete(transaction, reservation, status, body).await
}

fn created_conversation_response(
    status: StatusCode,
    body: &ConversationCreateResponse,
) -> Result<Response, AppError> {
    let kind = if body.conversation.kind == "channel" {
        "channels"
    } else {
        "threads"
    };
    let location = HeaderValue::from_str(&format!("/api/v1/{kind}/{}", body.conversation.id))
        .map_err(|_| AppError::Internal("construct Conversation Location"))?;
    Ok((status, [(LOCATION, location)], Json(body.clone())).into_response())
}

fn created_message_response(
    status: StatusCode,
    body: &MessageAppendResponse,
) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(&format!("/api/v1/messages/{}", body.message.id))
        .map_err(|_| AppError::Internal("construct Message Location"))?;
    Ok((status, [(LOCATION, location)], Json(body.clone())).into_response())
}

fn map_conversation_insert(
    result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
) -> Result<(), AppError> {
    match result {
        Ok(_) => Ok(()),
        Err(error)
            if matches!(
                database_constraint(&error),
                Some(
                    "conversations_active_channel_name_unique"
                        | "conversations_active_thread_name_unique"
                )
            ) =>
        {
            Err(AppError::Conflict(
                "conversation_name",
                "an active Conversation already uses that name",
            ))
        }
        Err(error) => Err(error.into()),
    }
}

fn database_constraint(error: &sqlx::Error) -> Option<&str> {
    match error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    }
}

fn normalize_conversation_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty()
        || name.chars().count() > MAX_CONVERSATION_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Conversation name"));
    }
    Ok(name.to_owned())
}

fn message_content_error(_error: MessageContentError) -> AppError {
    AppError::Validation("Message content")
}

fn encode_timeline_cursor(conversation_id: Uuid, before_sequence: i64) -> Result<String, AppError> {
    if before_sequence < 1 {
        return Err(AppError::Internal("encode timeline cursor"));
    }
    let mut bytes = Vec::with_capacity(25);
    bytes.push(1);
    bytes.extend_from_slice(conversation_id.as_bytes());
    bytes.extend_from_slice(&before_sequence.to_be_bytes());
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_timeline_cursor(value: &str, conversation_id: Uuid) -> Result<i64, AppError> {
    if value.len() > 64 {
        return Err(AppError::Invalid("timeline cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::Invalid("timeline cursor"))?;
    if bytes.len() != 25 || bytes[0] != 1 || bytes[1..17] != *conversation_id.as_bytes() {
        return Err(AppError::Invalid("timeline cursor"));
    }
    let sequence = i64::from_be_bytes(
        bytes[17..25]
            .try_into()
            .map_err(|_| AppError::Invalid("timeline cursor"))?,
    );
    if sequence < 1 {
        return Err(AppError::Invalid("timeline cursor"));
    }
    Ok(sequence)
}

fn encode_mention_cursor(cursor: &MentionCursor) -> Result<String, AppError> {
    let bytes =
        serde_json::to_vec(cursor).map_err(|_| AppError::Internal("serialize mention cursor"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_mention_cursor(value: &str) -> Result<MentionCursor, AppError> {
    if value.len() > 1_024 {
        return Err(AppError::Invalid("mention cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::Invalid("mention cursor"))?;
    let cursor: MentionCursor =
        serde_json::from_slice(&bytes).map_err(|_| AppError::Invalid("mention cursor"))?;
    if cursor.query.chars().count() > MAX_MENTION_SEARCH_CHARACTERS
        || cursor.name.chars().count() > MAX_CONVERSATION_NAME_CHARACTERS
        || cursor.handle.len() > 32
        || cursor.id.is_nil()
    {
        return Err(AppError::Invalid("mention cursor"));
    }
    Ok(cursor)
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit_event(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &'static str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        )
        VALUES (
            $1, $2, $3, $4, 'password_session',
            $5, 'conversation', $6, 'succeeded', $7, $8
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        MentionCursor, SidebarThreadRow, decode_mention_cursor, decode_timeline_cursor,
        encode_mention_cursor, encode_timeline_cursor, normalize_conversation_name,
        project_sidebar_threads, thread_attention,
    };
    use uuid::Uuid;

    fn sidebar_row(state: Option<&str>, run_sequence: Option<i64>) -> SidebarThreadRow {
        SidebarThreadRow {
            id: Uuid::now_v7(),
            parent_channel_id: Uuid::now_v7(),
            origin_message_id: None,
            name: "Thread".to_owned(),
            archived_at: None,
            parent_archived_at: None,
            version: 1,
            highest_sequence: 8,
            highest_seen_sequence: 4,
            latest_activity_sequence: 8,
            latest_run_state: state.map(str::to_owned),
            latest_run_entry_sequence: run_sequence,
            has_pending_response: false,
            failure_acknowledged: false,
        }
    }

    #[test]
    fn timeline_cursors_are_opaque_conversation_bound_and_bounded() {
        let conversation_id = Uuid::now_v7();
        let other_id = Uuid::now_v7();
        let cursor = encode_timeline_cursor(conversation_id, 42).expect("cursor");
        assert_eq!(
            decode_timeline_cursor(&cursor, conversation_id).expect("decode cursor"),
            42
        );
        assert!(decode_timeline_cursor(&cursor, other_id).is_err());
        assert!(decode_timeline_cursor("not+base64", conversation_id).is_err());
        assert!(decode_timeline_cursor(&"x".repeat(65), conversation_id).is_err());
    }

    #[test]
    fn mention_cursors_round_trip_and_reject_unbounded_input() {
        let expected = MentionCursor {
            query: "ag".to_owned(),
            name: "agent".to_owned(),
            handle: "agent".to_owned(),
            id: Uuid::now_v7(),
        };
        let encoded = encode_mention_cursor(&expected).expect("encode cursor");
        let actual = decode_mention_cursor(&encoded).expect("decode cursor");
        assert_eq!(actual.query, expected.query);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.handle, expected.handle);
        assert_eq!(actual.id, expected.id);
        assert!(decode_mention_cursor(&"x".repeat(1_025)).is_err());
    }

    #[test]
    fn conversation_names_are_trimmed_and_character_bounded() {
        assert_eq!(
            normalize_conversation_name("  General  ").expect("name"),
            "General"
        );
        assert!(normalize_conversation_name("").is_err());
        assert!(normalize_conversation_name("bad\nname").is_err());
        assert!(normalize_conversation_name(&"x".repeat(121)).is_err());
    }

    #[test]
    fn thread_attention_has_accessible_durable_precedence() {
        let mut blocked = sidebar_row(Some("blocked"), Some(8));
        blocked.has_pending_response = true;
        assert_eq!(thread_attention(&blocked).kind, "response_required");
        assert_eq!(thread_attention(&blocked).label, "Needs your response");

        blocked.has_pending_response = false;
        blocked.highest_seen_sequence = blocked.highest_sequence;
        assert_eq!(
            thread_attention(&blocked).kind,
            "settled",
            "a blocker for someone else is not the current Actor's active state"
        );

        assert_eq!(
            thread_attention(&sidebar_row(Some("working"), Some(8))).kind,
            "active"
        );
        assert_eq!(
            thread_attention(&sidebar_row(Some("failed"), Some(8))).kind,
            "failed"
        );
        let mut acknowledged = sidebar_row(Some("failed"), Some(8));
        acknowledged.failure_acknowledged = true;
        acknowledged.highest_seen_sequence = acknowledged.highest_sequence;
        assert_eq!(thread_attention(&acknowledged).kind, "settled");
        assert_eq!(
            thread_attention(&sidebar_row(Some("completed"), Some(8))).kind,
            "completed"
        );
        let mut seen = sidebar_row(None, None);
        seen.highest_seen_sequence = seen.highest_sequence;
        assert_eq!(thread_attention(&seen).kind, "settled");
    }

    #[test]
    fn sidebar_unions_attention_with_the_selected_channels_exact_recent_five() {
        let channel_id = Uuid::now_v7();
        let mut rows = (0..7)
            .map(|index| {
                let mut row = sidebar_row(None, None);
                row.parent_channel_id = channel_id;
                row.highest_seen_sequence = row.highest_sequence;
                row.latest_activity_sequence = index;
                row
            })
            .collect::<Vec<_>>();
        let always_visible = rows[0].id;
        rows[0].latest_run_state = Some("working".to_owned());

        let selected = project_sidebar_threads(rows, Some(channel_id), None, false);
        let visible = &selected.get(&channel_id).expect("selected Channel").0;
        assert_eq!(visible.len(), 6, "five recent plus one older active Thread");
        assert!(visible.iter().any(|thread| thread.id == always_visible));
        let unique = visible
            .iter()
            .map(|thread| thread.id)
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), visible.len(), "overlaps are deduplicated");

        let mut unselected_rows = (0..7)
            .map(|index| {
                let mut row = sidebar_row(None, None);
                row.parent_channel_id = channel_id;
                row.highest_seen_sequence = row.highest_sequence;
                row.latest_activity_sequence = index;
                row
            })
            .collect::<Vec<_>>();
        unselected_rows[0].id = always_visible;
        unselected_rows[0].latest_run_state = Some("working".to_owned());
        let unselected = project_sidebar_threads(unselected_rows, None, None, false);
        assert_eq!(unselected.get(&channel_id).expect("Channel").0.len(), 1);

        let mut parent_archived = sidebar_row(None, None);
        parent_archived.parent_channel_id = channel_id;
        parent_archived.parent_archived_at = Some("2026-07-31T00:00:00Z".to_owned());
        parent_archived.highest_seen_sequence = parent_archived.highest_sequence;
        let archived = project_sidebar_threads(vec![parent_archived], None, None, true);
        assert_eq!(
            archived.get(&channel_id).expect("archived Channel").0.len(),
            1,
            "children of archived Channels remain reachable as history"
        );
    }
}
