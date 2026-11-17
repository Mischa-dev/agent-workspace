//! The one way a thread is made (C1.2, ruling R9).
//!
//! Every path that starts a thread (the web dialog, `thread_start`, an agent
//! answering a channel tag, "work with a worker", a task launch, a tagged
//! run) calls [`create`]. It locks the parent through
//! `lock_conversation_for` (so visibility is checked again after any wait),
//! refuses a parent that is a thread, archived or out of reach, reuses the
//! open thread already started from the same message when asked, names the
//! thread by one policy, and always writes the creator's read state, one
//! `ConversationCreated` and an audit row. Nothing else inserts a thread
//! (`structure_guards`' `conversation_inserts`).

use axum::http::HeaderMap;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    ConversationAccessRow, UNNAMED_THREAD, enqueue_event, lock_conversation_for,
    map_conversation_insert, normalize_conversation_name, thread_name_from_text,
};
use crate::{
    auth::AuthenticatedActor,
    error::AppError,
    outbox::{ConversationCreatedPayload, OutboxConversationKind, OutboxEventPayload},
    policy::{Action, ActorKind},
};

/// How a new thread is named.
#[derive(Clone, Debug)]
pub(crate) enum ThreadName {
    /// Someone named it. A name already used by an open thread under the
    /// same parent is refused (409), as the web dialog expects.
    Given(String),
    /// Named from its origin message, else its first message, until someone
    /// names it (`auto_named`): editing or removing that message renames it
    /// (`refresh_auto_thread_names`), so its words never outlive the message.
    Derived,
    /// A name a caller composes (a task launch, an agent naming its own
    /// thread): made unique under the parent with " (2)", " (3)"…
    Composed(String),
}

/// What to make.
#[derive(Clone, Debug)]
pub(crate) struct ThreadSpec {
    /// The message in the parent the thread starts from.
    pub origin_message_id: Option<Uuid>,
    pub name: ThreadName,
    /// Return the open thread already started from the origin message
    /// instead of making a second one.
    pub reuse_for_origin: bool,
    /// Link the new thread to this task (`work_item_conversation_links`).
    pub link_task: Option<Uuid>,
    /// Extra audit metadata (a reply request, a launch). Ids only.
    pub audit: serde_json::Value,
    /// Where on a thing the thread is about (C1.3), already checked by
    /// `discussion::anchors`. Only under a comments conversation.
    pub anchor: Option<serde_json::Value>,
}

impl ThreadSpec {
    pub(crate) fn new(name: ThreadName) -> Self {
        Self {
            origin_message_id: None,
            name,
            reuse_for_origin: false,
            link_task: None,
            audit: serde_json::json!({}),
            anchor: None,
        }
    }

    pub(crate) fn anchored(mut self, anchor: Option<serde_json::Value>) -> Self {
        self.anchor = anchor;
        self
    }

    pub(crate) fn opened_from(mut self, origin_message_id: Option<Uuid>) -> Self {
        self.origin_message_id = origin_message_id;
        self
    }

    pub(crate) fn reusing(mut self) -> Self {
        self.reuse_for_origin = true;
        self
    }

    pub(crate) fn linked_to_task(mut self, work_item_id: Uuid) -> Self {
        self.link_task = Some(work_item_id);
        self
    }

    pub(crate) fn audited_with(mut self, metadata: serde_json::Value) -> Self {
        self.audit = metadata;
        self
    }
}

/// Who is making the thread, and through which edge: the audit row's
/// columns. Constructors cover each edge.
#[derive(Clone, Debug)]
pub(crate) struct ThreadCreator {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    /// Refuse (as not found) a parent outside this project: an MCP call
    /// acts in its principal's project.
    pub project_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub authentication_class: &'static str,
    pub channel: Option<&'static str>,
    pub client_name: Option<String>,
    pub mcp_credential_id: Option<Uuid>,
    pub mcp_oauth_grant_id: Option<Uuid>,
    pub external_agent_connection_id: Option<Uuid>,
    pub request_id: Option<String>,
}

impl ThreadCreator {
    /// A person in the browser.
    pub(crate) fn browser(
        principal: &AuthenticatedActor,
        headers: &HeaderMap,
    ) -> Result<Self, AppError> {
        Ok(Self {
            actor_id: principal.actor_id,
            actor_kind: super::parse_actor_kind(&principal.actor_kind)?,
            project_id: None,
            session_id: Some(principal.session_id),
            authentication_class: "password_session",
            channel: Some("browser"),
            client_name: None,
            mcp_credential_id: None,
            mcp_oauth_grant_id: None,
            external_agent_connection_id: None,
            request_id: headers
                .get(crate::REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= 128)
                .map(str::to_owned),
        })
    }

