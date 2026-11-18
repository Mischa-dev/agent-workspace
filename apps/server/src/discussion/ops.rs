//! Discussion as operations (C1.3): `comment_create` posts a comment on a
//! thing (its stream, a thread in it, or a new thread anchored to a place in
//! it), `comment_list` lists a thing's discussion, and
//! `comment_thread_update` resolves or reopens a thread there. The web panel
//! (`apps/web/src/discussion/`), agents and apps run the same ones.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{CommentBody, CommentPlace, SubjectKind};
use crate::{
    conversations::{ThreadPatch, threads::ThreadCreator},
    error::AppError,
    idempotency::IdempotencyKey,
    ops::{
        Edge, Idempotency, OpCall, OpContext, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<CommentCreate>()
        .add::<CommentList>()
        .add::<CommentThreadUpdate>();
}

/// The thing a discussion is about.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingInput {
    /// What kind of thing: page, sheet, whiteboard, record or task.
    pub kind: SubjectKind,
    pub id: IdRef,
}

impl ThingInput {
    fn target(&self) -> Target {
        Target::thing(self.kind.as_str(), self.id.id())
    }
}

/// Who is commenting, and through which edge, for the audit rows of what
/// the comment opens.
pub(crate) fn creator(ctx: &OpContext) -> ThreadCreator {
    let (session_id, channel, client_name, credential, grant, class) = match &ctx.edge {
        Edge::Web { session_id } => (
            Some(*session_id),
            Some("browser"),
            None,
            None,
            None,
            "password_session",
        ),
        Edge::Mcp {
            credential_id,
            oauth_grant_id,
            client_name,
            authentication_class,
            ..
        }
        | Edge::PublicApi {
            credential_id,
            oauth_grant_id,
            client_name,
            authentication_class,
            ..
        } => (
            None,
            Some("mcp"),
            Some(client_name.chars().take(80).collect()),
            *credential_id,
            *oauth_grant_id,
            *authentication_class,
        ),
        _ => (None, None, None, None, None, "background_job"),
    };
    ThreadCreator {
        actor_id: ctx.caller.account_id,
        actor_kind: ctx.caller.kind,
        project_id: ctx.caller.confined_space,
        session_id,
        authentication_class: class,
        channel,
        client_name,
        mcp_credential_id: credential,
        mcp_oauth_grant_id: grant,
        external_agent_connection_id: ctx.caller.connection_id,
        request_id: ctx.request_id.clone(),
    }
}

/// Comment on a thing.
pub struct CommentCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentCreateInput {
    pub thing: ThingInput,
    /// What to say; `@handle` mentions someone (a mentioned agent is asked
    /// to reply). Or pass `content` as the web composer writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// Reply in this thread of the thing's discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<IdRef>,
    /// Start a new thread about this place in the thing: a quote in a page
    /// ({kind: text, blockId, quote}), a cell or range in a sheet ({kind:
    /// cell, tabId, row, column} or {kind: range, tabId, from: [row,
    /// column], to}), elements on a board ({kind: elements, elementIds, at:
    /// {x, y}}) or a field ({kind: field, fieldId}).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Value>,
    /// Start a new thread with no anchor (about the whole thing).
    #[serde(default)]
    pub new_thread: bool,
    /// Quote this earlier comment in the same place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<IdRef>,
    /// Files uploaded to this discussion to post with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<IdRef>,
}

