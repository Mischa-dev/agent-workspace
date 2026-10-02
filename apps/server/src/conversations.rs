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
    external_events::enqueue_external_event,
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
    reactions::{self, ReactionView},
    replies::{self, ReplyRequestView},
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
        .route(
            "/api/v1/conversations/{id}/messages",
            post(send_direct_message),
        )
        .route("/api/v1/channels/{id}/threads", post(create_thread))
        .route(
            "/api/v1/conversations/{id}/threads",
            get(list_threads).post(create_thread),
        )
        .route("/api/v1/threads/{id}", axum::routing::patch(patch_thread))
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
pub(crate) struct ConversationAccessRow {
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) kind: String,
    pub(crate) parent_channel_id: Option<Uuid>,
    origin_message_id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) version: i64,
    archived_at: Option<String>,
    pub(crate) project_archived: bool,
    parent_archived: bool,
    pub(crate) parent_kind: Option<String>,
    pub(crate) thread_status: String,
    pub(crate) thread_outcome: Option<String>,
    pub(crate) next_entry_sequence: i64,
    membership_role: String,
    membership_enabled: bool,
}

impl ConversationAccessRow {
    /// The kind whose reply rules and members apply: a thread under a DM or
    /// group behaves like its parent.
    pub(crate) fn effective_kind(&self) -> &str {
        match (self.kind.as_str(), self.parent_kind.as_deref()) {
            ("thread", Some(parent @ ("dm" | "group"))) => parent,
            (kind, _) => kind,
        }
    }

    /// The conversation whose member list controls who can see this one.
    pub(crate) fn membership_conversation_id(&self) -> Uuid {
        match (self.kind.as_str(), self.parent_kind.as_deref()) {
            ("thread", Some("dm" | "group")) => self.parent_channel_id.unwrap_or(self.id),
            _ => self.id,
        }
    }

    /// Archived itself, or a thread whose parent is archived. Its project
    /// being archived is checked separately.
    pub(crate) fn is_archived(&self) -> bool {
        self.archived_at.is_some() || (self.kind == "thread" && self.parent_archived)
    }

    /// Whether the actor's role in this conversation's project allows an
    /// action here (a thread is its own scope). Archived state isn't
    /// considered, and visibility was settled when the row was loaded: every
    /// lookup that returns this row goes through `conversation_visible_to`.
    pub(crate) fn allows(
        &self,
        actor_id: Uuid,
        actor_kind: ActorKind,
        action: Action,
    ) -> Result<bool, AppError> {
        let scope = if self.kind == "thread" {
            ProjectScope::thread(self.project_id, self.id)
        } else {
            ProjectScope::project(self.project_id)
        };
        let subject = Subject::new(
            actor_id,
            actor_kind,
            true,
            Some(Membership::project(
                self.project_id,
                parse_membership_role(&self.membership_role)?,
                self.membership_enabled,
            )),
            None,
        );
        Ok(matches!(
            authorize(&subject, action, Resource::Project(scope)),
            Decision::Allow
        ))
    }
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
    /// Threads only: where the thread stands, such as Open or Decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_outcome: Option<String>,
    /// Threads only: the kind of conversation the thread lives under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_kind: Option<String>,
}