    /// An agent or connected app over MCP, confined to its project.
    pub(crate) fn mcp(principal: &crate::mcp::McpPrincipal) -> Self {
        Self {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind,
            project_id: Some(principal.project_id),
            session_id: None,
            authentication_class: principal.authentication_class,
            channel: Some("mcp"),
            client_name: Some(principal.client_name.chars().take(80).collect()),
            mcp_credential_id: principal.mcp_credential_id,
            mcp_oauth_grant_id: principal.oauth_grant_id,
            external_agent_connection_id: principal.external_agent_connection_id,
            request_id: None,
        }
    }

    /// Anyone, on a path that keeps its own audit columns (a launch).
    pub(crate) fn plain(
        actor_id: Uuid,
        actor_kind: ActorKind,
        session_id: Option<Uuid>,
        authentication_class: &'static str,
        request_id: Option<String>,
    ) -> Self {
        Self {
            actor_id,
            actor_kind,
            project_id: None,
            session_id,
            authentication_class,
            channel: None,
            client_name: None,
            mcp_credential_id: None,
            mcp_oauth_grant_id: None,
            external_agent_connection_id: None,
            request_id,
        }
    }
}

/// The thread made (or found, with `created: false`).
#[derive(Clone, Debug)]
pub(crate) struct CreatedThread {
    pub id: Uuid,
    pub name: String,
    pub created: bool,
    pub status: String,
    pub outcome: Option<String>,
    /// The parent, as locked and checked.
    pub parent: ConversationAccessRow,
}

/// The refusal for a parent that can't hold threads.
pub(crate) const NOT_A_THREAD_PARENT: AppError = AppError::Refused {
    field: "conversationId",
    message: "Threads go in a channel, DM, or group, not inside another thread.",
};

/// The refusal for an origin message that was removed.
pub(crate) const ORIGIN_REMOVED: AppError = AppError::Conflict("message", "removed");