impl Targeted for CommentCreateInput {
    fn target(&self) -> Target {
        self.thing.target()
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentPosted {
    #[schemars(with = "String")]
    pub message_id: Uuid,
    /// The thing's comments conversation (read it with conversation_read).
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    /// The thread it went in, when it went in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub thread_id: Option<Uuid>,
    /// Where the thing opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl Operation for CommentCreate {
    const NAME: &'static str = "comment_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Comment";
    const DESCRIPTION: &'static str = "Comment on a sheet, whiteboard or record: in its comment \
        stream, in a thread of its discussion (threadId), or in a new thread about a place in it \
        (anchor: a cell or range, board elements, a field) or about the whole thing (newThread). \
        Whoever can see the thing sees its comments. Mentioned agents are asked to reply there. \
        Comment on tasks with task_comment and on pages with page_comment for now.";
    const EMITS: &'static [&'static str] = &["message.created"];

    type Input = CommentCreateInput;
    type Output = CommentPosted;

    async fn run(
        call: &mut OpCall<'_>,
        input: CommentCreateInput,
    ) -> Result<CommentPosted, OpError> {
        let body = match (input.text, input.content) {
            (Some(text), None) if !text.trim().is_empty() => CommentBody::Text(text),
            (None, Some(content)) => CommentBody::Content(content),
            _ => {
                return Err(OpError::invalid(
                    "text",
                    "Say something: pass text, or content as the composer writes it.",
                ));
            }
        };
        let place = match (input.thread_id, input.anchor, input.new_thread) {
            (Some(thread), None, false) => CommentPlace::Thread(thread.id()),
            (None, anchor @ Some(_), _) => CommentPlace::NewThread { anchor },
            (None, None, true) => CommentPlace::NewThread { anchor: None },
            (None, None, false) => CommentPlace::Stream,
            _ => {
                return Err(OpError::invalid(
                    "threadId",
                    "Reply in a thread (threadId) or start one (anchor or newThread), not both.",
                ));
            }
        };
        // Without a request id the call runs once (R52); a fresh key keeps
        // the pipeline's own idempotency out of the way.
        let key = match call.ctx.request_id.as_deref() {
            Some(key) => IdempotencyKey::parse(key),
            None => IdempotencyKey::parse(&Uuid::now_v7().to_string()),
        }
        .map_err(|_| OpError::invalid("clientRequestId", "Pass a short request id."))?;
        let attachments = input
            .attachment_ids
            .iter()
            .map(|id| id.id())
            .collect::<Vec<_>>();
        let posted = super::post(
            call.state,
            call.ctx.into(),
            creator(call.ctx),
            input.thing.kind,
            input.thing.id.id(),
            place,
            key,
            body,
            &attachments,
            input.reply_to_message_id.map(|id| id.id()),
        )
        .await?;
        call.mark_replayed(posted.replayed);
        call.audit_target(
            "message",
            posted.message_id,
            json!({
                "subjectKind": input.thing.kind.as_str(),
                "subjectId": input.thing.id.id(),
                "threadId": posted.thread_id,
            }),
        );
        Ok(CommentPosted {
            message_id: posted.message_id,
            conversation_id: posted.conversation_id,
            thread_id: posted.thread_id,
            href: posted.href,
        })
    }
}

/// A thing's discussion: its comment stream and threads.
pub struct CommentList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentListInput {
    pub thing: ThingInput,
    /// Only threads with this status, such as Open or Resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl Targeted for CommentListInput {
    fn target(&self) -> Target {
        self.thing.target()
    }
}

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CommentThreadView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Where on the thing it is about; none for the whole thing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Value>,
    pub message_count: i64,
    /// Messages here you haven't seen.
    pub unread: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentListOutput {
    /// The thing's comments conversation; none until someone comments.
    /// Read its messages with conversation_read.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub conversation_id: Option<Uuid>,
    /// The space the comments live in (for mentions); none for a board
    /// outside any project until someone comments.
    #[schemars(with = "Option<String>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Uuid>,
    /// Comments in the stream (not in threads).
    pub message_count: i64,
    pub unread: bool,
    /// Its threads, oldest first.
    pub threads: Vec<CommentThreadView>,
    /// False when the thing takes no new comments here (archived, or a
    /// task or page, whose comments move here later).
    pub accepts_comments: bool,
}

const MAX_THREADS: i64 = 200;

impl Operation for CommentList {
    const NAME: &'static str = "comment_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List comments";
    const DESCRIPTION: &'static str = "A sheet's, whiteboard's or record's discussion: its \
        comments conversation (read the comments with conversation_read), how many comments \
        and whether any are unread, and its threads with their status, anchor and counts. \
        Filter threads by status, such as Open.";

