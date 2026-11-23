//! Direct messages and group conversations (K4.12).
//!
//! DMs and groups belong to no space: they are stored with no project, in
//! the reserved direct scope (`spaces::DIRECT`), and the API names that
//! scope as their place. Seeing one is participation alone
//! (`conversation_visible_to`); what someone may do in it is what their
//! role allows in a workspace they share with everyone else in it
//! (`direct_permissions`). Starting one needs `direct.start` in a workspace
//! shared with each other person (`direct_may_start`). Names come from each
//! account's workspace profile (`account_profile`). DMs from before R20
//! keep their workspace's id and are the pair's DM too.

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
        insert_audit_event, lock_seen_conversation_for, normalize_conversation_name,
    },
    error::{ApiJson, ApiQuery, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    outbox::{
        ConversationCreatedPayload, ConversationMemberAddedPayload,
        ConversationMemberRemovedPayload, ConversationUpdatedPayload, OutboxConversationKind,
        OutboxEventPayload,
    },
    policy::Action,
    replies::{self, Withdraw},
    spaces::DIRECT,
};

const DM_NAME: &str = "Direct message";
pub(crate) const MAX_GROUP_MEMBERS: usize = 20;
const MAX_LISTED_CONVERSATIONS: i64 = 200;
const MAX_LISTED_PEOPLE: i64 = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/direct", get(direct_overview))
        .route("/api/v1/direct/dms", post(open_dm))
        .route("/api/v1/direct/groups", post(create_group))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenDmRequest {
    actor_id: Uuid,
    /// The workspace it's started in; the caller's current one if left out.
    #[serde(default)]
    workspace_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateGroupRequest {
    name: Option<String>,
    member_ids: Vec<Uuid>,
    /// The workspace it's started in; the caller's current one if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectOverview {
    /// The place DMs and groups are named by (the direct scope).
    project_id: Uuid,
    /// The workspace the list was asked for, so the web app can offer its
    /// projects as places for an agent to work.
    organization_id: Uuid,
    /// That workspace's own space: where an agent added without a project
    /// lives.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_space_id: Option<Uuid>,
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
    /// The number a DM row shows, its mentions, and whether it's muted.
    #[serde(flatten)]
    counts: crate::conversations::UnreadCounts,
    last_activity_at: String,
    /// Kept in this workspace (R20), rather than the people's own.
    kept: bool,
    /// Tells apart a pair's DMs when the list has more than one (R31): the
    /// personal one beside a kept one, or DMs from before R20.
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectCapabilities {
    start_conversation: bool,
    manage_agents: bool,
}

#[derive(Debug, FromRow)]
struct DirectConversationRow {
    id: Uuid,
    kind: String,
    name: String,
    version: i64,
    highest_sequence: i64,
    highest_seen_sequence: i64,
    read_version: i64,
    last_activity_at: String,
    members: serde_json::Value,
    dm_key: Option<String>,
    kept: bool,
    workspace_id: Uuid,
    workspace_name: Option<String>,
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OverviewQuery {
    /// The workspace the person is in: its kept DMs are listed with their
    /// own (K4.6: never inferred from the last one used).
    workspace_id: Option<Uuid>,
}

async fn direct_overview(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<OverviewQuery>,
) -> Result<Json<DirectOverview>, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let organization_id =
        crate::workspace::named_or_only(&mut transaction, principal.actor_id, query.workspace_id)
            .await?;
    // Made on first use, like every workspace's own space (K4.10).
    let workspace_space_id =
        crate::spaces::workspace::join(&mut transaction, organization_id, principal.actor_id)
            .await?;
    let rows = sqlx::query_as::<_, DirectConversationRow>(
        r#"
        SELECT
            conversation.id,
            conversation.kind,
            conversation.name,
            conversation.version,
            conversation.next_entry_sequence - 1 AS highest_sequence,
            COALESCE(read_state.highest_seen_sequence, 0) AS highest_seen_sequence,
            COALESCE(read_state.version, 0) AS read_version,
            COALESCE(last_entry.created_at, conversation.created_at)::text AS last_activity_at,
            COALESCE(members.actors, '[]'::jsonb) AS members,
            conversation.dm_key,
            conversation.kept,
            conversation.workspace_id,
            home.name AS workspace_name
        FROM conversation_members me
        JOIN conversations conversation ON conversation.id = me.conversation_id
        LEFT JOIN organizations home ON home.id = conversation.workspace_id
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = conversation.id
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
                    'displayName', profile.display_name,
                    'handle', profile.handle,
                    'avatarUrl', NULL
                )
                ORDER BY lower(profile.display_name), actor.id
            ) AS actors
            FROM conversation_members member
            JOIN actors actor ON actor.id = member.actor_id
            CROSS JOIN LATERAL account_profile(member.actor_id, conversation.workspace_id) profile
            WHERE member.conversation_id = conversation.id
        ) members ON true
        WHERE me.actor_id = $1
          AND conversation.project_id IS NULL
          AND conversation.kind IN ('dm', 'group')
          AND conversation.archived_at IS NULL
          -- One list everywhere (R20): the person's own DMs, plus the ones
          -- this workspace keeps.
          AND (NOT conversation.kept OR conversation.workspace_id = $3)
          AND conversation_visible_to(conversation.id, $1)
          AND NOT direct_hidden_for(conversation.id, me.actor_id)
        ORDER BY COALESCE(last_entry.created_at, conversation.created_at) DESC, conversation.id
        LIMIT $2
        "#,
    )
    .bind(principal.actor_id)
    .bind(MAX_LISTED_CONVERSATIONS)
    .bind(organization_id)
    .fetch_all(&mut *transaction)
    .await?;
    // How many DMs each pair has in the list: more than one, and each
    // personal one says whose it is.
    let mut per_pair = std::collections::HashMap::<&str, usize>::new();
    for row in &rows {
        if let Some(key) = row.dm_key.as_deref() {
            *per_pair.entry(key).or_default() += 1;
        }
    }
    let labels = rows
        .iter()
        .map(|row| {
            let shared = row
                .dm_key
                .as_deref()
                .is_some_and(|key| per_pair.get(key).copied().unwrap_or(0) > 1);
            (shared && !row.kept).then(|| {
                if row.workspace_id == DIRECT {
                    "Personal".to_owned()
                } else {
                    row.workspace_name.clone().unwrap_or_default()
                }
            })
        })
        .collect::<Vec<_>>();
    let summary = crate::read_state::unread_by_conversation(
        &mut transaction,
        principal.actor_id,
        crate::read_state::UnreadScope::Direct,
    )
    .await?;
    let conversations = rows
        .into_iter()
        .zip(labels)
        .map(|(row, label)| {
            let (unread, counts) = crate::conversations::UnreadCounts::of(&summary, row.id);
            let read_state = ReadStateView {
                highest_seen_sequence: row.highest_seen_sequence,
                version: row.read_version,
            };
            Ok(DirectConversationItem {
                conversation: ConversationView {
                    id: row.id,
                    project_id: DIRECT,
                    kind: row.kind,
                    parent_channel_id: None,
                    origin_message_id: None,
                    name: row.name,
                    archived_at: None,
                    version: row.version,
                    thread_status: None,
                    thread_outcome: None,
                    parent_kind: None,
                    private: false,
                },
                members: serde_json::from_value(row.members)
                    .map_err(|_| AppError::Internal("direct conversation members"))?,
                highest_sequence: row.highest_sequence,
                unread,
                counts,
                read_state,
                last_activity_at: row.last_activity_at,
                kept: row.kept,
                label,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    // Everyone the person may start a DM with: the enabled members of the
    // workspaces where their role allows starting one.
    let people = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, profile.display_name, profile.handle
        FROM actors actor
        CROSS JOIN LATERAL account_profile(actor.id, $2) profile
        WHERE actor.id IN (
                SELECT theirs.actor_id
                FROM organization_memberships mine
                JOIN organizations workspace
                  ON workspace.id = mine.organization_id AND workspace.lifecycle = 'active'
                JOIN organization_memberships theirs
                  ON theirs.organization_id = mine.organization_id AND theirs.enabled
                WHERE mine.actor_id = $1 AND mine.enabled
                  AND role_allows(mine.role, mine.role_id, 'direct.start'))
          AND actor.id <> $1
          AND actor.status = 'active'
        ORDER BY (actor.kind = 'external_agent') DESC, lower(profile.display_name), actor.id -- // kind-check: mechanical (list order)
        LIMIT $3
        "#,
    )
    .bind(principal.actor_id)
    .bind(organization_id)
    .bind(MAX_LISTED_PEOPLE)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect();
    let standing =
        crate::roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
    let capabilities = DirectCapabilities {
        start_conversation: may_start_any(&mut transaction, principal.actor_id).await?,
        manage_agents: standing.may(Action::ExternalAgentManage.permission()),
    };
    transaction.commit().await?;
    Ok(Json(DirectOverview {
        project_id: DIRECT,
        organization_id,
        workspace_space_id,
        conversations,
        people,
        capabilities,
    }))
}

/// Whether the account may start a DM or group with anyone: its role in
/// some workspace allows `direct.start`.
pub(crate) async fn may_start_any(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM organization_memberships mine
            JOIN organizations workspace
              ON workspace.id = mine.organization_id AND workspace.lifecycle = 'active'
            JOIN actors me ON me.id = mine.actor_id AND me.status = 'active'
            WHERE mine.actor_id = $1 AND mine.enabled
              AND role_allows(mine.role, mine.role_id, $2))
        "#,
    )
    .bind(actor_id)
    .bind(crate::policy::Permission::DirectStart.key())
    .fetch_one(&mut **transaction)
    .await?)
}

