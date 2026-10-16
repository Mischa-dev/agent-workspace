//! Direct messages and group conversations.
//!
//! DMs and groups live in one hidden "direct" Project per Organization
//! (migration 0037). Each conversation is visible only to its members through
//! `conversation_visible_to`, which the shared conversation access checks call,
//! so timelines, sending, read state, realtime, and MCP reads need no
//! DM-specific code.

use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ActorView, ConversationCreateResponse, ConversationView, ReadStateView, enqueue_event,
        insert_audit_event, lock_conversation_for, normalize_conversation_name, parse_actor_kind,
    },
    error::{ApiJson, ApiPath, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    outbox::{
        ConversationCreatedPayload, ConversationMemberRemovedPayload, ConversationUpdatedPayload,
        OutboxConversationKind, OutboxEventPayload,
    },
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
    projects::membership_handle,
    replies::{self, Withdraw},
};

const DIRECT_PROJECT_KEY: &str = "DMSPACE";
const DIRECT_PROJECT_NAME: &str = "Direct messages";
const DM_NAME: &str = "Direct message";
pub(crate) const MAX_GROUP_MEMBERS: usize = 20;
const MAX_LISTED_CONVERSATIONS: i64 = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/direct", get(direct_overview))
        .route("/api/v1/direct/dms", post(open_dm))
        .route("/api/v1/direct/groups", post(create_group))
        .route("/api/v1/conversations/{id}/members", post(add_member))
        .route(
            "/api/v1/conversations/{id}/members/{actor_id}/remove",
            post(remove_member),
        )
}