impl From<&ConversationAccessRow> for ConversationView {
    fn from(row: &ConversationAccessRow) -> Self {
        let is_thread = row.kind == "thread";
        Self {
            id: row.id,
            project_id: row.project_id,
            kind: row.kind.clone(),
            parent_channel_id: row.parent_channel_id,
            origin_message_id: row.origin_message_id,
            name: row.name.clone(),
            archived_at: row.archived_at.clone(),
            version: row.version,
            thread_status: is_thread.then(|| row.thread_status.clone()),
            thread_outcome: if is_thread {
                row.thread_outcome.clone()
            } else {
                None
            },
            parent_kind: if is_thread {
                row.parent_kind.clone()
            } else {
                None
            },
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
    /// The connected app a person sent this through, when that app's
    /// "show via" setting is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// 1 until the message is edited; each edit adds one.
    #[serde(default = "first_revision")]
    pub revision: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Set once the message is removed. Its content and text are then empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<MessageRemovalView>,
    /// Emoji reactions in the order first added, as the viewer sees them.
    /// Always empty for a removed message. `default` because stored
    /// idempotent replays from before reactions have no such field.
    #[serde(default)]
    pub reactions: Vec<ReactionView>,
}

fn first_revision() -> i32 {
    1
}

/// Who removed a message, when, and through which connected app (when that
/// app's "show via" setting is on).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRemovalView {
    pub removed_at: String,
    pub removed_by: ActorView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
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
    /// Edit your own messages here (the same right as sending).
    pub edit_messages: bool,
    /// Remove your own messages, also once the conversation is archived.
    pub remove_own_messages: bool,
    /// Remove anyone's message (project owners and admins; people only).
    pub remove_any_message: bool,
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
    reply_requests: Vec<ReplyRequestView>,
    agent_turns_remaining: Option<i32>,
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
    pub(crate) pull_request_request_state: Option<String>,
    pub(crate) pull_request_failure_category: Option<String>,
    pub(crate) pull_request_number: Option<i64>,
    pub(crate) pull_request_id: Option<Uuid>,
    pub(crate) pull_request_url: Option<String>,
    pub(crate) pull_request_title: Option<String>,
    pub(crate) pull_request_state: Option<String>,
    pub(crate) pull_request_draft: Option<bool>,
    pub(crate) pull_request_checks_state: Option<String>,
    pub(crate) pull_request_review_state: Option<String>,
    pub(crate) pull_request_mergeability_state: Option<String>,
    pub(crate) pull_request_sync_state: Option<String>,
    pub(crate) pull_request_sync_failure_category: Option<String>,
    pub(crate) pull_request_automation_state: Option<String>,
    pub(crate) pull_request_automation_reason: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub(crate) struct RunCapabilities {
    pub(crate) interrupt: bool,
    pub(crate) retry: bool,
    pub(crate) publish: bool,
    pub(crate) create_pull_request: bool,
    pub(crate) refresh_pull_request: bool,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRow {
    pub(crate) id: Uuid,
    pub(crate) agent_run_id: Option<Uuid>,
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
    via: Option<String>,
    revision: Option<i32>,
    edited_at: Option<String>,
    removed_at: Option<String>,
    remover_actor_id: Option<Uuid>,
    remover_kind: Option<String>,
    remover_name: Option<String>,
    remover_handle: Option<String>,
    removed_via: Option<String>,
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
    via: Option<String>,
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
                    thread_status: None,
                    thread_outcome: None,
                    parent_kind: None,
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

    let conversation_id = insert_channel(
        &mut transaction,
        project_id,
        principal.actor_id,
        &command.name,
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
            thread_status: None,
            thread_outcome: None,
            parent_kind: None,
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

/// Create a channel in a Project, read up to date for its creator. The caller
/// has already checked `Action::ChannelCreate`. Shared with MCP.
pub(crate) async fn insert_channel(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    creator_actor_id: Uuid,
    name: &str,
) -> Result<Uuid, AppError> {
    let conversation_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES ($1, $2, 'channel', $3)
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(name)
    .execute(&mut **transaction)
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
    .bind(creator_actor_id)
    .execute(&mut **transaction)
    .await?;
    enqueue_event(
        transaction,
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id,
            kind: OutboxConversationKind::Channel,
            version: 1,
        }),
    )
    .await?;
    Ok(conversation_id)
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
    if !matches!(channel.kind.as_str(), "channel" | "dm" | "group") {
        return Err(AppError::NotFound("conversation"));
    }
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
        let origin_removed = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT removed_at IS NOT NULL
            FROM messages
            WHERE id = $1
              AND project_id = $2
              AND conversation_id = $3
            "#,
        )
        .bind(origin_message_id)
        .bind(channel.project_id)
        .bind(channel.id)
        .fetch_optional(&mut *transaction)
        .await?;
        match origin_removed {
            None => return Err(AppError::Validation("Thread origin Message")),
            Some(true) => return Err(AppError::Conflict("message", "removed")),
            Some(false) => {}
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
            thread_status: Some("Open".to_owned()),
            thread_outcome: None,
            parent_kind: Some(channel.kind.clone()),
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThreadListQuery {
    status: Option<String>,
    include_archived: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListResponse {
    conversation_id: Uuid,
    threads: Vec<ThreadSummaryView>,
}

/// A thread as it appears on a conversation's thread board.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadSummaryView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) outcome: Option<String>,
    pub(crate) origin_message_id: Option<Uuid>,
    pub(crate) message_count: i64,
    pub(crate) unread: bool,
    pub(crate) archived: bool,
    pub(crate) last_activity_at: String,
    pub(crate) status_changed_at: Option<String>,
}

/// The threads under a channel, DM, or group, newest activity first.
async fn list_threads(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ThreadListQuery>,
) -> Result<Json<ThreadListResponse>, AppError> {
    let conversation = authorized_conversation(
        &state,
        &principal,
        conversation_id,
        Action::ProjectView,
        false,
    )
    .await?;
    if !matches!(conversation.kind.as_str(), "channel" | "dm" | "group") {
        return Err(AppError::NotFound("conversation"));
    }
    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let threads = thread_summaries(
        &state.pool,
        conversation.id,
        principal.actor_id,
        status,
        query.include_archived.unwrap_or(false),
        200,
    )
    .await?;
    Ok(Json(ThreadListResponse {
        conversation_id: conversation.id,
        threads,
    }))
}

pub(crate) async fn thread_summaries(
    pool: &sqlx::PgPool,
    parent_id: Uuid,
    viewer_actor_id: Uuid,
    status: Option<&str>,
    include_archived: bool,
    limit: i64,
) -> Result<Vec<ThreadSummaryView>, AppError> {
    Ok(sqlx::query_as::<_, ThreadSummaryView>(
        r#"
        SELECT thread.id, thread.name, thread.thread_status AS status,
               thread.thread_outcome AS outcome, thread.origin_message_id,
               thread.next_entry_sequence - 1 AS message_count,
               thread.next_entry_sequence - 1 > COALESCE(read_state.highest_seen_sequence, 0)
                   AS unread,
               thread.archived_at IS NOT NULL AS archived,
               COALESCE(last_entry.created_at, thread.created_at)::text AS last_activity_at,
               thread.thread_status_changed_at::text AS status_changed_at
        FROM conversations thread
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = thread.id AND read_state.actor_id = $2
        LEFT JOIN LATERAL (
            SELECT entry.created_at FROM conversation_entries entry
            WHERE entry.conversation_id = thread.id
            ORDER BY entry.sequence DESC LIMIT 1
        ) last_entry ON true
        WHERE thread.parent_channel_id = $1
          AND thread.kind = 'thread'
          AND conversation_visible_to(thread.id, $2)
          AND ($3::text IS NULL OR lower(thread.thread_status) = lower($3))
          AND ($4 OR thread.archived_at IS NULL)
        ORDER BY COALESCE(last_entry.created_at, thread.created_at) DESC, thread.id
        LIMIT $5
        "#,
    )
    .bind(parent_id)
    .bind(viewer_actor_id)
    .bind(status)
    .bind(include_archived)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThreadPatch {
    pub(crate) name: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) outcome: Option<String>,
    #[serde(default)]
    pub(crate) clear_outcome: bool,
}

pub(crate) const THREAD_STATUS_SUGGESTIONS: [&str; 6] = [
    "Open",
    "In progress",
    "Decided",
    "Done",
    "Dropped",
    "Blocked",
];
const MAX_THREAD_STATUS_CHARACTERS: usize = 40;
const MAX_THREAD_OUTCOME_CHARACTERS: usize = 1000;

/// Normalize a thread status: suggestions keep their spelling, and any short
/// custom status is allowed.
pub(crate) fn normalize_thread_status(value: &str) -> Result<String, AppError> {
    let status = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if status.is_empty()
        || status.chars().count() > MAX_THREAD_STATUS_CHARACTERS
        || status.chars().any(char::is_control)
    {
        return Err(AppError::Validation("thread status"));
    }
    Ok(THREAD_STATUS_SUGGESTIONS
        .iter()
        .find(|suggestion| suggestion.eq_ignore_ascii_case(&status))
        .map_or(status, |suggestion| (*suggestion).to_owned()))
}

fn normalize_thread_outcome(value: &str) -> Result<Option<String>, AppError> {
    let outcome = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if outcome.chars().count() > MAX_THREAD_OUTCOME_CHARACTERS {
        return Err(AppError::Validation("thread outcome"));
    }
    Ok((!outcome.is_empty()).then_some(outcome))
}

/// Rename a thread or change its status or outcome. Shared by the browser API
/// and MCP. Returns the updated thread.
pub(crate) async fn update_thread(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    thread_id: Uuid,
    patch: &ThreadPatch,
) -> Result<ConversationView, AppError> {
    if patch.name.is_none()
        && patch.status.is_none()
        && patch.outcome.is_none()
        && !patch.clear_outcome
    {
        return Err(AppError::Validation("thread change"));
    }
    let name = patch
        .name
        .as_deref()
        .map(normalize_conversation_name)
        .transpose()?;
    let status = patch
        .status
        .as_deref()
        .map(normalize_thread_status)
        .transpose()?;
    let outcome = match (&patch.outcome, patch.clear_outcome) {
        (Some(_), true) => return Err(AppError::Validation("thread outcome")),
        (Some(value), false) => Some(normalize_thread_outcome(value)?),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let thread = lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        thread_id,
        Action::ThreadCreate,
        false,
    )
    .await?;
    require_conversation_kind(&thread, "thread")?;
    let status_changed = status
        .as_ref()
        .is_some_and(|status| *status != thread.thread_status)
        || outcome
            .as_ref()
            .is_some_and(|outcome| *outcome != thread.thread_outcome);
    let updated = sqlx::query(
        r#"
        UPDATE conversations
        SET name = COALESCE($2, name),
            thread_status = COALESCE($3, thread_status),
            thread_outcome = CASE WHEN $4 THEN $5 ELSE thread_outcome END,
            thread_status_changed_at = CASE WHEN $6 THEN now() ELSE thread_status_changed_at END,
            thread_status_changed_by_actor_id =
                CASE WHEN $6 THEN $7 ELSE thread_status_changed_by_actor_id END,
            version = version + 1,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(thread.id)
    .bind(&name)
    .bind(&status)
    .bind(outcome.is_some())
    .bind(outcome.clone().flatten())
    .bind(status_changed)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await;
    map_conversation_insert(updated)?;
    let version = thread.version + 1;
    enqueue_event(
        transaction,
        thread.project_id,
        OutboxEventPayload::ConversationUpdated(crate::outbox::ConversationUpdatedPayload {
            conversation_id: thread.id,
            version,
        }),
    )
    .await?;
    let mut view = ConversationView::from(&thread);
    view.version = version;
    if let Some(name) = name {
        view.name = name;
    }
    if let Some(status) = status {
        view.thread_status = Some(status);
    }
    if let Some(outcome) = outcome {
        view.thread_outcome = outcome;
    }
    Ok(view)
}

/// Rename a channel or group. Threads are renamed with `update_thread`, and
/// DMs are named after the people in them. Shared with MCP.
pub(crate) async fn rename_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    conversation_id: Uuid,
    name: &str,
) -> Result<ConversationView, AppError> {
    let name = normalize_conversation_name(name)?;
    let conversation = lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    let action = match conversation.kind.as_str() {
        "channel" => Action::ChannelCreate,
        "group" => Action::MessageSend,
        _ => return Err(AppError::Validation("conversation kind")),
    };
    authorize_conversation_row_for(actor_id, actor_kind, &conversation, action, false)?;
    let mut view = ConversationView::from(&conversation);
    if conversation.name == name {
        return Ok(view);
    }
    let updated = sqlx::query(
        "UPDATE conversations SET name = $2, version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(conversation.id)
    .bind(&name)
    .execute(&mut **transaction)
    .await;
    map_conversation_insert(updated)?;
    view.version = conversation.version + 1;
    view.name = name;
    enqueue_event(
        transaction,
        conversation.project_id,
        OutboxEventPayload::ConversationUpdated(crate::outbox::ConversationUpdatedPayload {
            conversation_id: conversation.id,
            version: view.version,
        }),
    )
    .await?;
    Ok(view)
}

async fn patch_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(thread_id): ApiPath<Uuid>,
    ApiJson(patch): ApiJson<ThreadPatch>,
) -> Result<Json<ConversationView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let view = update_thread(
        &mut transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        thread_id,
        &patch,
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        view.project_id,
        "conversation.thread_updated",
        thread_id,
        // No name: one suggested from a message must vanish with it.
        serde_json::json!({ "status": view.thread_status }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(view))
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

async fn send_direct_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SendMessageRequest>,
) -> Result<Response, AppError> {
    // DMs and groups have no kind-specific route. Access still goes through
    // the shared visibility check in `lock_message_conversation`.
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let outcome = append_message_command(
        &state,
        MessageCommandActor {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            mcp_credential: None,
            via_grant_id: None,
        },
        conversation_id,
        None,
        idempotency_key,
        request.content,
        None,
    )
    .await?;
    created_message_response(outcome.status, &outcome.body)
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
    let outcome = append_message_command(
        state,
        MessageCommandActor {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            mcp_credential: None,
            via_grant_id: None,
        },
        conversation_id,
        Some(expected_kind),
        idempotency_key,
        request.content,
        None,
    )
    .await?;
    created_message_response(outcome.status, &outcome.body)
}

#[derive(Clone, Debug)]
pub(crate) struct MessageCommandActor {
    pub(crate) actor_id: Uuid,
    pub(crate) actor_kind: String,
    pub(crate) mcp_credential: Option<MessageMcpCredential>,
    /// The connected app a person is sending through, if any.
    pub(crate) via_grant_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MessageMcpCredential {
    pub(crate) credential_id: Uuid,
    pub(crate) connection_id: Uuid,
}

/// Answer a claimed reply request with this message, atomically.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReplyCompletion {
    pub(crate) request_id: Uuid,
    pub(crate) generation: i32,
}

pub(crate) struct MessageAppendOutcome {
    pub(crate) status: StatusCode,
    pub(crate) body: MessageAppendResponse,
    pub(crate) idempotent_replay: bool,
}

pub(crate) async fn append_message_command(
    state: &AppState,
    actor: MessageCommandActor,
    conversation_id: Uuid,
    expected_kind: Option<&'static str>,
    idempotency_key: IdempotencyKey,
    content: serde_json::Value,
    reply: Option<ReplyCompletion>,
) -> Result<MessageAppendOutcome, AppError> {
    let content = MessageContent::from_json(content).map_err(message_content_error)?;
    let request_hash = RequestHash::for_value(&content)?;
    let scope = idempotency::resource_bound_scope("conversation", conversation_id, "message_send")?;
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_message_conversation(
        &mut transaction,
        &actor,
        conversation_id,
        Action::MessageSend,
    )
    .await?;
    if let Some(expected_kind) = expected_kind {
        require_conversation_kind(&conversation, expected_kind)?;
    }
    require_active_conversation(&conversation)?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        actor.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: MessageAppendResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok(MessageAppendOutcome {
                status: replay.status(),
                body,
                idempotent_replay: true,
            });
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    let ResolvedContent {
        content_json,
        plain_text,
        mentioned_actor_ids,
        work_item_references,
        work_item_rows,
    } = resolve_message_content(&mut transaction, conversation.project_id, content).await?;

    let message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text, via_oauth_grant_id
        )
        VALUES ($1, $2, $3, $4, 1, $5, $6, $7)
        "#,
    )
    .bind(message_id)
    .bind(conversation.project_id)
    .bind(conversation.id)
    .bind(actor.actor_id)
    .bind(&content_json)
    .bind(&plain_text)
    .bind(actor.via_grant_id)
    .execute(&mut *transaction)
    .await?;
    insert_message_projections(
        &mut transaction,
        conversation.project_id,
        message_id,
        &mentioned_actor_ids,
        &work_item_references,
    )
    .await?;
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
            author_actor_id: actor.actor_id,
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
            membership.handle AS author_handle,
            via_client.client_name AS via
        FROM messages message
        JOIN conversation_entries entry ON entry.message_id = message.id
        JOIN actors actor ON actor.id = message.author_actor_id
        JOIN project_memberships membership
          ON membership.project_id = message.project_id
         AND membership.actor_id = message.author_actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = message.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
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
            id: actor.actor_id,
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
        via: result.via,
        revision: 1,
        edited_at: None,
        removed: None,
        reactions: Vec::new(),
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
    if let Some(reply) = reply {
        replies::complete_with_reply(
            &mut transaction,
            reply.request_id,
            reply.generation,
            actor.actor_id,
            conversation.id,
            message_id,
        )
        .await?;
    }
    enqueue_external_message_events(
        &mut transaction,
        &conversation,
        actor.actor_id,
        message_id,
        &mentioned_actor_ids,
    )
    .await?;
    replies::ask_for_replies(
        &mut transaction,
        replies::NewMessage {
            project_id: conversation.project_id,
            conversation_id: conversation.id,
            members_conversation_id: conversation.membership_conversation_id(),
            conversation_kind: conversation.effective_kind(),
            message_id,
            author_actor_id: actor.actor_id,
            author_kind: &actor.actor_kind,
            mentioned_actor_ids: &mentioned_actor_ids,
        },
    )
    .await?;
    if let Some(mcp) = actor.mcp_credential {
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, project_id, authentication_class, action,
                target_kind, target_id, outcome, channel, client_name,
                mcp_credential_id, external_agent_connection_id, metadata
            )
            SELECT $1, $2, $3, 'mcp_bearer', 'conversation.message_sent',
                   'message', $4, 'succeeded', 'mcp', credential.client_name,
                   credential.id, $5, jsonb_build_object('conversation_id', $6)
            FROM mcp_client_credentials credential
            WHERE credential.id = $7
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(actor.actor_id)
        .bind(conversation.project_id)
        .bind(message_id)
        .bind(mcp.connection_id)
        .bind(conversation.id)
        .bind(mcp.credential_id)
        .execute(&mut *transaction)
        .await?;
    }
    finish_idempotent_command(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    Ok(MessageAppendOutcome {
        status: StatusCode::CREATED,
        body,
        idempotent_replay: false,
    })
}

/// Validated message content with what it mentions and references, resolved
/// against the project. Send and edit share it, so both apply the same limits
/// and checks.
pub(crate) struct ResolvedContent {
    pub(crate) content_json: serde_json::Value,
    pub(crate) plain_text: String,
    pub(crate) mentioned_actor_ids: Vec<Uuid>,
    /// (node index, work item) for each work item reference node.
    pub(crate) work_item_references: Vec<(usize, Uuid)>,
    /// id, key, title, status category, archived at.
    pub(crate) work_item_rows: Vec<(Uuid, String, String, String, Option<String>)>,
}

/// Resolve and validate message content: mentioned people must be enabled,
/// active members of the project, and referenced work items must belong to
/// it. Locks both for share, so they can't change before the message lands.
pub(crate) async fn resolve_message_content(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    content: MessageContent,
) -> Result<ResolvedContent, AppError> {
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
        .bind(project_id)
        .bind(&mention_ids)
        .fetch_all(&mut **transaction)
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
        .bind(project_id)
        .bind(&work_item_ids)
        .fetch_all(&mut **transaction)
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
    Ok(ResolvedContent {
        content_json,
        plain_text,
        mentioned_actor_ids,
        work_item_references,
        work_item_rows,
    })
}

/// Write a message's mention and work item reference projections, which must
/// match its current content (the database checks).
pub(crate) async fn insert_message_projections(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    message_id: Uuid,
    mentioned_actor_ids: &[Uuid],
    work_item_references: &[(usize, Uuid)],
) -> Result<(), AppError> {
    for actor_id in mentioned_actor_ids {
        sqlx::query(
            r#"
            INSERT INTO message_mentions (project_id, message_id, actor_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(project_id)
        .bind(message_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    }
    for (node_index, work_item_id) in work_item_references {
        sqlx::query(
            r#"
            INSERT INTO message_work_item_references (
                project_id, message_id, work_item_id, node_index
            )
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(project_id)
        .bind(message_id)
        .bind(work_item_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("message content"))?)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

/// Lock a conversation (FOR UPDATE) for a message command by a person or an
/// agent: the conversation must pass `conversation_visible_to`, an agent's MCP
/// credential and connection must still be valid for the project, and the
/// actor's role must allow `action` (send passes MessageSend; edit and remove
/// pass ProjectView and check their own rules). Anything else is NotFound.
pub(crate) async fn lock_message_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    conversation_id: Uuid,
    action: Action,
) -> Result<ConversationAccessRow, AppError> {
    let mcp_project_id = if let Some(proof) = actor.mcp_credential {
        Some(
            sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT target.project_id
                FROM conversations target, mcp_client_credentials credential
                JOIN external_agent_connections connection
                  ON connection.id = credential.external_agent_connection_id
                JOIN actors actor ON actor.id = credential.actor_id
                WHERE target.id = $4 AND conversation_visible_to(target.id, $3)
                  AND agent_connection_can_access(connection.id, target.project_id)
                  AND credential.id = $1
                  AND connection.id = $2
                  AND credential.actor_id = $3
                  AND actor.kind = 'external_agent'
                  AND actor.status = 'active'
                  AND credential.status = 'active'
                  AND credential.revoked_at IS NULL
                  AND credential.expires_at > now()
                  AND credential.scopes @> ARRAY['write']::text[]
                  AND connection.actor_id = credential.actor_id
                  AND connection.project_id = credential.project_id
                  AND connection.status = 'active'
                  AND connection.scopes = credential.scopes
                FOR SHARE OF credential, connection, actor
                "#,
            )
            .bind(proof.credential_id)
            .bind(proof.connection_id)
            .bind(actor.actor_id)
            .bind(conversation_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("conversation"))?,
        )
    } else {
        None
    };
    let row = sqlx::query_as::<_, ConversationAccessRow>(
        r#"
        SELECT conversation.id, conversation.project_id, conversation.kind,
               conversation.parent_channel_id, conversation.origin_message_id,
               conversation.name, conversation.version,
               conversation.archived_at::text AS archived_at,
               project.archived_at IS NOT NULL AS project_archived,
               COALESCE(parent.archived_at IS NOT NULL, false) AS parent_archived,
               parent.kind AS parent_kind,
               conversation.thread_status,
               conversation.thread_outcome,
               conversation.next_entry_sequence,
               membership.role AS membership_role,
               membership.enabled AS membership_enabled
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN actors actor ON actor.id = $2
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = $2
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
          AND conversation_visible_to(conversation.id, $2)
          AND actor.kind = $3 AND actor.status = 'active'
          AND (
              -- Browser authentication, including the loopback-only development
              -- principal, was validated before entering this command path.
              ($4::uuid IS NULL AND actor.kind = 'human')
              OR
              ($4::uuid = conversation.project_id AND actor.kind = 'external_agent')
          )
        FOR UPDATE OF conversation
        FOR SHARE OF project, actor, organization_membership, membership
        "#,
    )
    .bind(conversation_id)
    .bind(actor.actor_id)
    .bind(&actor.actor_kind)
    .bind(mcp_project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    let scope = if row.kind == "thread" {
        ProjectScope::thread(row.project_id, row.id)
    } else {
        ProjectScope::project(row.project_id)
    };
    let subject = Subject::new(
        actor.actor_id,
        parse_actor_kind(&actor.actor_kind)?,
        true,
        Some(Membership::project(
            row.project_id,
            parse_membership_role(&row.membership_role)?,
            row.membership_enabled,
        )),
        None,
    );
    if !matches!(
        authorize(&subject, action, Resource::Project(scope)),
        Decision::Allow
    ) {
        return Err(AppError::NotFound("conversation"));
    }
    Ok(row)
}

/// Tell mentioned agents' connections they were mentioned. Only agents who
/// can see the conversation are told.
pub(crate) async fn enqueue_external_message_events(
    transaction: &mut Transaction<'_, Postgres>,
    conversation: &ConversationAccessRow,
    author_actor_id: Uuid,
    message_id: Uuid,
    mentioned_actor_ids: &[Uuid],
) -> Result<(), AppError> {
    if mentioned_actor_ids.is_empty() {
        return Ok(());
    }
    // Historical migration tests intentionally exercise this command path on
    // pre-M9 schemas. The durable Message is still valid there; external-agent
    // delivery begins only once the M9 connection table has been installed.
    let external_connections_available = sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('external_agent_connections') IS NOT NULL",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if !external_connections_available {
        return Ok(());
    }
    let connections = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT connection.id, connection.actor_id
        FROM external_agent_connections connection
        JOIN actors actor ON actor.id = connection.actor_id
        JOIN project_memberships membership
          ON membership.project_id = $1
         AND membership.actor_id = connection.actor_id
        WHERE agent_connection_can_access(connection.id, $1)
          AND connection.actor_id = ANY($2)
          AND connection.actor_id <> $3
          AND connection.status = 'active'
          AND actor.status = 'active'
          AND membership.enabled
          AND conversation_visible_to($4, connection.actor_id)
        ORDER BY connection.id
        FOR SHARE OF connection, actor, membership
        "#,
    )
    .bind(conversation.project_id)
    .bind(mentioned_actor_ids)
    .bind(author_actor_id)
    .bind(conversation.id)
    .fetch_all(&mut **transaction)
    .await?;
    let kind = match conversation.kind.as_str() {
        "thread" => "threads",
        "channel" => "channels",
        _ => "messages",
    };
    for (connection_id, _) in connections {
        enqueue_external_event(
            transaction,
            connection_id,
            conversation.project_id,
            "message.mentioned",
            "You were mentioned in a Message",
            "message",
            message_id,
            &if kind == "messages" {
                format!("/messages/{}", conversation.id)
            } else {
                format!(
                    "/projects/{}/{kind}/{}",
                    conversation.project_id, conversation.id
                )
            },
        )
        .await?;
    }
    Ok(())
}

/// One timeline entry with its message (and who removed it) or worker event.
/// Callers append a WHERE clause; `$1` is the conversation and `$2` its project.
const TIMELINE_ENTRY_SELECT: &str = r#"
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
            via_client.client_name AS via,
            message.revision,
            message.edited_at::text AS edited_at,
            message.removed_at::text AS removed_at,
            message.removed_by_actor_id AS remover_actor_id,
            remover_actor.kind AS remover_kind,
            remover.display_name AS remover_name,
            remover.handle AS remover_handle,
            removed_via_client.client_name AS removed_via,
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
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = message.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        LEFT JOIN actors remover_actor ON remover_actor.id = message.removed_by_actor_id
        LEFT JOIN project_memberships remover
          ON remover.project_id = $2
         AND remover.actor_id = message.removed_by_actor_id
        LEFT JOIN mcp_oauth_grants removed_via_grant
          ON removed_via_grant.id = message.removed_via_oauth_grant_id
         AND removed_via_grant.show_via
        LEFT JOIN mcp_oauth_clients removed_via_client
          ON removed_via_client.id = removed_via_grant.client_id
        LEFT JOIN agent_run_events event ON event.id = entry.agent_run_event_id
"#;

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
    let mut rows = sqlx::query_as::<_, TimelineEntryRow>(&format!(
        "{TIMELINE_ENTRY_SELECT}
        WHERE entry.conversation_id = $1
          AND entry.sequence < $3
        ORDER BY entry.sequence DESC
        LIMIT $4"
    ))
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
    let mut entries = rows
        .into_iter()
        .map(timeline_entry_view)
        .collect::<Result<Vec<_>, _>>()?;
    reactions::attach_reactions(
        &state.pool,
        conversation.id,
        &mut entries,
        principal.actor_id,
    )
    .await?;
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
    let (reply_requests, agent_turns_remaining) =
        if matches!(conversation.effective_kind(), "dm" | "group") {
            let (requests, remaining) =
                replies::timeline_requests(&state.pool, conversation.id).await?;
            (requests, Some(remaining))
        } else {
            (Vec::new(), None)
        };
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
        reply_requests,
        agent_turns_remaining,
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
    let (conversation, archived) = set_conversation_archived(
        &mut transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        conversation_id,
        &[expected_kind],
        true,
        Some(request.expected_version),
    )
    .await?;
    // The browser archives only what it can see as active.
    let archived = archived.ok_or(AppError::NotFound("conversation"))?;
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

/// Archive or restore a channel or thread. Shared by the browser API and MCP.
///
/// Returns the conversation as it was, and as it is now if this changed it;
/// archiving something already archived (or restoring something active)
/// changes nothing. Archiving is refused while an Agent Run is active in it
/// or, for a channel, in any of its threads. A thread can't be restored while
/// its channel is archived.
pub(crate) async fn set_conversation_archived(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    conversation_id: Uuid,
    allowed_kinds: &[&str],
    archive: bool,
    expected_version: Option<i64>,
) -> Result<(ConversationAccessRow, Option<ConversationAccessRow>), AppError> {
    let conversation = lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        conversation_id,
        Action::ConversationArchive,
        true,
    )
    .await?;
    if !allowed_kinds.contains(&conversation.kind.as_str()) {
        return Err(AppError::NotFound("conversation"));
    }
    if conversation.archived_at.is_some() == archive {
        return Ok((conversation, None));
    }
    if let Some(expected_version) = expected_version
        && conversation.version != expected_version
    {
        return Err(AppError::VersionConflict {
            resource: "conversation",
            current_version: conversation.version,
        });
    }
    if !archive && conversation.kind == "thread" && conversation.parent_archived {
        return Err(AppError::Conflict(
            "conversation",
            "restore the thread's channel first",
        ));
    }
    if archive {
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
            .fetch_all(&mut **transaction)
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
        .fetch_one(&mut **transaction)
        .await?;
        if active_run {
            return Err(AppError::Conflict(
                "conversation",
                "non-terminal Agent Run prevents archive",
            ));
        }
    }
    let changed = sqlx::query_as::<_, ConversationAccessRow>(
        r#"
        UPDATE conversations
        SET archived_at = CASE WHEN $5 THEN now() ELSE NULL END,
            archived_by_actor_id = CASE WHEN $5 THEN $2 ELSE NULL END,
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
            $4::text AS parent_kind,
            thread_status,
            thread_outcome,
            next_entry_sequence,
            $3::text AS membership_role,
            true AS membership_enabled
        "#,
    )
    .bind(conversation.id)
    .bind(actor_id)
    .bind(&conversation.membership_role)
    .bind(&conversation.parent_kind)
    .bind(archive)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| {
        if matches!(
            database_constraint(&error),
            Some(
                "conversations_active_channel_name_unique"
                    | "conversations_active_thread_name_unique"
            )
        ) {
            AppError::Conflict(
                "conversation_name",
                "an active Conversation already uses that name; rename one of them first",
            )
        } else {
            error.into()
        }
    })?;
    let payload = if archive {
        OutboxEventPayload::ConversationArchived(ConversationArchivedPayload {
            conversation_id: conversation.id,
            version: changed.version,
        })
    } else {
        OutboxEventPayload::ConversationUpdated(crate::outbox::ConversationUpdatedPayload {
            conversation_id: conversation.id,
            version: changed.version,
        })
    };
    enqueue_event(transaction, conversation.project_id, payload).await?;
    Ok((conversation, Some(changed)))
}

async fn search_mentionable_actors(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<MentionSearchQuery>,
) -> Result<Json<MentionActorResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
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
          AND actor.kind IN ('human', 'external_agent', 'execution_worker')
          -- Only offer agents that could answer: viewers cannot send, and a
          -- read-only connection cannot claim a reply.
          AND (
              actor.kind <> 'external_agent'
              OR (
                  membership.role <> 'viewer'
                  AND EXISTS (
                      SELECT 1 FROM external_agent_connections connection
                      WHERE connection.actor_id = actor.id
                        AND 'write' = ANY(connection.scopes)
                        AND agent_connection_can_access(connection.id, membership.project_id)
                  )
              )
          )
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
                      AND approval.eligible_role IN ($3, 'any_authorized')
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
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
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
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
        WHERE project.id = $1
        FOR SHARE OF project, organization_membership, membership
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

pub(crate) async fn fetch_conversation_access(
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
            parent.kind AS parent_kind,
            conversation.thread_status,
            conversation.thread_outcome,
            conversation.next_entry_sequence,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = $2
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
          AND conversation_visible_to(conversation.id, $2)
        "#,
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?)
}

pub(crate) async fn lock_conversation_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    action: Action,
    include_archived: bool,
) -> Result<ConversationAccessRow, AppError> {
    lock_conversation_for(
        transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        conversation_id,
        action,
        include_archived,
    )
    .await
}

/// Lock a conversation for a person or agent, whether they came through a
/// browser session or MCP. Visibility goes through `conversation_visible_to`.
pub(crate) async fn lock_conversation_for(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
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
            parent.kind AS parent_kind,
            conversation.thread_status,
            conversation.thread_outcome,
            conversation.next_entry_sequence,
            membership.role AS membership_role,
            membership.enabled AS membership_enabled
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = $2
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
          AND conversation_visible_to(conversation.id, $2)
        FOR UPDATE OF conversation
        FOR SHARE OF project, organization_membership, membership
        "#,
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    authorize_conversation_row_for(actor_id, actor_kind, &row, action, include_archived)?;
    if let Some(parent_channel_id) = row.parent_channel_id {
        let parent_active = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT archived_at IS NULL
            FROM conversations
            WHERE id = $1 AND project_id = $2 AND kind IN ('channel', 'dm', 'group')
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
    authorize_conversation_row_for(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        row,
        action,
        include_archived,
    )
}

pub(crate) fn authorize_conversation_row_for(
    actor_id: Uuid,
    actor_kind: ActorKind,
    row: &ConversationAccessRow,
    action: Action,
    include_archived: bool,
) -> Result<(), AppError> {
    if row.project_archived || (!include_archived && row.is_archived()) {
        return Err(AppError::NotFound("conversation"));
    }
    if row.allows(actor_id, actor_kind, action)? {
        Ok(())
    } else {
        Err(AppError::NotFound("conversation"))
    }
}

pub(crate) fn require_active_conversation(row: &ConversationAccessRow) -> Result<(), AppError> {
    if row.project_archived || row.is_archived() {
        Err(AppError::NotFound("conversation"))
    } else {
        Ok(())
    }
}

pub(crate) fn require_conversation_kind(
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

pub(crate) fn parse_actor_kind(value: &str) -> Result<ActorKind, AppError> {
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
    let active = !conversation.project_archived && !conversation.is_archived();
    let actor_kind = parse_actor_kind(&principal.actor_kind)?;
    let allows = |action| conversation.allows(principal.actor_id, actor_kind, action);
    Ok(ConversationCapabilities {
        view: allows(Action::ProjectView)?,
        send_message: active && allows(Action::MessageSend)?,
        archive: active && allows(Action::ConversationArchive)?,
        mark_seen: allows(Action::ReadStateUpdate)?,
        worker_invoke: active && conversation.kind == "thread" && allows(Action::WorkerInvoke)?,
        edit_messages: active && allows(Action::MessageSend)?,
        remove_own_messages: !conversation.project_archived && allows(Action::MessageSend)?,
        remove_any_message: !conversation.project_archived
            && actor_kind == ActorKind::Human
            && allows(Action::MessageModerate)?,
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

pub(crate) async fn fetch_actor_views(
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

/// The timeline entry for one message in `conversation`, as the timeline
/// shows it. `conversation` was loaded through a lookup that checks
/// `conversation_visible_to`; a message from any other conversation is None.
pub(crate) async fn fetch_timeline_entry<'e, E>(
    executor: E,
    conversation: &ConversationAccessRow,
    message_id: Uuid,
) -> Result<Option<TimelineEntryView>, AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let sql = format!(
        "{TIMELINE_ENTRY_SELECT}
        WHERE entry.conversation_id = $1 AND entry.message_id = $3"
    );
    sqlx::query_as::<_, TimelineEntryRow>(&sql)
        .bind(conversation.id)
        .bind(conversation.project_id)
        .bind(message_id)
        .fetch_optional(executor)
        .await?
        .map(timeline_entry_view)
        .transpose()
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
            let removed = match row.removed_at {
                Some(removed_at) => Some(MessageRemovalView {
                    removed_at,
                    removed_by: ActorView {
                        id: row.remover_actor_id.ok_or(AppError::Internal(
                            "removed timeline Message missing remover",
                        ))?,
                        kind: row
                            .remover_kind
                            .ok_or(AppError::Internal("removed timeline Message remover kind"))?,
                        display_name: row
                            .remover_name
                            .unwrap_or_else(|| "Former member".to_owned()),
                        handle: row
                            .remover_handle
                            .unwrap_or_else(|| "former-member".to_owned()),
                        avatar_url: None,
                    },
                    via: row.removed_via,
                }),
                None => None,
            };
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
                via: row.via,
                revision: row.revision.unwrap_or(1),
                edited_at: row.edited_at,
                removed,
                reactions: Vec::new(),
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
            pull_request_request.state AS pull_request_request_state,
            pull_request_request.failure_category AS pull_request_failure_category,
            pull_request.pull_request_number,
            pull_request.id AS pull_request_id,
            pull_request.html_url AS pull_request_url,
            pull_request.title AS pull_request_title,
            pull_request.state AS pull_request_state,
            pull_request.draft AS pull_request_draft,
            pull_request.checks_state AS pull_request_checks_state,
            pull_request.review_state AS pull_request_review_state,
            pull_request.mergeability_state AS pull_request_mergeability_state,
            pull_request.sync_state AS pull_request_sync_state,
            pull_request.sync_failure_category AS pull_request_sync_failure_category,
            pull_request.automation_state AS pull_request_automation_state,
            pull_request.automation_reason AS pull_request_automation_reason,
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
        LEFT JOIN github_pull_request_requests pull_request_request
          ON pull_request_request.agent_run_id = run.id
        LEFT JOIN github_pull_requests pull_request
          ON pull_request.id = pull_request_request.github_pull_request_id
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
            pull_request_request.state AS pull_request_request_state,
            pull_request_request.failure_category AS pull_request_failure_category,
            pull_request.pull_request_number,
            pull_request.id AS pull_request_id,
            pull_request.html_url AS pull_request_url,
            pull_request.title AS pull_request_title,
            pull_request.state AS pull_request_state,
            pull_request.draft AS pull_request_draft,
            pull_request.checks_state AS pull_request_checks_state,
            pull_request.review_state AS pull_request_review_state,
            pull_request.mergeability_state AS pull_request_mergeability_state,
            pull_request.sync_state AS pull_request_sync_state,
            pull_request.sync_failure_category AS pull_request_sync_failure_category,
            pull_request.automation_state AS pull_request_automation_state,
            pull_request.automation_reason AS pull_request_automation_reason,
            NULL::bigint AS queue_position,
            run.failure_reason,
            run.created_at::text AS created_at,
            run.updated_at::text AS updated_at
        FROM agent_runs run
        LEFT JOIN repository_workspaces workspace
          ON workspace.id = run.repository_workspace_id
        LEFT JOIN github_pull_request_requests pull_request_request
          ON pull_request_request.agent_run_id = run.id
        LEFT JOIN github_pull_requests pull_request
          ON pull_request.id = pull_request_request.github_pull_request_id
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
                create_pull_request: false,
                refresh_pull_request: false,
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
        let create_pull_request = active_run.is_none()
            && run.published_commit.is_some()
            && run.pull_request_request_state.is_none()
            && state.config.github.is_some()
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::PullRequestCreate,
                ProjectScope::run(
                    conversation.project_id,
                    conversation.id,
                    run.id,
                    run.repository_id,
                ),
            )
            .unwrap_or(false);
        let refresh_pull_request = run.pull_request_id.is_some()
            && state.config.github.is_some()
            && policy_allows(
                principal,
                &conversation.membership_role,
                conversation.membership_enabled,
                conversation.project_id,
                Action::PullRequestRefresh,
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
                create_pull_request,
                refresh_pull_request,
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
            let agent_run_id = approval
                .agent_run_id
                .ok_or(AppError::Internal("run approval without Agent Run"))?;
            let eligibility = if approval.eligible_role == "any_authorized" {
                ApprovalEligibility::AnyAuthorizedMember
            } else {
                ApprovalEligibility::Role(parse_membership_role(&approval.eligible_role)?)
            };
            let respond = approval.status == "pending"
                && matches!(
                    authorize(
                        &subject,
                        Action::ApprovalRespond,
                        Resource::Approval {
                            scope: ProjectScope::approval(
                                conversation.project_id,
                                conversation.id,
                                agent_run_id,
                                approval.repository_id,
                                approval.id,
                            ),
                            requirements: ApprovalRequirements {
                                requesting_actor_id: approval.requesting_actor_id,
                                eligibility,
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

pub(crate) async fn enqueue_event(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    payload: OutboxEventPayload,
) -> Result<(), AppError> {
    let event = NewOutboxEvent::new(project_id, payload)
        .map_err(|_| AppError::Internal("construct Conversation outbox event"))?;
    outbox::enqueue(transaction, &event).await?;
    Ok(())
}

pub(crate) async fn finish_idempotent_command<T: Serialize>(
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

/// A thread name from the start of a message's first line, as MCP thread_start
/// suggests it. Message removal renames threads still carrying it.
pub(crate) fn suggested_thread_name(text: &str) -> String {
    let first_line = text.trim().lines().next().unwrap_or_default().trim();
    let short = first_line.chars().take(60).collect::<String>();
    if short.chars().count() < first_line.chars().count() {
        let trimmed = short
            .rsplit_once(char::is_whitespace)
            .map_or(short.as_str(), |(head, _)| head);
        format!("{trimmed}…")
    } else {
        short
    }
}

/// The name the web thread dialog suggests for a message (ThreadDialog.tsx),
/// after the server trims it. Message removal renames threads carrying it.
pub(crate) fn web_suggested_thread_name(text: &str) -> String {
    let first_line = text.trim().split('\n').next().unwrap_or_default();
    let short = first_line.chars().take(60).collect::<String>();
    let name = if short.chars().count() < first_line.chars().count() {
        // The dialog drops a trailing partial word: /\s+\S*$/.
        let without_word = short.trim_end_matches(|character: char| !character.is_whitespace());
        let head = if without_word.is_empty() {
            short.as_str()
        } else {
            without_word.trim_end()
        };
        format!("{head}…")
    } else {
        short
    };
    name.trim().to_owned()
}

pub(crate) fn normalize_conversation_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty()
        || name.chars().count() > MAX_CONVERSATION_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Conversation name"));
    }
    Ok(name.to_owned())
}

pub(crate) fn message_content_error(_error: MessageContentError) -> AppError {
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

/// Record a browser action on a conversation. Metadata holds IDs and counts,
/// never message text or names.
pub(crate) async fn insert_audit_event(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &'static str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    insert_audit_event_for(
        transaction,
        headers,
        principal,
        project_id,
        action,
        "conversation",
        target_id,
        metadata,
    )
    .await
}

/// `insert_audit_event` for something other than a conversation, such as a
/// message (`target_kind` "message"). Metadata holds IDs and counts, never
/// message text or names.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_audit_event_for(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &'static str,
    target_kind: &'static str,
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
            $5, $6, $7, 'succeeded', $8, $9
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
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
        MentionCursor, RunCapabilities, SidebarThreadRow, decode_mention_cursor,
        decode_timeline_cursor, encode_mention_cursor, encode_timeline_cursor,
        normalize_conversation_name, project_sidebar_threads, thread_attention,
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
    fn run_capabilities_use_the_public_camel_case_contract() {
        let value = serde_json::to_value(RunCapabilities {
            interrupt: false,
            retry: true,
            publish: true,
            create_pull_request: true,
            refresh_pull_request: true,
        })
        .expect("serialize run capabilities");

        assert_eq!(value["createPullRequest"], true);
        assert_eq!(value["refreshPullRequest"], true);
        assert!(value.get("create_pull_request").is_none());
        assert!(value.get("refresh_pull_request").is_none());
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