async fn open_dm(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<OpenDmRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    // Started inside the workspace the person is in, a kept workspace's
    // DM is its own; with none named it is personal (K4.6).
    let inside = request.workspace_id;
    let (conversation_id, created) = open_dm_in(
        &mut transaction,
        principal.actor_id,
        request.actor_id,
        inside,
    )
    .await?;
    if created {
        announce_created(
            &mut transaction,
            &headers,
            &principal,
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
    require_authenticated_write(&state, &headers, &principal).await?;
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
        workspace_id: request.workspace_id,
    })?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let scope = idempotency::resource_bound_scope("account", principal.actor_id, "group_create")?;
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
    // Started inside the workspace the person is in, a kept workspace's
    // DM is its own; with none named it is personal (K4.6).
    let inside = request.workspace_id;
    let conversation_id =
        create_group_in(&mut transaction, principal.actor_id, &others, name, inside).await?;
    announce_created(
        &mut transaction,
        &headers,
        &principal,
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

/// Who changes a group's members.
pub(crate) struct GroupEditor {
    pub actor_id: Uuid,
}

/// What `change_group_members` did. `added` and `removed` hold only the
/// actors whose membership actually changed; both empty means nothing did.
pub(crate) struct GroupMembersChange {
    /// The group's space: none, for a group outside one.
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub name: String,
    pub version: i64,
    /// Everyone in the group after the change.
    pub members: Vec<ActorView>,
    pub added: Vec<ActorView>,
    pub removed: Vec<ActorView>,
}

/// Add and remove people and agents in a group or a private channel (C2.6).
/// Shared by the browser API and MCP; callers record their own audit rows.
///
/// A private channel's managers (whoever made it, to begin with) and the
/// space's moderators change its members; any member can leave. Whoever is
/// added must be in the channel's space already, and there's no cap.
///
/// - Visibility goes through `lock_seen_conversation_for`
///   (`conversation_visible_to`), which also locks the group row, so
///   concurrent changes are serialized and the cap is checked under that
///   lock. Someone who can't see the group gets `NotFound("conversation")`.
/// - Leaving needs only to see the group; adding or removing others needs
///   `conversations.members` there (`Forbidden`), whoever is asking:
///   whoever is added reads the whole history, so roles decide who may hand
///   that out. Whoever is added must be someone the editor may start a
///   conversation with (`direct_may_start`).
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
    let conversation = lock_seen_conversation_for(
        transaction,
        editor.actor_id,
        conversation_id,
        Action::ProjectView,
    )
    .await?;
    // Groups, and private channels (K4.11, C2.6), have members; nothing
    // else does. A private channel's managers, or whoever moderates its
    // space, change its members; a group's, whoever's role there holds
    // `conversations.members`. Anyone may leave.
    let private_channel = conversation.private && conversation.kind == "channel";
    if private_channel {
        if !leaving_only && !manages_channel(transaction, &conversation, &editor).await? {
            return Err(AppError::Forbidden);
        }
    } else if conversation.kind != "group" {
        return Err(AppError::Conflict(
            "conversation",
            "only groups and private channels change members",
        ));
    } else if !leaving_only
        && !conversation.allows(editor.actor_id, Action::ConversationMembersChange)?
    {
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
    let place = conversation.place();
    let workspace_id = conversation.workspace_id;
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
        let people = if private_channel {
            // A private channel's members read its space; who may add them
            // is the channel's role, checked as it was locked.
            space_readers(transaction, workspace_id, conversation.place(), &to_add).await?
        } else {
            lock_startable(transaction, editor.actor_id, workspace_id, &to_add).await?
        };
        if !private_channel && current.len() + to_add.len() - to_remove.len() > MAX_GROUP_MEMBERS {
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
            SELECT conversation.project_id, conversation.id, member,
                   conversation.next_entry_sequence - 1, 1
            FROM conversations conversation
            CROSS JOIN unnest($2::uuid[]) AS member
            WHERE (conversation.id = $1
                   OR (conversation.parent_channel_id = $1 AND conversation.kind = 'thread'))
              AND conversation_visible_to(conversation.id, $3)
            ON CONFLICT (conversation_id, actor_id) DO UPDATE
            SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
                version = conversation_read_states.version + 1,
                updated_at = now()
            WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
            "#,
        )
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
        let removed = member_views(transaction, workspace_id, &to_remove).await?;
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
        place,
        OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
            conversation_id,
            version,
        }),
    )
    .await?;
    for actor in &added {
        enqueue_event(
            transaction,
            place,
            OutboxEventPayload::ConversationMemberAdded(ConversationMemberAddedPayload {
                conversation_id,
                actor_id: actor.id,
                version,
            }),
        )
        .await?;
    }
    for actor_id in &to_remove {
        enqueue_event(
            transaction,
            place,
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

/// These accounts, locked, if each reads `space`; `NotFound` names none
/// that doesn't (a private channel's members read its space).
async fn space_readers(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    space: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<PersonRow>, AppError> {
    let people = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, profile.display_name, profile.handle
        FROM actors actor
        CROSS JOIN LATERAL account_profile(actor.id, $1) profile
        WHERE actor.id = ANY($2) AND actor.status = 'active'
          AND space_readable_by($3, actor.id)
        ORDER BY actor.id
        FOR SHARE OF actor
        "#,
    )
    .bind(workspace_id)
    .bind(actor_ids)
    .bind(space)
    .fetch_all(&mut **transaction)
    .await?;
    if people.len() != actor_ids.len() {
        return Err(AppError::NotFound("person"));
    }
    Ok(people)
}

/// Whether `editor` changes a private channel's members: one of its
/// managers, or someone who moderates its space.
async fn manages_channel(
    transaction: &mut Transaction<'_, Postgres>,
    channel: &crate::conversations::ConversationAccessRow,
    editor: &GroupEditor,
) -> Result<bool, AppError> {
    if channel.allows(editor.actor_id, Action::MessageModerate)? {
        return Ok(true);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM conversation_members
            WHERE conversation_id = $1 AND actor_id = $2 AND manager
        )
        "#,
    )
    .bind(channel.id)
    .bind(editor.actor_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Everyone in a group, by name.
async fn group_member_views(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
) -> Result<Vec<ActorView>, AppError> {
    Ok(sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, profile.display_name, profile.handle
        FROM conversation_members member
        JOIN conversations conversation ON conversation.id = member.conversation_id
        JOIN actors actor ON actor.id = member.actor_id
        CROSS JOIN LATERAL account_profile(member.actor_id, conversation.workspace_id) profile
        -- visibility: the caller locked the group through conversation_visible_to.
        WHERE member.conversation_id = $1
        ORDER BY lower(profile.display_name), actor.id
        "#,
    )
    .bind(conversation_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect())
}

/// These accounts, by the names they go by in `workspace_id` (or another
/// workspace of theirs), whether or not they're still active (a former
/// member can still be removed from a group).
async fn member_views(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<ActorView>, AppError> {
    Ok(sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT actor.id, actor.kind, profile.display_name, profile.handle
        FROM actors actor
        CROSS JOIN LATERAL account_profile(actor.id, $1) profile
        WHERE actor.id = ANY($2)
        ORDER BY lower(profile.display_name), actor.id
        "#,
    )
    .bind(workspace_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ActorView::from)
    .collect())
}

/// Find or create the DM between two accounts. Returns the conversation
/// and whether it was just created. An existing DM is found whoever may
/// start one now (it stays readable); making one needs `direct.start` in a
/// workspace both are in.
///
/// `inside` is the workspace the person is in as they start it: when that
/// workspace keeps its DMs (R20's `kept`) and both are its members, the
/// pair's kept DM there is found or made instead of their personal one.
pub(crate) async fn open_dm_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    other_actor_id: Uuid,
    inside: Option<Uuid>,
) -> Result<(Uuid, bool), AppError> {
    // A DM with yourself is your note to self (C2.32): one member, kept
    // like any other DM.
    let alone = other_actor_id == actor_id;
    let (first, second) = if actor_id < other_actor_id {
        (actor_id, other_actor_id)
    } else {
        (other_actor_id, actor_id)
    };
    let dm_key = format!("{first}:{second}");
    let kept = kept_in(transaction, inside, actor_id, &[other_actor_id]).await?;
    if let Some(id) = pair_dm(transaction, kept, actor_id, &dm_key).await? {
        return Ok((id, false));
    }
    if let Some(workspace) = kept {
        require_start_in(transaction, workspace, actor_id).await?;
    }
    if !alone {
        lock_startable(
            transaction,
            actor_id,
            kept.unwrap_or(DIRECT),
            &[other_actor_id],
        )
        .await?;
    }
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO conversations (id, project_id, workspace_id, kind, name, visibility, dm_key, kept)
        VALUES ($1, NULL, COALESCE($4, direct_scope_id()), 'dm', $2, 'members', $3, $4 IS NOT NULL)
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(DM_NAME)
    .bind(&dm_key)
    .bind(kept)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(conversation_id) = inserted else {
        // Someone else made it just now.
        let id = pair_dm(transaction, kept, actor_id, &dm_key)
            .await?
            .ok_or(AppError::Conflict("conversation", "try again"))?;
        return Ok((id, false));
    };
    let members = if alone {
        vec![actor_id]
    } else {
        vec![actor_id, other_actor_id]
    };
    insert_members(transaction, conversation_id, actor_id, &members).await?;
    Ok((conversation_id, true))
}

/// Create a group of the caller and others they may start a conversation
/// with. Started `inside` a workspace that keeps its DMs, among its
/// members only, the group is kept there (R20).
pub(crate) async fn create_group_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    others: &[Uuid],
    name: Option<String>,
    inside: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if others.is_empty() || others.len() >= MAX_GROUP_MEMBERS || others.contains(&actor_id) {
        return Err(AppError::Validation("group members"));
    }
    let kept = kept_in(transaction, inside, actor_id, others).await?;
    if let Some(workspace) = kept {
        require_start_in(transaction, workspace, actor_id).await?;
    }
    let people = lock_startable(transaction, actor_id, kept.unwrap_or(DIRECT), others).await?;
    let name = match name {
        Some(name) => name,
        None => default_group_name(&people),
    };
    let conversation_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, workspace_id, kind, name, visibility, kept)
        VALUES ($1, NULL, COALESCE($3, direct_scope_id()), 'group', $2, 'members', $3 IS NOT NULL)
        "#,
    )
    .bind(conversation_id)
    .bind(&name)
    .bind(kept)
    .execute(&mut **transaction)
    .await?;
    let mut members = others.to_vec();
    members.push(actor_id);
    insert_members(transaction, conversation_id, actor_id, &members).await?;
    Ok(conversation_id)
}