    type Input = CommentListInput;
    type Output = CommentListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: CommentListInput,
    ) -> Result<CommentListOutput, OpError> {
        let viewer = call.account_id();
        // An agent reaches the spaces its connection allows.
        let reach = call.ctx.caller.connection_id;
        let kind = input.thing.kind;
        let thing_id = input.thing.id.id();
        let status = input
            .status
            .map(|status| status.trim().to_owned())
            .filter(|status| !status.is_empty());
        let connection = &mut **call.tx().await?;
        let subject = kind.subject();
        let home = subject
            .home(connection, thing_id, viewer)
            .await?
            .ok_or(OpError::not_found("That thing"))?;
        let accepts_comments = subject.open && home.accepts_comments;
        let Some(conversation_id) =
            crate::conversations::threads::comments_of(connection, kind.as_str(), thing_id).await?
        else {
            return Ok(CommentListOutput {
                conversation_id: None,
                space_id: home.project_id,
                message_count: 0,
                unread: false,
                threads: Vec::new(),
                accepts_comments,
            });
        };
        let (space_id, message_count, unread) = sqlx::query_as::<_, (Uuid, i64, bool)>(
            r#"
            SELECT conversation.project_id, conversation.next_entry_sequence - 1,
                   conversation.last_message_sequence
                       > COALESCE(read_state.highest_seen_sequence, 0)
            FROM conversations conversation
            LEFT JOIN conversation_read_states read_state
              ON read_state.conversation_id = conversation.id AND read_state.actor_id = $2
            WHERE conversation.id = $1
              AND actor_can_access_project($2, $3, conversation.project_id)
              AND conversation_visible_to(conversation.id, $2)
            "#,
        )
        .bind(conversation_id)
        .bind(viewer)
        .bind(reach)
        .fetch_optional(&mut *connection)
        .await
        .map_err(AppError::from)?
        .ok_or(OpError::not_found("That thing"))?;
        let threads = sqlx::query_as::<_, CommentThreadView>(
            r#"
            SELECT thread.id, thread.name, thread.thread_status AS status,
                   thread.thread_outcome AS outcome, thread.anchor,
                   thread.next_entry_sequence - 1 AS message_count,
                   thread.last_message_sequence
                       > COALESCE(read_state.highest_seen_sequence, 0) AS unread,
                   to_char(thread.created_at AT TIME ZONE 'UTC',
                           'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
            FROM conversations thread
            LEFT JOIN conversation_read_states read_state
              ON read_state.conversation_id = thread.id AND read_state.actor_id = $2
            WHERE thread.parent_channel_id = $1 AND thread.kind = 'thread'
              AND thread.archived_at IS NULL
              AND ($3::text IS NULL OR lower(thread.thread_status) = lower($3))
              AND conversation_visible_to(thread.id, $2)
            ORDER BY thread.created_at, thread.id
            LIMIT $4
            "#,
        )
        .bind(conversation_id)
        .bind(viewer)
        .bind(status)
        .bind(MAX_THREADS)
        .fetch_all(&mut *connection)
        .await
        .map_err(AppError::from)?;
        Ok(CommentListOutput {
            conversation_id: Some(conversation_id),
            space_id: Some(space_id),
            message_count,
            unread,
            threads,
            accepts_comments,
        })
    }
}

/// Resolve or reopen a thread in a thing's discussion.
pub struct CommentThreadUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentThreadUpdateInput {
    pub thread_id: IdRef,
    /// Its status, such as Open or Resolved.
    pub status: String,
    /// What it came to, shown with it; an empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl Targeted for CommentThreadUpdateInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.thread_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentThreadUpdated {
    #[schemars(with = "String")]
    pub thread_id: Uuid,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl Operation for CommentThreadUpdate {
    const NAME: &'static str = "comment_thread_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Resolve comment thread";
    const DESCRIPTION: &'static str = "Set the status of a thread in a thing's discussion, such as \
        Resolved when it's dealt with or Open to reopen it, and optionally what it came to \
        (outcome). Threads in channels and DMs use thread_update.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = CommentThreadUpdateInput;
    type Output = CommentThreadUpdated;

    async fn run(
        call: &mut OpCall<'_>,
        input: CommentThreadUpdateInput,
    ) -> Result<CommentThreadUpdated, OpError> {
        let actor_id = call.account_id();
        let actor_kind = call.ctx.caller.kind;
        let reach = call.ctx.caller.connection_id;
        let thread_id = input.thread_id.id();
        let status = input.status.trim().to_owned();
        if status.is_empty() {
            return Err(OpError::invalid(
                "status",
                "Say its status, such as Resolved.",
            ));
        }
        let patch = ThreadPatch {
            name: None,
            status: Some(status),
            clear_outcome: input
                .outcome
                .as_deref()
                .is_some_and(|outcome| outcome.trim().is_empty()),
            outcome: input.outcome.filter(|outcome| !outcome.trim().is_empty()),
        };
        let tx = call.tx().await?;
        let in_comments = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations thread
                JOIN conversations parent ON parent.id = thread.parent_channel_id
                WHERE thread.id = $1 AND thread.kind = 'thread' AND parent.kind = 'comments'
                  AND actor_can_access_project($2, $3, thread.project_id)
                  AND conversation_visible_to(thread.id, $2)
            )
            "#,
        )
        .bind(thread_id)
        .bind(actor_id)
        .bind(reach)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if !in_comments {
            return Err(OpError::not_found("That thread"));
        }
        let updated =
            crate::conversations::update_thread(tx, actor_id, actor_kind, thread_id, &patch)
                .await?;
        call.audit_target(
            "conversation",
            thread_id,
            json!({ "status": updated.thread_status }),
        );
        Ok(CommentThreadUpdated {
            thread_id,
            status: updated.thread_status.unwrap_or_default(),
            outcome: updated.thread_outcome,
        })
    }
}