struct DirectSpace {
    project_id: Uuid,
    role: MembershipRole,
    enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenDmRequest {
    actor_id: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateGroupRequest {
    name: Option<String>,
    member_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddMemberRequest {
    actor_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectOverview {
    project_id: Uuid,
    conversations: Vec<DirectConversationItem>,
    people: Vec<ActorView>,
    capabilities: DirectCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectConversationItem {
    #[serde(flatten)]
    conversation: ConversationView,
    members: Vec<ActorView>,
    highest_sequence: i64,
    read_state: ReadStateView,
    unread: bool,
    last_activity_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectCapabilities {
    start_conversation: bool,
    manage_agents: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MembersResponse {
    conversation_id: Uuid,
    version: i64,
    members: Vec<ActorView>,
}

#[derive(Debug, FromRow)]
struct DirectConversationRow {
    id: Uuid,
    project_id: Uuid,
    kind: String,
    name: String,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    read_version: i64,
    last_activity_at: String,
    members: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct PersonRow {
    id: Uuid,
    kind: String,
    display_name: String,
    handle: String,
}

impl From<PersonRow> for ActorView {
    fn from(row: PersonRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            display_name: row.display_name,
            handle: row.handle,
            avatar_url: None,
        }
    }
}

async fn direct_overview(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<DirectOverview>, AppError> {
    // The first visit creates the space, so this read runs in a transaction.
    let mut transaction = state.pool.begin().await?;
    let space = ensure_direct_space(&mut transaction, &Person::from(&principal)).await?;
    require(&principal, &space, Action::ProjectView)?;
    let rows = sqlx::query_as::<_, DirectConversationRow>(
        r#"
        SELECT
            conversation.id,
            conversation.project_id,
            conversation.kind,
            conversation.name,
            conversation.version,
            conversation.next_entry_sequence - 1 AS highest_sequence,
            COALESCE(read_state.highest_seen_sequence, 0) AS highest_seen_sequence,
            COALESCE(read_state.version, 0) AS read_version,
            COALESCE(last_entry.created_at, conversation.created_at)::text AS last_activity_at,
            COALESCE(members.actors, '[]'::jsonb) AS members
        FROM conversation_members me
        JOIN conversations conversation ON conversation.id = me.conversation_id
        LEFT JOIN conversation_read_states read_state
          ON read_state.project_id = conversation.project_id
         AND read_state.conversation_id = conversation.id
         AND read_state.actor_id = me.actor_id
        LEFT JOIN LATERAL (
            SELECT entry.created_at
            FROM conversation_entries entry
            WHERE entry.conversation_id = conversation.id
            ORDER BY entry.sequence DESC
            LIMIT 1
        ) last_entry ON true
        LEFT JOIN LATERAL (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', actor.id,
                    'kind', actor.kind,
                    'displayName', membership.display_name,
                    'handle', membership.handle,
                    'avatarUrl', NULL
                )
                ORDER BY lower(membership.display_name), actor.id
            ) AS actors
            FROM conversation_members member
            JOIN actors actor ON actor.id = member.actor_id
            JOIN project_memberships membership
              ON membership.project_id = member.project_id
             AND membership.actor_id = member.actor_id
            WHERE member.conversation_id = conversation.id
        ) members ON true
        WHERE me.project_id = $1
          AND me.actor_id = $2
          AND conversation.kind IN ('dm', 'group')
          AND conversation.archived_at IS NULL
        ORDER BY COALESCE(last_entry.created_at, conversation.created_at) DESC, conversation.id
        LIMIT $3
        "#,
    )
    .bind(space.project_id)
    .bind(principal.actor_id)
    .bind(MAX_LISTED_CONVERSATIONS)
    .fetch_all(&mut *transaction)
    .await?;
    let conversations = rows
        .into_iter()
        .map(|row| {
            let read_state = ReadStateView {
                highest_seen_sequence: row.highest_seen_sequence,
                version: row.read_version,
            };
            Ok(DirectConversationItem {
                conversation: ConversationView {
                    id: row.id,
                    project_id: row.project_id,
                    kind: row.kind,
                    parent_channel_id: None,
                    origin_message_id: None,
                    name: row.name,
                    archived_at: None,
                    version: row.version,
                    thread_status: None,
                    thread_outcome: None,
                    parent_kind: None,
                },
                members: serde_json::from_value(row.members)
                    .map_err(|_| AppError::Internal("direct conversation members"))?,
                highest_sequence: row.highest_sequence,
                unread: row.highest_sequence > read_state.highest_seen_sequence,
                read_state,
                last_activity_at: row.last_activity_at,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let people = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
          AND actor.id <> $2
        ORDER BY (actor.kind = 'external_agent') DESC, lower(membership.display_name), actor.id
        "#,
    )
    .bind(space.project_id)
    .bind(principal.actor_id)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect();
    let capabilities = DirectCapabilities {
        start_conversation: allows(&principal, &space, Action::ChannelCreate)?,
        manage_agents: allows(&principal, &space, Action::ExternalAgentManage)?,
    };
    transaction.commit().await?;
    Ok(Json(DirectOverview {
        project_id: space.project_id,
        conversations,
        people,
        capabilities,
    }))
}

async fn open_dm(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<OpenDmRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.actor_id == principal.actor_id {
        return Err(AppError::Validation("DM recipient"));
    }
    let mut transaction = state.pool.begin().await?;
    let space = ensure_direct_space(&mut transaction, &Person::from(&principal)).await?;
    require(&principal, &space, Action::ChannelCreate)?;
    let (conversation_id, created) = open_dm_in(
        &mut transaction,
        space.project_id,
        principal.actor_id,
        request.actor_id,
    )
    .await?;
    if created {
        announce_created(
            &mut transaction,
            &headers,
            &principal,
            space.project_id,
            conversation_id,
            OutboxConversationKind::Dm,
            "conversation.dm_created",
        )
        .await?;
    }
    let body = conversation_response(&mut transaction, &principal, conversation_id).await?;
    transaction.commit().await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(body)).into_response())
}

async fn create_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CreateGroupRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let others = request
        .member_ids
        .iter()
        .copied()
        .filter(|id| *id != principal.actor_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if others.is_empty() || others.len() >= MAX_GROUP_MEMBERS {
        return Err(AppError::Validation("group members"));
    }
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(normalize_conversation_name)
        .transpose()?;
    let request_hash = RequestHash::for_value(&CreateGroupRequest {
        name: name.clone(),
        member_ids: others.clone(),
    })?;

    let mut transaction = state.pool.begin().await?;
    let space = ensure_direct_space(&mut transaction, &Person::from(&principal)).await?;
    require(&principal, &space, Action::ChannelCreate)?;
    let scope = idempotency::resource_bound_scope("project", space.project_id, "group_create")?;
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
            return Ok((replay.status(), Json(body)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let conversation_id = create_group_in(
        &mut transaction,
        space.project_id,
        principal.actor_id,
        &others,
        name,
    )
    .await?;
    announce_created(
        &mut transaction,
        &headers,
        &principal,
        space.project_id,
        conversation_id,
        OutboxConversationKind::Group,
        "conversation.group_created",
    )
    .await?;
    let body = conversation_response(&mut transaction, &principal, conversation_id).await?;
    idempotency::complete(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn add_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<AddMemberRequest>,
) -> Result<Json<MembersResponse>, AppError> {
    change_members_from_web(
        &state,
        &headers,
        &principal,
        conversation_id,
        &[request.actor_id],
        &[],
    )
    .await
}

async fn remove_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, actor_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<MembersResponse>, AppError> {
    change_members_from_web(
        &state,
        &headers,
        &principal,
        conversation_id,
        &[],
        &[actor_id],
    )
    .await
}

async fn change_members_from_web(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    add: &[Uuid],
    remove: &[Uuid],
) -> Result<Json<MembersResponse>, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let mut transaction = state.pool.begin().await?;
    let change = change_group_members(
        &mut transaction,
        GroupEditor {
            actor_id: principal.actor_id,
            actor_kind: parse_actor_kind(&principal.actor_kind)?,
        },
        conversation_id,
        add,
        remove,
        None,
    )
    .await?;
    for (members, action) in [
        (&change.added, "conversation.member_added"),
        (&change.removed, "conversation.member_removed"),
    ] {
        for member in members {
            insert_audit_event(
                &mut transaction,
                headers,
                principal,
                change.project_id,
                action,
                conversation_id,
                serde_json::json!({ "actor_id": member.id }),
            )
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(Json(MembersResponse {
        conversation_id,
        version: change.version,
        members: change.members,
    }))
}

/// Who changes a group's members.
pub(crate) struct GroupEditor {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
}

/// What `change_group_members` did. `added` and `removed` hold only the
/// actors whose membership actually changed; both empty means nothing did.
pub(crate) struct GroupMembersChange {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub name: String,
    pub version: i64,
    /// Everyone in the group after the change.
    pub members: Vec<ActorView>,
    pub added: Vec<ActorView>,
    pub removed: Vec<ActorView>,
}

/// Add and remove people and agents in a group. Shared by the browser API and
/// MCP; callers record their own audit rows.
///
/// - Visibility goes through `lock_conversation_for` (`conversation_visible_to`),
///   which also locks the group row, so concurrent changes are serialized and
///   the cap is checked under that lock. Someone who can't see the group gets
///   `NotFound("conversation")`.
/// - People can add and remove anyone. Anyone else (agents) can only remove
///   themselves: whoever is added reads the whole history, so handing out
///   that access needs a person (`Forbidden`).
/// - Leaving needs only to see the group; changing others needs
///   `ChannelCreate` in the Messages space.
/// - A change that changes nothing bumps no version and sends no events.
/// - New members' read marks start at the newest entry of the group and of
///   each thread under it, so its history isn't flagged as unread.
/// - Removed agents' open reply requests there are withdrawn, and each removed
///   actor gets a `conversation.member.removed` event of their own.
/// - There's no minimum: the last member can leave. An empty group is
///   invisible to everyone, and nothing is deleted.
pub(crate) async fn change_group_members(
    transaction: &mut Transaction<'_, Postgres>,
    editor: GroupEditor,
    conversation_id: Uuid,
    add: &[Uuid],
    remove: &[Uuid],
    expected_version: Option<i64>,
) -> Result<GroupMembersChange, AppError> {
    let add = add.iter().copied().collect::<BTreeSet<_>>();
    let remove = remove.iter().copied().collect::<BTreeSet<_>>();
    if (add.is_empty() && remove.is_empty()) || !add.is_disjoint(&remove) {
        return Err(AppError::Validation("group members"));
    }
    let leaving_only = add.is_empty() && remove.iter().all(|id| *id == editor.actor_id);
    let conversation = lock_conversation_for(
        transaction,
        editor.actor_id,
        editor.actor_kind,
        conversation_id,
        if leaving_only {
            Action::ProjectView
        } else {
            Action::ChannelCreate
        },
        false,
    )
    .await?;
    if conversation.kind != "group" {
        return Err(AppError::Conflict(
            "conversation",
            "only groups can change members",
        ));
    }
    if editor.actor_kind != ActorKind::Human && !leaving_only {
        return Err(AppError::Forbidden);
    }
    if let Some(expected_version) = expected_version
        && expected_version != conversation.version
    {
        return Err(AppError::VersionConflict {
            resource: "conversation",
            current_version: conversation.version,
        });
    }
    let project_id = conversation.project_id;
    let current = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM conversation_members WHERE conversation_id = $1",
    )
    .bind(conversation_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let to_add = add.difference(&current).copied().collect::<Vec<_>>();
    let to_remove = remove.intersection(&current).copied().collect::<Vec<_>>();
    if to_add.is_empty() && to_remove.is_empty() {
        return Ok(GroupMembersChange {
            project_id,
            conversation_id,
            name: conversation.name,
            version: conversation.version,
            members: group_member_views(transaction, conversation_id).await?,
            added: Vec::new(),
            removed: Vec::new(),
        });
    }
    let added = if to_add.is_empty() {
        Vec::new()
    } else {
        let people = lock_people(transaction, project_id, &to_add).await?;
        if current.len() + to_add.len() - to_remove.len() > MAX_GROUP_MEMBERS {
            return Err(AppError::Conflict(
                "group members",
                "a group can have at most 20 members",
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
            SELECT $1, $2, member, $3 FROM unnest($4::uuid[]) AS member
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(conversation_id)
        .bind(editor.actor_id)
        .bind(&to_add)
        .execute(&mut **transaction)
        .await?;
        // New members can read the whole history, but it isn't counted as
        // unread: their marks start at the newest entry of the group and of
        // each thread under it.
        sqlx::query(
            r#"
            INSERT INTO conversation_read_states (
                project_id, conversation_id, actor_id, highest_seen_sequence, version
            )
            SELECT $1, conversation.id, member, conversation.next_entry_sequence - 1, 1
            FROM conversations conversation
            CROSS JOIN unnest($3::uuid[]) AS member
            WHERE conversation.project_id = $1
              AND (conversation.id = $2
                   OR (conversation.parent_channel_id = $2 AND conversation.kind = 'thread'))
              AND conversation_visible_to(conversation.id, $4)
            ON CONFLICT (project_id, conversation_id, actor_id) DO UPDATE
            SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
                version = conversation_read_states.version + 1,
                updated_at = now()
            WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
            "#,
        )
        .bind(project_id)
        .bind(conversation_id)
        .bind(&to_add)
        .bind(editor.actor_id)
        .execute(&mut **transaction)
        .await?;
        people.into_iter().map(ActorView::from).collect()
    };
    let removed = if to_remove.is_empty() {
        Vec::new()
    } else {
        let removed = member_views(transaction, project_id, &to_remove).await?;
        // A removed agent can no longer see the group or its threads, so its
        // open requests there could never be answered or retried. Withdraw
        // them while the visibility check still sees the group (the editor
        // may be the one leaving), before the member rows go.
        replies::withdraw_requests(
            transaction,
            Withdraw::AgentsInGroup {
                group_id: conversation_id,
                agent_actor_ids: &to_remove,
                editor_actor_id: editor.actor_id,
            },
            "Removed from the group.",
        )
        .await?;
        sqlx::query(
            "DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = ANY($2)",
        )
        .bind(conversation_id)
        .bind(&to_remove)
        .execute(&mut **transaction)
        .await?;
        removed
    };
    let version = sqlx::query_scalar::<_, i64>(
        "UPDATE conversations SET version = version + 1, updated_at = now() WHERE id = $1 RETURNING version",
    )
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    // Everyone who can still see the group (including whoever was just
    // added) hears about the change; each removed actor is told on their own.
    enqueue_event(
        transaction,
        project_id,
        OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
            conversation_id,
            version,
        }),
    )
    .await?;
    for actor_id in &to_remove {
        enqueue_event(
            transaction,
            project_id,
            OutboxEventPayload::ConversationMemberRemoved(ConversationMemberRemovedPayload {
                conversation_id,
                actor_id: *actor_id,
                version,
            }),
        )
        .await?;
    }
    Ok(GroupMembersChange {
        project_id,
        conversation_id,
        name: conversation.name,
        version,
        members: group_member_views(transaction, conversation_id).await?,
        added,
        removed,
    })
}

/// Everyone in a group, by name.
async fn group_member_views(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
) -> Result<Vec<ActorView>, AppError> {
    Ok(sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM conversation_members member
        JOIN actors actor ON actor.id = member.actor_id
        JOIN project_memberships membership
          ON membership.project_id = member.project_id
         AND membership.actor_id = member.actor_id
        WHERE member.conversation_id = $1
        ORDER BY lower(membership.display_name), actor.id
        "#,
    )
    .bind(conversation_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect())
}

/// These members of a space, by name, whether or not they're still enabled
/// (a former member can still be removed from a group).
async fn member_views(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<ActorView>, AppError> {
    Ok(sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1 AND membership.actor_id = ANY($2)
        ORDER BY lower(membership.display_name), actor.id
        "#,
    )
    .bind(project_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect())
}

/// Find or create the DM between two members of a direct space. Returns the
/// conversation and whether it was just created. The other person must be a
/// member of the space; `lock_people` checks that.
pub(crate) async fn open_dm_in(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    other_actor_id: Uuid,
) -> Result<(Uuid, bool), AppError> {
    if other_actor_id == actor_id {
        return Err(AppError::Validation("DM recipient"));
    }
    lock_people(transaction, project_id, &[other_actor_id]).await?;
    // One DM per pair: the sorted pair is unique within the space, so a
    // second request (or a race) finds the existing conversation.
    let (first, second) = if actor_id < other_actor_id {
        (actor_id, other_actor_id)
    } else {
        (other_actor_id, actor_id)
    };
    let dm_key = format!("{first}:{second}");
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO conversations (id, project_id, kind, name, visibility, dm_key)
        VALUES ($1, $2, 'dm', $3, 'members', $4)
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(DM_NAME)
    .bind(&dm_key)
    .fetch_optional(&mut **transaction)
    .await?;
    let created = inserted.is_some();
    let conversation_id = match inserted {
        Some(id) => id,
        None => sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM conversations WHERE project_id = $1 AND kind = 'dm' AND dm_key = $2",
        )
        .bind(project_id)
        .bind(&dm_key)
        .fetch_one(&mut **transaction)
        .await?,
    };
    if created {
        insert_members(
            transaction,
            project_id,
            conversation_id,
            actor_id,
            &[actor_id, other_actor_id],
        )
        .await?;
    }
    Ok((conversation_id, created))
}

/// Create a group with the caller and the given members of a direct space.
pub(crate) async fn create_group_in(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    others: &[Uuid],
    name: Option<String>,
) -> Result<Uuid, AppError> {
    if others.is_empty() || others.len() >= MAX_GROUP_MEMBERS || others.contains(&actor_id) {
        return Err(AppError::Validation("group members"));
    }
    let people = lock_people(transaction, project_id, others).await?;
    let name = match name {
        Some(name) => name,
        None => default_group_name(&people),
    };
    let conversation_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name, visibility)
        VALUES ($1, $2, 'group', $3, 'members')
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(&name)
    .execute(&mut **transaction)
    .await?;
    let mut members = others.to_vec();
    members.push(actor_id);
    insert_members(transaction, project_id, conversation_id, actor_id, &members).await?;
    Ok(conversation_id)
}

/// The direct space a person or agent can start DMs and groups in, with
/// their role there. People get their space created on first use.
pub(crate) async fn space_for_actor(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    agent_connection_id: Option<Uuid>,
) -> Result<(Uuid, MembershipRole), AppError> {
    if let Some(connection_id) = agent_connection_id {
        let (project_id, role) = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT project.id, membership.role
            FROM projects project
            JOIN project_memberships membership
              ON membership.project_id = project.id
             AND membership.actor_id = $1
             AND membership.enabled
            WHERE project.kind = 'direct' AND project.archived_at IS NULL
              AND agent_connection_can_access($2, project.id)
            ORDER BY project.created_at, project.id
            LIMIT 1
            "#,
        )
        .bind(actor_id)
        .bind(connection_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("direct messages"))?;
        return Ok((project_id, parse_role(&role)?));
    }
    let (login_name, display_name) = sqlx::query_as::<_, (String, String)>(
        "SELECT login_name, display_name FROM users WHERE actor_id = $1 AND status = 'active'",
    )
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("user"))?;
    let space = ensure_direct_space(
        transaction,
        &Person {
            actor_id,
            actor_kind: "human",
            login_name: &login_name,
            display_name: &display_name,
        },
    )
    .await?;
    if !space.enabled {
        return Err(AppError::NotFound("direct messages"));
    }
    Ok((space.project_id, space.role))
}

/// The fields of a signed-in person the direct space needs.
struct Person<'a> {
    actor_id: Uuid,
    actor_kind: &'a str,
    login_name: &'a str,
    display_name: &'a str,
}

impl<'a> From<&'a AuthenticatedActor> for Person<'a> {
    fn from(principal: &'a AuthenticatedActor) -> Self {
        Self {
            actor_id: principal.actor_id,
            actor_kind: &principal.actor_kind,
            login_name: &principal.login_name,
            display_name: &principal.display_name,
        }
    }
}

/// Host command support: the direct space for a person, by login name.
/// Returns the space's Project ID and the person's Actor ID.
pub async fn space_for_login(
    pool: &sqlx::PgPool,
    login_name: &str,
) -> Result<(Uuid, Uuid), AppError> {
    let mut transaction = pool.begin().await?;
    let (actor_id, login_name, display_name) = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT actor_id, login_name, display_name FROM users WHERE login_name = $1 AND status = 'active'",
    )
    .bind(login_name)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("user"))?;
    let space = ensure_direct_space(
        &mut transaction,
        &Person {
            actor_id,
            actor_kind: "human",
            login_name: &login_name,
            display_name: &display_name,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok((space.project_id, actor_id))
}

/// Find or create the caller's direct space and make sure they belong to it.
async fn ensure_direct_space(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &Person<'_>,
) -> Result<DirectSpace, AppError> {
    if principal.actor_kind != "human" {
        return Err(AppError::NotFound("direct messages"));
    }
    // Solo-first: a person normally belongs to one Organization. Use the
    // earliest one if there are several.
    let (organization_id, organization_role) = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT organization_id, role
        FROM organization_memberships
        WHERE actor_id = $1 AND enabled
        ORDER BY created_at, organization_id
        LIMIT 1
        "#,
    )
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("organization"))?;
    ensure_space_in(transaction, principal, organization_id, &organization_role).await
}

/// The direct space of `organization_id` for a person who is an enabled
/// member there, created (and joined) if needed. Account-wide notices, such
/// as capacity alerts, are filed under it because they belong to no
/// project. `None` for anyone who isn't an enabled human member.
pub(crate) async fn space_for_member(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let Some((login_name, display_name, organization_role)) =
        sqlx::query_as::<_, (String, String, String)>(
            r#"
            SELECT users.login_name, users.display_name, membership.role
            FROM users
            JOIN organization_memberships membership
              ON membership.actor_id = users.actor_id AND membership.organization_id = $2
            WHERE users.actor_id = $1 AND users.status = 'active' AND membership.enabled
            "#,
        )
        .bind(actor_id)
        .bind(organization_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(None);
    };
    let space = ensure_space_in(
        transaction,
        &Person {
            actor_id,
            actor_kind: "human",
            login_name: &login_name,
            display_name: &display_name,
        },
        organization_id,
        &organization_role,
    )
    .await?;
    Ok(space.enabled.then_some(space.project_id))
}

async fn ensure_space_in(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &Person<'_>,
    organization_id: Uuid,
    organization_role: &str,
) -> Result<DirectSpace, AppError> {
    let project_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name, kind)
        VALUES ($1, $2, $3, $4, 'direct')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(DIRECT_PROJECT_KEY)
    .bind(DIRECT_PROJECT_NAME)
    .execute(&mut **transaction)
    .await?;
    let project_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 AND kind = 'direct'",
    )
    .bind(organization_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Internal("direct space could not be created"))?;
    sqlx::query(
        "INSERT INTO project_counters (project_id, next_work_item_number) VALUES ($1, 1) ON CONFLICT DO NOTHING",
    )
    .bind(project_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO project_instructions (project_id, content) VALUES ($1, '') ON CONFLICT DO NOTHING",
    )
    .bind(project_id)
    .execute(&mut **transaction)
    .await?;

    let role = match organization_role {
        "owner" => "owner",
        "admin" => "admin",
        _ => "member",
    };
    let handle = membership_handle(principal.login_name);
    // Handles are unique per Project. If an agent already took this one,
    // fall back to a suffixed handle rather than failing the first visit.
    let fallback = format!(
        "{}-{}",
        handle.chars().take(24).collect::<String>(),
        &principal.actor_id.simple().to_string()[..6]
    );
    for candidate in [handle, fallback] {
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(principal.actor_id)
        .bind(role)
        .bind(principal.display_name)
        .bind(&candidate)
        .execute(&mut **transaction)
        .await?;
    }
    let (role, enabled) = sqlx::query_as::<_, (String, bool)>(
        "SELECT role, enabled FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Internal("direct space membership"))?;
    Ok(DirectSpace {
        project_id,
        role: parse_role(&role)?,
        enabled,
    })
}

/// Lock the given Actors' memberships and confirm each can join a DM or group.
async fn lock_people(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<PersonRow>, AppError> {
    let rows = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.actor_id = ANY($2)
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
        ORDER BY lower(membership.display_name), actor.id
        FOR SHARE OF membership, actor
        "#,
    )
    .bind(project_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.len() != actor_ids.len() {
        return Err(AppError::NotFound("person"));
    }
    Ok(rows)
}

async fn insert_members(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    added_by: Uuid,
    actor_ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT $1, $2, member, $3 FROM unnest($4::uuid[]) AS member
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(conversation_id)
    .bind(added_by)
    .bind(actor_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (project_id, conversation_id, actor_id, highest_seen_sequence)
        VALUES ($1, $2, $3, 0)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(conversation_id)
    .bind(added_by)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn announce_created(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    conversation_id: Uuid,
    kind: OutboxConversationKind,
    audit_action: &'static str,
) -> Result<(), AppError> {
    enqueue_event(
        transaction,
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id,
            kind,
            version: 1,
        }),
    )
    .await?;
    insert_audit_event(
        transaction,
        headers,
        principal,
        project_id,
        audit_action,
        conversation_id,
        serde_json::json!({}),
    )
    .await
}

async fn conversation_response(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
) -> Result<ConversationCreateResponse, AppError> {
    let (project_id, kind, name, version, seen, read_version) =
        sqlx::query_as::<_, (Uuid, String, String, i64, i64, i64)>(
            r#"
            SELECT conversation.project_id, conversation.kind, conversation.name,
                   conversation.version,
                   COALESCE(read_state.highest_seen_sequence, 0),
                   COALESCE(read_state.version, 0)
            FROM conversations conversation
            LEFT JOIN conversation_read_states read_state
              ON read_state.conversation_id = conversation.id
             AND read_state.actor_id = $2
            WHERE conversation.id = $1
            "#,
        )
        .bind(conversation_id)
        .bind(principal.actor_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(ConversationCreateResponse {
        conversation: ConversationView {
            id: conversation_id,
            project_id,
            kind,
            parent_channel_id: None,
            origin_message_id: None,
            name,
            archived_at: None,
            version,
            thread_status: None,
            thread_outcome: None,
            parent_kind: None,
        },
        read_state: ReadStateView {
            highest_seen_sequence: seen,
            version: read_version,
        },
    })
}

fn default_group_name(people: &[PersonRow]) -> String {
    let joined = people
        .iter()
        .map(|person| person.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.chars().count() <= 120 {
        joined
    } else {
        format!("{}…", joined.chars().take(119).collect::<String>())
    }
}

fn parse_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

fn allows(
    principal: &AuthenticatedActor,
    space: &DirectSpace,
    action: Action,
) -> Result<bool, AppError> {
    let actor_kind = match principal.actor_kind.as_str() {
        "human" => ActorKind::Human,
        _ => return Ok(false),
    };
    let subject = Subject::new(
        principal.actor_id,
        actor_kind,
        true,
        Some(Membership::project(
            space.project_id,
            space.role,
            space.enabled,
        )),
        None,
    );
    Ok(authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(space.project_id)),
    )
    .is_allowed())
}

fn require(
    principal: &AuthenticatedActor,
    space: &DirectSpace,
    action: Action,
) -> Result<(), AppError> {
    if allows(principal, space, action)? {
        Ok(())
    } else {
        Err(AppError::NotFound("direct messages"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_group_names_stay_within_the_name_limit() {
        let person = |name: &str| PersonRow {
            id: Uuid::nil(),
            kind: "external_agent".to_owned(),
            display_name: name.to_owned(),
            handle: "x".to_owned(),
        };
        assert_eq!(
            default_group_name(&[person("Mara"), person("Rook")]),
            "Mara, Rook"
        );
        let long = vec![person(&"a".repeat(80)), person(&"b".repeat(80))];
        assert_eq!(default_group_name(&long).chars().count(), 120);
    }
}