/// The workspace a new DM or group is kept in (R20): `inside`, when it is
/// active, its `messages.direct_messages` setting is `kept`, and the
/// starter and everyone else are enabled members of it.
async fn kept_in(
    transaction: &mut Transaction<'_, Postgres>,
    inside: Option<Uuid>,
    actor_id: Uuid,
    others: &[Uuid],
) -> Result<Option<Uuid>, AppError> {
    let Some(workspace) = inside.filter(|workspace| *workspace != DIRECT) else {
        return Ok(None);
    };
    let setting =
        crate::settings::workspace_value(&mut **transaction, workspace, "messages.direct_messages")
            .await?;
    if setting != serde_json::json!("kept") {
        return Ok(None);
    }
    let mut members = others.to_vec();
    members.push(actor_id);
    members.sort();
    members.dedup();
    let all_in = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT count(DISTINCT membership.actor_id) = cardinality($2::uuid[])
        FROM organization_memberships membership
        JOIN organizations workspace
          ON workspace.id = membership.organization_id AND workspace.lifecycle = 'active'
        JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
        WHERE membership.organization_id = $1 AND membership.enabled
          AND membership.actor_id = ANY($2)
        "#,
    )
    .bind(workspace)
    .bind(&members)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(all_in.then_some(workspace))
}

