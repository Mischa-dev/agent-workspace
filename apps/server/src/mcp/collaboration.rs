use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    conversations::{MessageCommandActor, MessageMcpCredential, append_message_command},
    error::AppError,
    external_events::{ExternalEvent, decode_cursor, encode_cursor},
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    policy::{Action, ActorKind},
};

const DEFAULT_READ_LIMIT: u16 = 50;
const MAX_READ_LIMIT: u16 = 100;
const MAX_SEARCH_CHARACTERS: usize = 120;
const MAX_CURSOR_BYTES: usize = 1_024;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationListArguments {
    pub project_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationTimelineArguments {
    pub conversation_id: String,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MentionableActorSearchArguments {
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationMessageSendArguments {
    pub conversation_id: String,
    pub content: MessageContent,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(super) enum CollaborationReadResult<T> {
    Found { value: T },
    NotFound { retry_category: String },
    PermissionDenied { retry_category: String },
    InvalidRequest { retry_category: String },
    TemporarilyUnavailable { retry_category: String },
}

impl<T> CollaborationReadResult<T> {
    pub(super) fn denied() -> Self {
        Self::PermissionDenied {
            retry_category: "reauthenticate_or_request_scope".to_owned(),
        }
    }

    fn not_found() -> Self {
        Self::NotFound {
            retry_category: "do_not_retry".to_owned(),
        }
    }

    fn invalid() -> Self {
        Self::InvalidRequest {
            retry_category: "correct_input".to_owned(),
        }
    }

    fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            retry_category: "retry_with_backoff".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationListProjection {
    conversations: Vec<ConversationProjection>,
    next_cursor: Option<String>,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
struct ConversationProjection {
    id: String,
    kind: String,
    parent_channel_id: Option<String>,
    name: String,
    version: i64,
    highest_sequence: i64,
    canonical_path: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationTimelineProjection {
    conversation: ConversationProjection,
    entries: Vec<TimelineEntryProjection>,
    next_cursor: Option<String>,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
struct TimelineEntryProjection {
    id: String,
    sequence: i64,
    kind: String,
    created_at: String,
    message: Option<serde_json::Value>,
    worker_event: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MentionableActorProjection {
    actors: Vec<MentionableActor>,
    next_cursor: Option<String>,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
struct MentionableActor {
    id: String,
    kind: String,
    display_name: String,
    handle: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum ConversationMessageSendResult {
    Applied {
        result: serde_json::Value,
        idempotent_replay: bool,
    },
    NotFound {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    Conflict {
        retry_category: String,
    },
    InvalidRequest {
        field: String,
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

impl ConversationMessageSendResult {
    pub(super) fn denied() -> Self {
        Self::PermissionDenied {
            retry_category: "reauthenticate_or_request_scope".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct BoundCursor {
    version: u8,
    kind: String,
    connection_id: Uuid,
    project_id: Uuid,
    credential_id: Uuid,
    position: String,
}

#[derive(Debug, FromRow)]
struct ConnectionAuthorization {
    signing_key: Vec<u8>,
    membership_role: String,
}

#[derive(Clone, Copy, Debug)]
enum ReadFailure {
    Authorization,
    Invalid,
    NotFound,
    Database(&'static str),
}

pub(super) async fn conversation_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationListArguments,
) -> CollaborationReadResult<ConversationListProjection> {
    let result = conversation_list_inner(state, principal, arguments).await;
    map_read_result(result, "conversation_list")
}

async fn conversation_list_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationListArguments,
) -> Result<ConversationListProjection, ReadFailure> {
    if arguments
        .project_id
        .as_deref()
        .is_some_and(|value| value.parse::<Uuid>() != Ok(principal.project_id))
    {
        return Err(ReadFailure::NotFound);
    }
    let limit = bounded_limit(arguments.limit)?;
    let (mut transaction, authorization) = begin_authorized_read(state, principal, "read").await?;
    let after_id = arguments
        .cursor
        .as_deref()
        .map(|cursor| {
            decode_bound_cursor(
                &authorization.signing_key,
                principal,
                "conversations",
                cursor,
            )
        })
        .transpose()?
        .map(|value| value.parse::<Uuid>().map_err(|_| ReadFailure::Invalid))
        .transpose()?;
    let mut rows = sqlx::query_as::<_, ConversationProjection>(
        r#"
        SELECT conversation.id::text AS id,
               conversation.kind,
               conversation.parent_channel_id::text AS parent_channel_id,
               conversation.name,
               conversation.version,
               conversation.next_entry_sequence - 1 AS highest_sequence,
               '/projects/' || conversation.project_id::text || '/' ||
                 CASE WHEN conversation.kind = 'thread' THEN 'threads/' ELSE 'channels/' END ||
                 conversation.id::text AS canonical_path
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.project_id = $1
          AND conversation.archived_at IS NULL
          AND (conversation.kind = 'channel' OR parent.archived_at IS NULL)
          AND ($2::uuid IS NULL OR conversation.id > $2)
        ORDER BY conversation.id
        LIMIT $3
        "#,
    )
    .bind(principal.project_id)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("conversation_list_fetch"))?;
    let truncated = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if truncated {
        rows.pop();
    }
    let next_cursor = if truncated {
        rows.last()
            .map(|row| {
                encode_bound_cursor(
                    &authorization.signing_key,
                    principal,
                    "conversations",
                    &row.id,
                )
            })
            .transpose()?
    } else {
        None
    };
    transaction
        .commit()
        .await
        .map_err(|_| ReadFailure::Database("conversation_list_commit"))?;
    Ok(ConversationListProjection {
        conversations: rows,
        next_cursor,
        truncated,
    })
}

pub(super) async fn conversation_timeline_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationTimelineArguments,
) -> CollaborationReadResult<ConversationTimelineProjection> {
    let result = conversation_timeline_inner(state, principal, arguments).await;
    map_read_result(result, "conversation_timeline_get")
}

async fn conversation_timeline_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationTimelineArguments,
) -> Result<ConversationTimelineProjection, ReadFailure> {
    let conversation_id = arguments
        .conversation_id
        .parse::<Uuid>()
        .map_err(|_| ReadFailure::Invalid)?;
    let limit = bounded_limit(arguments.limit)?;
    let (mut transaction, authorization) = begin_authorized_read(state, principal, "read").await?;
    let conversation = sqlx::query_as::<_, ConversationProjection>(
        r#"
        SELECT conversation.id::text AS id,
               conversation.kind,
               conversation.parent_channel_id::text AS parent_channel_id,
               conversation.name,
               conversation.version,
               conversation.next_entry_sequence - 1 AS highest_sequence,
               '/projects/' || conversation.project_id::text || '/' ||
                 CASE WHEN conversation.kind = 'thread' THEN 'threads/' ELSE 'channels/' END ||
                 conversation.id::text AS canonical_path
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1 AND conversation.project_id = $2
          AND conversation.archived_at IS NULL
          AND (conversation.kind = 'channel' OR parent.archived_at IS NULL)
        "#,
    )
    .bind(conversation_id)
    .bind(principal.project_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("conversation_timeline_access"))?
    .ok_or(ReadFailure::NotFound)?;
    let before_sequence = arguments
        .cursor
        .as_deref()
        .map(|cursor| {
            decode_bound_cursor(
                &authorization.signing_key,
                principal,
                &format!("timeline:{conversation_id}"),
                cursor,
            )
        })
        .transpose()?
        .map(|value| value.parse::<i64>().map_err(|_| ReadFailure::Invalid))
        .transpose()?
        .unwrap_or(conversation.highest_sequence + 1);
    if before_sequence < 1 || before_sequence > conversation.highest_sequence + 1 {
        return Err(ReadFailure::Invalid);
    }
    let mut rows = sqlx::query_as::<_, TimelineEntryProjection>(
        r#"
        SELECT entry.id::text AS id, entry.sequence, entry.kind,
               entry.created_at::text AS created_at,
               CASE WHEN message.id IS NULL THEN NULL ELSE jsonb_build_object(
                   'id', message.id,
                   'author', jsonb_build_object(
                       'id', author.id,
                       'kind', author.kind,
                       'displayName', author_membership.display_name,
                       'handle', author_membership.handle
                   ),
                   'content', message.content,
                   'plainText', message.plain_text,
                   'mentionedActors', COALESCE(mentions.actors, '[]'::jsonb),
                   'referencedWorkItems', COALESCE(work_items.items, '[]'::jsonb),
                   'createdAt', message.created_at::text
               ) END AS message,
               CASE WHEN event.id IS NULL THEN NULL ELSE jsonb_build_object(
                   'id', event.id,
                   'agentRunId', event.agent_run_id,
                   'kind', event.kind,
                   'summary', event.summary,
                   'createdAt', event.created_at::text
               ) END AS worker_event
        FROM conversation_entries entry
        LEFT JOIN messages message ON message.id = entry.message_id
        LEFT JOIN actors author ON author.id = message.author_actor_id
        LEFT JOIN project_memberships author_membership
          ON author_membership.project_id = $2 AND author_membership.actor_id = author.id
        LEFT JOIN LATERAL (
            SELECT jsonb_agg(jsonb_build_object(
                'id', actor.id, 'kind', actor.kind,
                'displayName', membership.display_name, 'handle', membership.handle
            ) ORDER BY lower(membership.display_name), actor.id) AS actors
            FROM message_mentions mention
            JOIN actors actor ON actor.id = mention.actor_id
            JOIN project_memberships membership
              ON membership.project_id = mention.project_id AND membership.actor_id = mention.actor_id
            WHERE mention.message_id = message.id
        ) mentions ON true
        LEFT JOIN LATERAL (
            SELECT jsonb_agg(jsonb_build_object(
                'id', item.id,
                'key', project.project_key || '-' || item.number::text,
                'title', item.title,
                'statusCategory', status.semantic_category,
                'archivedAt', item.archived_at::text,
                'canonicalPath', '/projects/' || item.project_id::text || '/work-items/' || item.id::text
            ) ORDER BY item.number, item.id) AS items
            FROM message_work_item_references reference
            JOIN work_items item ON item.id = reference.work_item_id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            WHERE reference.message_id = message.id
        ) work_items ON true
        LEFT JOIN agent_run_events event ON event.id = entry.agent_run_event_id
        WHERE entry.conversation_id = $1 AND entry.sequence < $3
        ORDER BY entry.sequence DESC
        LIMIT $4
        "#,
    )
    .bind(conversation_id)
    .bind(principal.project_id)
    .bind(before_sequence)
    .bind(limit + 1)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("conversation_timeline_fetch"))?;
    let truncated = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if truncated {
        rows.pop();
    }
    rows.reverse();
    let next_cursor = if truncated {
        rows.first()
            .map(|row| {
                encode_bound_cursor(
                    &authorization.signing_key,
                    principal,
                    &format!("timeline:{conversation_id}"),
                    &row.sequence.to_string(),
                )
            })
            .transpose()?
    } else {
        None
    };
    transaction
        .commit()
        .await
        .map_err(|_| ReadFailure::Database("conversation_timeline_commit"))?;
    Ok(ConversationTimelineProjection {
        conversation,
        entries: rows,
        next_cursor,
        truncated,
    })
}

pub(super) async fn mentionable_actor_search(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MentionableActorSearchArguments,
) -> CollaborationReadResult<MentionableActorProjection> {
    let result = mentionable_actor_search_inner(state, principal, arguments).await;
    map_read_result(result, "mentionable_actor_search")
}

async fn mentionable_actor_search_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MentionableActorSearchArguments,
) -> Result<MentionableActorProjection, ReadFailure> {
    let query = arguments.query.unwrap_or_default().trim().to_owned();
    if query.chars().count() > MAX_SEARCH_CHARACTERS {
        return Err(ReadFailure::Invalid);
    }
    let limit = bounded_limit(arguments.limit)?.min(50);
    let (mut transaction, authorization) = begin_authorized_read(state, principal, "read").await?;
    let after_id = arguments
        .cursor
        .as_deref()
        .map(|cursor| {
            decode_bound_cursor(
                &authorization.signing_key,
                principal,
                "mentionable_actors",
                cursor,
            )
        })
        .transpose()?
        .map(|value| value.parse::<Uuid>().map_err(|_| ReadFailure::Invalid))
        .transpose()?;
    let mut rows = sqlx::query_as::<_, MentionableActor>(
        r#"
        SELECT actor.id::text AS id, actor.kind,
               membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
          AND ($2 = '' OR membership.display_name ILIKE '%' || $2 || '%'
                       OR membership.handle ILIKE '%' || $2 || '%')
          AND ($3::uuid IS NULL OR actor.id > $3)
        ORDER BY actor.id
        LIMIT $4
        "#,
    )
    .bind(principal.project_id)
    .bind(query)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("mentionable_actor_search_fetch"))?;
    let truncated = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if truncated {
        rows.pop();
    }
    let next_cursor = if truncated {
        rows.last()
            .map(|row| {
                encode_bound_cursor(
                    &authorization.signing_key,
                    principal,
                    "mentionable_actors",
                    &row.id,
                )
            })
            .transpose()?
    } else {
        None
    };
    transaction
        .commit()
        .await
        .map_err(|_| ReadFailure::Database("mentionable_actor_search_commit"))?;
    Ok(MentionableActorProjection {
        actors: rows,
        next_cursor,
        truncated,
    })
}

pub(super) async fn conversation_message_send(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationMessageSendArguments,
) -> ConversationMessageSendResult {
    if principal.actor_kind != ActorKind::ExternalAgent
        || principal.external_agent_connection_id.is_none()
        || !principal.can_write()
    {
        return ConversationMessageSendResult::denied();
    }
    let conversation_id = match arguments.conversation_id.parse::<Uuid>() {
        Ok(value) => value,
        Err(_) => return invalid_send("conversationId"),
    };
    let idempotency_key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(value) => value,
        Err(_) => return invalid_send("clientRequestId"),
    };
    let Some(connection_id) = principal.external_agent_connection_id else {
        return ConversationMessageSendResult::denied();
    };
    let content = match serde_json::to_value(arguments.content) {
        Ok(content) => content,
        Err(_) => return unavailable_send(),
    };
    match append_message_command(
        state,
        MessageCommandActor {
            actor_id: principal.actor_id,
            actor_kind: "external_agent".to_owned(),
            mcp_credential: Some(MessageMcpCredential {
                credential_id: principal.credential_id,
                connection_id,
            }),
        },
        conversation_id,
        None,
        idempotency_key,
        content,
    )
    .await
    {
        Ok(outcome) => match serde_json::to_value(outcome.body) {
            Ok(result) => ConversationMessageSendResult::Applied {
                result,
                idempotent_replay: outcome.idempotent_replay,
            },
            Err(_) => unavailable_send(),
        },
        Err(AppError::NotFound(_)) => ConversationMessageSendResult::NotFound {
            retry_category: "do_not_retry".to_owned(),
        },
        Err(
            AppError::Forbidden | AppError::AuthenticationRequired | AppError::InvalidCredentials,
        ) => ConversationMessageSendResult::denied(),
        Err(AppError::Conflict(_, _)) => ConversationMessageSendResult::Conflict {
            retry_category: "use_new_client_request_id_or_same_payload".to_owned(),
        },
        Err(AppError::Invalid(_) | AppError::Validation(_) | AppError::PayloadTooLarge) => {
            invalid_send("content")
        }
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "conversation_message_send failed"
            );
            unavailable_send()
        }
    }
}

async fn begin_authorized_read<'a>(
    state: &'a AppState,
    principal: &McpPrincipal,
    required_scope: &str,
) -> Result<(Transaction<'a, Postgres>, ConnectionAuthorization), ReadFailure> {
    let connection_id = principal
        .external_agent_connection_id
        .ok_or(ReadFailure::Authorization)?;
    if principal.actor_kind != ActorKind::ExternalAgent || !principal.has_scope(required_scope) {
        return Err(ReadFailure::Authorization);
    }
    let mut transaction = state
        .pool
        .begin()
        .await
        .map_err(|_| ReadFailure::Database("begin"))?;
    let authorization = sqlx::query_as::<_, ConnectionAuthorization>(
        r#"
        SELECT credential.token_digest AS signing_key,
               membership.role AS membership_role
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = credential.actor_id
        JOIN projects project ON project.id = credential.project_id
        JOIN project_memberships membership
          ON membership.project_id = credential.project_id
         AND membership.actor_id = credential.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = credential.actor_id
        WHERE credential.id = $1
          AND credential.project_id = $2
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active' AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY[$5]::text[]
          AND connection.status = 'active'
          AND connection.project_id = credential.project_id
          AND connection.actor_id = credential.actor_id
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent' AND actor.status = 'active'
          AND project.archived_at IS NULL
          AND membership.enabled AND organization_membership.enabled
        FOR SHARE OF credential, connection, actor, project, membership,
                     organization_membership
        "#,
    )
    .bind(principal.credential_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .bind(connection_id)
    .bind(required_scope)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("authorize"))?
    .ok_or(ReadFailure::Authorization)?;
    if !super::mcp_policy_allows(
        principal.actor_id,
        principal.actor_kind,
        principal.project_id,
        &authorization.membership_role,
        Action::McpRead,
    ) {
        return Err(ReadFailure::Authorization);
    }
    Ok((transaction, authorization))
}

fn bounded_limit(limit: Option<u16>) -> Result<i64, ReadFailure> {
    let value = limit.unwrap_or(DEFAULT_READ_LIMIT);
    if !(1..=MAX_READ_LIMIT).contains(&value) {
        return Err(ReadFailure::Invalid);
    }
    Ok(i64::from(value))
}

fn encode_bound_cursor(
    signing_key: &[u8],
    principal: &McpPrincipal,
    kind: &str,
    position: &str,
) -> Result<String, ReadFailure> {
    let connection_id = principal
        .external_agent_connection_id
        .ok_or(ReadFailure::Authorization)?;
    let payload = serde_json::to_vec(&BoundCursor {
        version: 1,
        kind: kind.to_owned(),
        connection_id,
        project_id: principal.project_id,
        credential_id: principal.credential_id,
        position: position.to_owned(),
    })
    .map_err(|_| ReadFailure::Database("cursor_encode"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key)
        .map_err(|_| ReadFailure::Database("cursor_key"))?;
    mac.update(&payload);
    let signature = mac.finalize().into_bytes();
    let mut envelope = payload;
    envelope.extend_from_slice(&signature);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

fn decode_bound_cursor(
    signing_key: &[u8],
    principal: &McpPrincipal,
    expected_kind: &str,
    cursor: &str,
) -> Result<String, ReadFailure> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES {
        return Err(ReadFailure::Invalid);
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ReadFailure::Invalid)?;
    let payload_length = envelope.len().checked_sub(32).ok_or(ReadFailure::Invalid)?;
    let (payload, signature) = envelope.split_at(payload_length);
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key)
        .map_err(|_| ReadFailure::Database("cursor_key"))?;
    mac.update(payload);
    mac.verify_slice(signature)
        .map_err(|_| ReadFailure::Invalid)?;
    let decoded: BoundCursor = serde_json::from_slice(payload).map_err(|_| ReadFailure::Invalid)?;
    if decoded.version != 1
        || decoded.kind != expected_kind
        || Some(decoded.connection_id) != principal.external_agent_connection_id
        || decoded.project_id != principal.project_id
        || decoded.credential_id != principal.credential_id
        || decoded.position.is_empty()
    {
        return Err(ReadFailure::Invalid);
    }
    Ok(decoded.position)
}

fn map_read_result<T>(
    result: Result<T, ReadFailure>,
    operation: &'static str,
) -> CollaborationReadResult<T> {
    match result {
        Ok(value) => CollaborationReadResult::Found { value },
        Err(ReadFailure::Authorization) => CollaborationReadResult::denied(),
        Err(ReadFailure::Invalid) => CollaborationReadResult::invalid(),
        Err(ReadFailure::NotFound) => CollaborationReadResult::not_found(),
        Err(ReadFailure::Database(category)) => {
            tracing::error!(operation, category, "MCP collaboration read failed");
            CollaborationReadResult::unavailable()
        }
    }
}

fn invalid_send(field: &str) -> ConversationMessageSendResult {
    ConversationMessageSendResult::InvalidRequest {
        field: field.to_owned(),
        retry_category: "correct_input".to_owned(),
    }
}

fn unavailable_send() -> ConversationMessageSendResult {
    ConversationMessageSendResult::TemporarilyUnavailable {
        retry_category: "retry_with_backoff".to_owned(),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExternalEventPollArguments {
    pub after_cursor: Option<String>,
    pub limit: Option<u16>,
    pub wait_seconds: Option<u8>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum ExternalEventPollResult {
    Events {
        events: Vec<ExternalEventProjection>,
        next_cursor: String,
        has_more: bool,
    },
    CursorExpired {
        retry_category: String,
        resync_path: String,
    },
    Busy {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    InvalidRequest {
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExternalEventProjection {
    id: String,
    sequence: i64,
    kind: String,
    summary: String,
    resource_kind: String,
    resource_id: String,
    canonical_path: String,
    created_at: String,
    expires_at: String,
}

pub(super) async fn poll_external_events(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ExternalEventPollArguments,
) -> ExternalEventPollResult {
    let Some(connection_id) = principal.external_agent_connection_id else {
        return denied();
    };
    if !principal.has_scope("events") {
        return denied();
    }
    let limit = arguments.limit.unwrap_or(50);
    let wait_seconds = arguments.wait_seconds.unwrap_or(0);
    if !(1..=100).contains(&limit) || wait_seconds > 30 {
        return ExternalEventPollResult::InvalidRequest {
            retry_category: "do_not_retry_without_change".to_owned(),
        };
    }

    match poll(
        state,
        principal,
        connection_id,
        arguments.after_cursor.as_deref(),
        i64::from(limit),
        wait_seconds,
    )
    .await
    {
        Ok(result) => result,
        Err(PollFailure::InvalidCursor) => ExternalEventPollResult::InvalidRequest {
            retry_category: "do_not_retry_without_change".to_owned(),
        },
        Err(PollFailure::Authorization) => denied(),
        Err(PollFailure::Database(category)) => {
            tracing::error!(category, "external event poll failed");
            ExternalEventPollResult::TemporarilyUnavailable {
                retry_category: "retry_with_backoff".to_owned(),
            }
        }
    }
}

async fn poll(
    state: &AppState,
    principal: &McpPrincipal,
    connection_id: Uuid,
    cursor: Option<&str>,
    limit: i64,
    wait_seconds: u8,
) -> Result<ExternalEventPollResult, PollFailure> {
    let mut transaction = state
        .pool
        .begin()
        .await
        .map_err(|_| PollFailure::Database("begin"))?;
    let signing_key = revalidate_connection(&mut transaction, principal, connection_id)
        .await?
        .ok_or(PollFailure::Authorization)?;
    let acquired =
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("m9-external-poll:{connection_id}"))
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| PollFailure::Database("poll_lock"))?;
    if !acquired {
        return Ok(ExternalEventPollResult::Busy {
            retry_category: "retry_with_backoff".to_owned(),
        });
    }
    let after_sequence = match cursor {
        Some(cursor) => decode_cursor(
            &signing_key,
            cursor,
            connection_id,
            principal.project_id,
            principal.credential_id,
        )
        .map_err(|_| PollFailure::InvalidCursor)?,
        None => 0,
    };
    if cursor_is_expired(&mut transaction, connection_id, after_sequence).await? {
        return Ok(ExternalEventPollResult::CursorExpired {
            retry_category: "resynchronize".to_owned(),
            resync_path: format!("/projects/{}", principal.project_id),
        });
    }

    let deadline = Instant::now() + Duration::from_secs(u64::from(wait_seconds));
    loop {
        let mut rows = sqlx::query_as::<_, ExternalEvent>(
            r#"
            SELECT id, sequence, kind, summary, resource_kind, resource_id,
                   canonical_path, created_at::text AS created_at,
                   expires_at::text AS expires_at
            FROM external_agent_events
            WHERE connection_id = $1 AND sequence > $2 AND expires_at > now()
            ORDER BY sequence
            LIMIT $3
            "#,
        )
        .bind(connection_id)
        .bind(after_sequence)
        .bind(limit + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| PollFailure::Database("fetch"))?;
        if !rows.is_empty() || Instant::now() >= deadline {
            if revalidate_connection(&mut transaction, principal, connection_id)
                .await?
                .is_none()
            {
                return Err(PollFailure::Authorization);
            }
            let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
            if has_more {
                rows.pop();
            }
            let next_sequence = rows.last().map_or(after_sequence, |event| event.sequence);
            let next_cursor = encode_cursor(
                &signing_key,
                connection_id,
                principal.project_id,
                principal.credential_id,
                next_sequence,
            )
            .map_err(|_| PollFailure::Database("encode_cursor"))?;
            transaction
                .commit()
                .await
                .map_err(|_| PollFailure::Database("commit"))?;
            return Ok(ExternalEventPollResult::Events {
                events: rows
                    .into_iter()
                    .map(ExternalEventProjection::from)
                    .collect(),
                next_cursor,
                has_more,
            });
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn revalidate_connection(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    connection_id: Uuid,
) -> Result<Option<Vec<u8>>, PollFailure> {
    sqlx::query_scalar::<_, Vec<u8>>(
        r#"
        SELECT credential.token_digest
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = connection.actor_id
        JOIN projects project ON project.id = connection.project_id
        JOIN project_memberships membership
          ON membership.project_id = connection.project_id
         AND membership.actor_id = connection.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = connection.actor_id
        WHERE credential.id = $1
          AND credential.project_id = $2
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active'
          AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY['events']::text[]
          AND connection.status = 'active'
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent'
          AND actor.status = 'active'
          AND project.archived_at IS NULL
          AND membership.enabled
          AND organization_membership.enabled
        "#,
    )
    .bind(principal.credential_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .bind(connection_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| PollFailure::Database("revalidate"))
}

async fn cursor_is_expired(
    transaction: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    after_sequence: i64,
) -> Result<bool, PollFailure> {
    let (next_sequence, minimum_available) = sqlx::query_as::<_, (i64, Option<i64>)>(
        r#"
        SELECT counter.next_sequence,
               (SELECT min(sequence) FROM external_agent_events
                WHERE connection_id = $1 AND expires_at > now())
        FROM external_agent_event_counters counter
        WHERE counter.connection_id = $1
        "#,
    )
    .bind(connection_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| PollFailure::Database("cursor_retention"))?
    .unwrap_or((1, None));
    if after_sequence >= next_sequence {
        return Ok(true);
    }
    Ok(match minimum_available {
        Some(minimum) => after_sequence + 1 < minimum,
        None => next_sequence > after_sequence + 1,
    })
}

#[derive(Clone, Copy, Debug)]
enum PollFailure {
    Authorization,
    InvalidCursor,
    Database(&'static str),
}

fn denied() -> ExternalEventPollResult {
    ExternalEventPollResult::PermissionDenied {
        retry_category: "reauthenticate_or_request_scope".to_owned(),
    }
}

impl From<ExternalEvent> for ExternalEventProjection {
    fn from(event: ExternalEvent) -> Self {
        Self {
            id: event.id.to_string(),
            sequence: event.sequence,
            kind: event.kind,
            summary: event.summary,
            resource_kind: event.resource_kind,
            resource_id: event.resource_id.to_string(),
            canonical_path: event.canonical_path,
            created_at: event.created_at,
            expires_at: event.expires_at,
        }
    }
}