/// Make a thread under `parent_id`, or find the one already started from the
/// same message (`reuse_for_origin`). Runs in the caller's transaction, so a
/// caller that does more (posting, linking a run) commits it all at once.
///
/// Errors: `NotFound("conversation")` when the parent is out of reach or
/// archived; [`NOT_A_THREAD_PARENT`]; `NotFound("message")` when the origin
/// isn't a message of the parent; [`ORIGIN_REMOVED`]; a 409 for a given name
/// already in use.
pub(crate) async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    creator: &ThreadCreator,
    parent_id: Uuid,
    spec: ThreadSpec,
) -> Result<CreatedThread, AppError> {
    let parent = lock_conversation_for(
        transaction,
        creator.actor_id,
        creator.actor_kind,
        parent_id,
        Action::ThreadCreate,
        false,
    )
    .await?;
    if creator
        .project_id
        .is_some_and(|project_id| project_id != parent.project_id)
    {
        return Err(AppError::NotFound("conversation"));
    }
    if !matches!(
        parent.kind.as_str(),
        "channel" | "dm" | "group" | "comments"
    ) {
        return Err(NOT_A_THREAD_PARENT);
    }
    if spec.anchor.is_some() && parent.kind != "comments" {
        return Err(AppError::Refused {
            field: "anchor",
            message: "Only a thread in a thing's comments is anchored to a place in it.",
        });
    }

    let mut origin_text = None;
    let mut origin_author = None;
    if let Some(origin_id) = spec.origin_message_id {
        // Whether it was removed comes first: a removed message's text is
        // never read.
        let origin = sqlx::query_as::<_, (bool, String, Uuid)>(
            r#"
            -- visibility: a message of the parent locked and checked above
            SELECT removed_at IS NOT NULL,
                   CASE WHEN removed_at IS NULL THEN plain_text ELSE '' END,
                   author_actor_id
            FROM messages
            WHERE id = $1 AND conversation_id = $2 AND project_id = $3
            "#,
        )
        .bind(origin_id)
        .bind(parent.id)
        .bind(parent.project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("message"))?;
        if spec.reuse_for_origin
            && let Some((id, name, status, outcome)) =
                sqlx::query_as::<_, (Uuid, String, String, Option<String>)>(
                    r#"
                    -- visibility: a thread under the parent checked above
                    SELECT id, name, thread_status, thread_outcome FROM conversations
                    WHERE origin_message_id = $1 AND parent_channel_id = $2
                      AND kind = 'thread' AND archived_at IS NULL
                    ORDER BY created_at, id
                    LIMIT 1
                    "#,
                )
                .bind(origin_id)
                .bind(parent.id)
                .fetch_optional(&mut **transaction)
                .await?
        {
            return Ok(CreatedThread {
                id,
                name,
                created: false,
                status,
                outcome,
                parent,
            });
        }
        if origin.0 {
            return Err(ORIGIN_REMOVED);
        }
        origin_text = Some(origin.1);
        origin_author = Some(origin.2);
    }

    let (name, auto_named) = match &spec.name {
        ThreadName::Given(name) => (normalize_conversation_name(name)?, false),
        ThreadName::Derived => (
            origin_text
                .as_deref()
                .and_then(thread_name_from_text)
                .unwrap_or_else(|| UNNAMED_THREAD.to_owned()),
            true,
        ),
        ThreadName::Composed(base) => {
            let base = normalize_conversation_name(base)?;
            (unique_name(transaction, parent.id, &base).await?, false)
        }
    };

    let thread_id = Uuid::now_v7();
    let inserted = insert(
        transaction,
        NewConversation {
            id: thread_id,
            project_id: parent.project_id,
            kind: "thread",
            parent_id: Some(parent.id),
            origin_message_id: spec.origin_message_id,
            name: &name,
            auto_named,
            visibility: "project",
            subject: None,
            anchor: spec.anchor.as_ref(),
        },
    )
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
    .bind(parent.project_id)
    .bind(thread_id)
    .bind(creator.actor_id)
    .execute(&mut **transaction)
    .await?;
    // C2.5: whoever starts a thread, and whoever wrote the message it opens
    // from, follows it.
    let followers = std::iter::once(creator.actor_id)
        .chain(origin_author)
        .collect::<Vec<_>>();
    crate::notification_settings::follow_automatically(
        transaction,
        parent.project_id,
        thread_id,
        &followers,
    )
    .await?;
    if let Some(work_item_id) = spec.link_task {
        sqlx::query(
            r#"
            INSERT INTO work_item_conversation_links (
                project_id, work_item_id, conversation_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(parent.project_id)
        .bind(work_item_id)
        .bind(thread_id)
        .bind(creator.actor_id)
        .execute(&mut **transaction)
        .await?;
    }
    enqueue_event(
        transaction,
        parent.project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: thread_id,
            kind: OutboxConversationKind::Thread,
            version: 1,
        }),
    )
    .await?;
    let mut metadata = serde_json::json!({ "parent_channel_id": parent.id });
    if let (Some(object), Some(extra)) = (metadata.as_object_mut(), spec.audit.as_object()) {
        object.extend(extra.clone());
    }
    if let Some(work_item_id) = spec.link_task {
        metadata["work_item_id"] = work_item_id.to_string().into();
    }
    audit(
        transaction,
        creator,
        "conversation.thread_created",
        parent.project_id,
        thread_id,
        metadata,
    )
    .await?;
    Ok(CreatedThread {
        id: thread_id,
        name,
        created: true,
        status: "Open".to_owned(),
        outcome: None,
        parent,
    })
}

/// `base`, or `base (2)`, `base (3)`… whichever no open thread under the
/// parent uses (names are compared without case, as the index does).
pub(crate) async fn unique_name(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
    base: &str,
) -> Result<String, AppError> {
    let taken = sqlx::query_scalar::<_, String>(
        r#"
        -- visibility: names only, under a parent the caller locked and may see
        SELECT lower(name) FROM conversations
        WHERE parent_channel_id = $1 AND kind = 'thread' AND archived_at IS NULL
          AND NOT auto_named
        "#,
    )
    .bind(parent_id)
    .fetch_all(&mut **transaction)
    .await?;
    if !taken.contains(&base.to_lowercase()) {
        return Ok(base.to_owned());
    }
    for number in 2..1000 {
        let suffix = format!(" ({number})");
        let stem = base
            .chars()
            .take(super::MAX_CONVERSATION_NAME_CHARACTERS - suffix.chars().count())
            .collect::<String>();
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict(
        "conversation_name",
        "an active Conversation already uses that name",
    ))
}