/// A kept DM or group answers to its own workspace's role only, so
/// starting one needs `direct.start` there.
async fn require_start_in(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    let allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM organization_memberships mine
            WHERE mine.organization_id = $1 AND mine.actor_id = $2 AND mine.enabled
              AND role_allows(mine.role, mine.role_id, $3))
        "#,
    )
    .bind(workspace)
    .bind(actor_id)
    .bind(crate::policy::Permission::DirectStart.key())
    .fetch_one(&mut **transaction)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// The pair's DM that `actor_id` sees: their kept one in `kept`'s
/// workspace, or else their personal one (R20: one per pair; the newest from
/// before R20, when DMs lived in a workspace, if there is no other).
async fn pair_dm(
    transaction: &mut Transaction<'_, Postgres>,
    kept: Option<Uuid>,
    actor_id: Uuid,
    dm_key: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM conversations
        WHERE kind = 'dm' AND dm_key = $1 AND project_id IS NULL
          AND CASE WHEN $3::uuid IS NULL THEN NOT kept
                   ELSE kept AND workspace_id = $3 END
          AND conversation_visible_to(id, $2)
        ORDER BY (workspace_id = direct_scope_id()) DESC, created_at DESC, id
        LIMIT 1
        "#,
    )
    .bind(dm_key)
    .bind(actor_id)
    .bind(kept)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Lock these accounts and confirm `actor_id` may start a conversation