async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    creator: &ThreadCreator,
    action: &str,
    project_id: Uuid,
    thread_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata,
            channel, client_name, mcp_credential_id, mcp_oauth_grant_id,
            external_agent_connection_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $14, 'conversation', $6,
            'succeeded', $7, $8, $9, $10, $11, $12, $13
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(creator.actor_id)
    .bind(project_id)
    .bind(creator.session_id)
    .bind(creator.authentication_class)
    .bind(thread_id)
    .bind(creator.request_id.as_deref())
    .bind(metadata)
    .bind(creator.channel)
    .bind(creator.client_name.as_deref())
    .bind(creator.mcp_credential_id)
    .bind(creator.mcp_oauth_grant_id)
    .bind(creator.external_agent_connection_id)
    .bind(action)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A conversation row to insert: a thread here, or a thing's comments
/// conversation (`open_comments`). The one `INSERT INTO conversations` for
/// both, so every made conversation passes the same constraints.
struct NewConversation<'a> {
    id: Uuid,
    project_id: Uuid,
    kind: &'static str,
    parent_id: Option<Uuid>,
    origin_message_id: Option<Uuid>,
    name: &'a str,
    auto_named: bool,
    visibility: &'static str,
    subject: Option<(&'a str, Uuid)>,
    anchor: Option<&'a serde_json::Value>,
}

/// Inserts the row; a second comments conversation for the same thing
/// inserts nothing (`rows_affected` is 0) and the caller reads the first.
async fn insert(
    transaction: &mut Transaction<'_, Postgres>,
    row: NewConversation<'_>,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, workspace_id, kind, parent_channel_id, origin_message_id, name,
            auto_named, visibility, subject_kind, subject_id, anchor
        )
        VALUES (
            $1, $2, (SELECT organization_id FROM projects WHERE id = $2), $3, $4, $5, $6, $7,
            $8, $9, $10, $11
        )
        ON CONFLICT (subject_kind, subject_id) WHERE kind = 'comments' DO NOTHING
        "#,
    )
    .bind(row.id)
    .bind(row.project_id)
    .bind(row.kind)
    .bind(row.parent_id)
    .bind(row.origin_message_id)
    .bind(row.name)
    .bind(row.auto_named)
    .bind(row.visibility)
    .bind(row.subject.map(|(kind, _)| kind))
    .bind(row.subject.map(|(_, id)| id))
    .bind(row.anchor)
    .execute(&mut **transaction)
    .await
}

/// The name every comments conversation has; the thing names the place.
pub(crate) const COMMENTS_NAME: &str = "Comments";

/// A thing's comments conversation in `project_id`, made if it doesn't exist
/// yet (C1.3). Two first comments race safely: the unique index lets one
/// insert, and the other reads it. The caller has checked the creator may
/// see and comment on the thing (`discussion::open_for`). Returns the id and
/// whether this call made it.
pub(crate) async fn open_comments(
    transaction: &mut Transaction<'_, Postgres>,
    creator: &ThreadCreator,
    project_id: Uuid,
    subject_kind: &str,
    subject_id: Uuid,
) -> Result<(Uuid, bool), AppError> {
    if let Some(id) = comments_of(transaction, subject_kind, subject_id).await? {
        return Ok((id, false));
    }
    let id = Uuid::now_v7();
    let inserted = insert(
        transaction,
        NewConversation {
            id,
            project_id,
            kind: "comments",
            parent_id: None,
            origin_message_id: None,
            name: COMMENTS_NAME,
            auto_named: false,
            visibility: "subject",
            subject: Some((subject_kind, subject_id)),
            anchor: None,
        },
    )
    .await?;
    if inserted.rows_affected() == 0 {
        return comments_of(transaction, subject_kind, subject_id)
            .await?
            .map(|id| (id, false))
            .ok_or(AppError::Internal("comments conversation vanished"));
    }
    enqueue_event(
        transaction,
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: id,
            kind: OutboxConversationKind::Comments,
            version: 1,
        }),
    )
    .await?;
    audit(
        transaction,
        creator,
        "conversation.comments_opened",
        project_id,
        id,
        serde_json::json!({ "subject_kind": subject_kind, "subject_id": subject_id }),
    )
    .await?;
    Ok((id, true))
}

/// The thing's comments conversation, if it has one.
pub(crate) async fn comments_of(
    connection: &mut sqlx::PgConnection,
    subject_kind: &str,
    subject_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        -- visibility: callers check the subject, which is the rule here
        SELECT id FROM conversations
        WHERE kind = 'comments' AND subject_kind = $1 AND subject_id = $2
        "#,
    )
    .bind(subject_kind)
    .bind(subject_id)
    .fetch_optional(&mut *connection)
    .await?)
}