/// with each (`direct_may_start`); their names as they go by in
/// `workspace_id` (or another workspace of theirs). `Forbidden` when they
/// share a workspace whose role doesn't allow it, `NotFound` otherwise.
async fn lock_startable(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    workspace_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<PersonRow>, AppError> {
    #[derive(FromRow)]
    struct Startable {
        #[sqlx(flatten)]
        person: PersonRow,
        may_start: bool,
    }
    let rows = sqlx::query_as::<_, Startable>(
        r#"
        SELECT actor.id, actor.kind, profile.display_name, profile.handle,
               direct_may_start($1, actor.id) AS may_start
        FROM actors actor
        CROSS JOIN LATERAL account_profile(actor.id, $2) profile
        WHERE actor.id = ANY($3)
          AND actor.status = 'active'
          AND EXISTS (
              SELECT 1
              FROM organization_memberships mine
              JOIN organization_memberships theirs
                ON theirs.organization_id = mine.organization_id
               AND theirs.actor_id = actor.id AND theirs.enabled
              WHERE mine.actor_id = $1 AND mine.enabled)
        ORDER BY lower(profile.display_name), actor.id
        FOR SHARE OF actor
        "#,
    )
    .bind(actor_id)
    .bind(workspace_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.len() != actor_ids.len() {
        return Err(AppError::NotFound("person"));
    }
    if rows.iter().any(|row| !row.may_start) {
        return Err(AppError::Forbidden);
    }
    Ok(rows.into_iter().map(|row| row.person).collect())
}

async fn insert_members(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    added_by: Uuid,
    actor_ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT NULL, $1, member, $2 FROM unnest($3::uuid[]) AS member
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(conversation_id)
    .bind(added_by)
    .bind(actor_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (project_id, conversation_id, actor_id, highest_seen_sequence)
        VALUES (NULL, $1, $2, 0)
        ON CONFLICT DO NOTHING
        "#,
    )
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
    conversation_id: Uuid,
    kind: OutboxConversationKind,
    audit_action: &'static str,
) -> Result<(), AppError> {
    enqueue_event(
        transaction,
        DIRECT,
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
        DIRECT,
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
    let (kind, name, version, seen, read_version) =
        sqlx::query_as::<_, (String, String, i64, i64, i64)>(
            r#"
            SELECT conversation.kind, conversation.name,
                   conversation.version,
                   COALESCE(read_state.highest_seen_sequence, 0),
                   COALESCE(read_state.version, 0)
            FROM conversations conversation
            LEFT JOIN conversation_read_states read_state
              ON read_state.conversation_id = conversation.id
             AND read_state.actor_id = $2
            WHERE conversation.id = $1
              AND conversation_visible_to(conversation.id, $2)
            "#,
        )
        .bind(conversation_id)
        .bind(principal.actor_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(ConversationCreateResponse {
        conversation: ConversationView {
            id: conversation_id,
            project_id: DIRECT,
            kind,
            parent_channel_id: None,
            origin_message_id: None,
            name,
            archived_at: None,
            version,
            thread_status: None,
            thread_outcome: None,
            parent_kind: None,
            private: false,
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
