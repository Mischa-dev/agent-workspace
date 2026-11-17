//! A1.8: requests as operations. Anyone may ask people, agents or a role
//! something (`request_create`); whoever it is for answers (`request_respond`);
//! its raiser withdraws it (`request_cancel`); and `request_get` and
//! `request_list` read them. A run raises its requests through its
//! `interrupt` activities instead; they are answered the same way.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{AnswerError, OpenError, Origin, RequestRow, RequestView};
use crate::ops::{
    ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
    Targeted, refs::IdRef,
};

const REQUEST: &str = "That request";

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<RequestOpen>()
        .add::<RequestAnswer>()
        .add::<RequestCancel>()
        .add::<RequestGet>()
        .add::<RequestList>();
}

impl From<AnswerError> for OpError {
    fn from(error: AnswerError) -> Self {
        match error {
            AnswerError::Invalid { field, message } => Self::invalid(field, message),
            AnswerError::Settled(message) => Self::Conflict {
                reason: ConflictReason::Transition,
                message: message.into(),
                current: None,
            },
            AnswerError::NotYours => Self::invalid(
                "requestId",
                "This request isn't yours to answer: it asks someone else, or a role you don't hold.",
            ),
            AnswerError::App(error) => error.into(),
        }
    }
}

impl From<OpenError> for OpError {
    fn from(error: OpenError) -> Self {
        match error {
            OpenError::Refused { field, message } => Self::Invalid {
                field: field.into(),
                message: message.into(),
            },
            OpenError::App(error) => error.into(),
        }
    }
}

// ------------------------------------------------------------------ open

/// Ask something.
pub struct RequestOpen;

/// One option to pick.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestOptionInput {
    /// A short id the answer names, such as "yes".
    pub id: String,
    /// What the option says, up to 200 characters.
    pub label: String,
    /// approve or deny, when the option says yes or no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// Who a request is for.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudienceInput {
    /// People and agents in the project, by id or @handle (at most 20).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<String>,
    /// Roles by key, such as owner or admin (at most 10): whoever holds one
    /// and may answer requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestOpenInput {
    /// The space (project): its id or link. Not needed with conversationId.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// question (the default), approval or proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What you need, up to 2,000 characters, so it can be answered from
    /// the card alone.
    pub title: String,
    /// More context, up to 8,000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Up to 8 answers to pick from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<RequestOptionInput>,
    /// Whether a typed answer is taken (default: yes for a question, or
    /// when there are no options).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_text: Option<bool>,
    /// Who it's for: accounts, or roles.
    pub audience: AudienceInput,
    /// The conversation it's about; the people asked must see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<IdRef>,
    /// The task it's about: its id, link or key (such as AWD-12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Seconds until it lapses: 60 to 604,800 (a week).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<i64>,
    /// What happens then: expire (the default) or deny (picks the option
    /// styled deny).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_expiry: Option<String>,
}

impl Targeted for RequestOpenInput {
    fn target(&self) -> Target {
        match (self.space_id, self.conversation_id) {
            (Some(space), _) => Target::Space(space.id()),
            (None, Some(conversation)) => Target::thing("conversation", conversation.id()),
            (None, None) => Target::Workspace(None),
        }
    }
}

/// An account in the space by id or @handle.
async fn account_in_space(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    space_id: Uuid,
    reference: &str,
) -> Result<Option<Uuid>, OpError> {
    let reference = reference.trim();
    let id = crate::ops::refs::parse_id(reference);
    let handle = reference.trim_start_matches('@').to_lowercase();
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor_id FROM project_memberships
        WHERE project_id = $1 AND enabled
          AND (actor_id = $2 OR ($2 IS NULL AND lower(handle) = $3))
        "#,
    )
    .bind(space_id)
    .bind(id)
    .bind(handle)
    .fetch_optional(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?)
}

/// A task in the space by id, link or key.
async fn task_in_space(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    space_id: Uuid,
    reference: &str,
) -> Result<Option<Uuid>, OpError> {
    let reference = reference.trim();
    let id = crate::ops::refs::parse_id(reference);
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT item.id FROM task_rows item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1
          AND (item.id = $2
               OR ($2 IS NULL AND upper(project.project_key || '-' || item.number::text) = upper($3)))
        "#,
    )
    .bind(space_id)
    .bind(id)
    .bind(reference)
    .fetch_optional(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?)
}

impl Operation for RequestOpen {
    const NAME: &'static str = "request_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RequestsRaise);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["request.raised"];
    const TITLE: &'static str = "Ask someone something";
    const DESCRIPTION: &'static str = "Ask people, agents or a role in a project something, on a card in their inbox: a question, an approval or a proposal, with up to 8 options to pick from and optionally a typed answer. Name who it's for in audience (accounts by id or @handle, or roles by key). When it is answered you get a request.answered event and an inbox card; read the answer with request_get. Withdraw it with request_cancel.";
    type Input = RequestOpenInput;
    type Output = RequestView;

    async fn run(call: &mut OpCall<'_>, input: RequestOpenInput) -> Result<RequestView, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::invalid(
            "spaceId",
            "Say which project to ask in (spaceId or conversationId).",
        ))?;
        let workspace_id = call.ctx.workspace_id;
        let account_id = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let mut accounts = Vec::with_capacity(input.audience.accounts.len());
        for reference in input.audience.accounts.iter().take(21) {
            let account =
                account_in_space(tx, space_id, reference)
                    .await?
                    .ok_or(OpError::invalid(
                        "audience.accounts",
                        "Name people and agents in this project, by id or @handle.",
                    ))?;
            accounts.push(json!(account));
        }
        let conversation_id = match input.conversation_id {
            Some(conversation) => {
                // -- visibility: the raiser must see it, in this space.
                let found = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM conversations WHERE id = $1 AND project_id = $2 \
                     AND conversation_visible_to(id, $3)",
                )
                .bind(conversation.id())
                .bind(space_id)
                .bind(account_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?;
                Some(found.ok_or(OpError::not_found("That conversation"))?)
            }
            None => None,
        };
        let work_item_id = match input.task_id.as_deref() {
            Some(task) => Some(
                task_in_space(tx, space_id, task)
                    .await?
                    .ok_or(OpError::not_found("That task"))?,
            ),
            None => None,
        };
        let request = json!({
            "kind": input.kind,
            "title": input.title,
            "detail": input.detail,
            "options": input.options,
            "allowText": input.allow_text,
            "audience": {"accounts": accounts, "roles": input.audience.roles},
            "expiresInSeconds": input.expires_in_seconds,
            "onExpiry": input.on_expiry,
        });
        let raised = super::open(
            tx,
            Origin {
                account_id,
                workspace_id,
                space_id: Some(space_id),
                run_id: None,
                host_request_ref: None,
                conversation_id,
                work_item_id,
                subject: None,
                field: "request",
            },
            &request,
        )
        .await?;
        // The people asked must see what it is about.
        let row = super::get(tx, raised.request_id)
            .await?
            .ok_or(OpError::Unavailable)?;
        for account in &row.addressed_account_ids {
            let sees = sqlx::query_scalar::<_, bool>("SELECT request_visible_to($1, $2)")
                .bind(row.id)
                .bind(account)
                .fetch_one(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?;
            if !sees {
                return Err(OpError::invalid(
                    "audience.accounts",
                    "Someone you're asking can't see the conversation it's about.",
                ));
            }
        }
        super::announce_raised(tx, row.id).await?;
        call.audit_target("request", row.id, json!({"kind": row.kind}));
        Ok(RequestView::from(&row))
    }
}

// ---------------------------------------------------------------- answer

/// Answer a request.
pub struct RequestAnswer;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAnswerInput {
    /// The request: its id or link.
    pub request_id: IdRef,
    /// The option you pick, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// A typed answer, up to 4,000 characters, when the request takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Targeted for RequestAnswerInput {
    fn target(&self) -> Target {
        Target::thing("request", self.request_id.id())
    }
}

impl Operation for RequestAnswer {
    const NAME: &'static str = "request_respond";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["request.answered"];
    const TITLE: &'static str = "Answer a request";
    const DESCRIPTION: &'static str = "Answer a request asked of you or a role you hold (request_list for_me lists them): pick an option by its id, type an answer, or both. An answer is final; it goes back to whoever asked, and a run waiting on it goes on.";
    type Input = RequestAnswerInput;
    type Output = RequestView;

    async fn run(call: &mut OpCall<'_>, input: RequestAnswerInput) -> Result<RequestView, OpError> {
        let caller = call.ctx.caller.clone();
        let state = call.state;
        let space = call.ctx.space_id;
        let tx = call.tx().await?;
        // An answer can set a run going again: a suspended or closing
        // workspace answers nothing, and a suspension waits for this one
        // (the workspace is held before the request's row is locked).
        if let Some(space) = space {
            crate::workspace::lifecycle::hold_open_for_space(tx, space, "request_respond").await?;
        }
        let row = super::visible(tx, input.request_id.id(), caller.account_id, true)
            .await?
            .ok_or(OpError::not_found(REQUEST))?;
        let row = super::answer(
            tx,
            state,
            &caller,
            &row,
            input.option_id.as_deref(),
            input.text.as_deref(),
        )
        .await?;
        // An approved run request queued its run (`external_execution`).
        if row.subject_kind.as_deref() == Some(crate::external_execution::REQUEST_SUBJECT) {
            call.start_queued_runs();
        }
        call.audit_target("request", row.id, json!({"status": row.status}));
        Ok(RequestView::from(&row))
    }
}

// ---------------------------------------------------------------- cancel

/// Withdraw a request you raised.
pub struct RequestCancel;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestCancelInput {
    /// The request: its id or link.
    pub request_id: IdRef,
}

impl Targeted for RequestCancelInput {
    fn target(&self) -> Target {
        Target::thing("request", self.request_id.id())
    }
}

impl Operation for RequestCancel {
    const NAME: &'static str = "request_cancel";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["request.cancelled"];
    const TITLE: &'static str = "Withdraw a request";
    const DESCRIPTION: &'static str = "Withdraw a request you raised that nobody has answered yet; its cards go away. Withdrawing one already settled changes nothing.";
    type Input = RequestCancelInput;
    type Output = RequestView;

    async fn run(call: &mut OpCall<'_>, input: RequestCancelInput) -> Result<RequestView, OpError> {
        let account_id = call.ctx.caller.account_id;
        let state = call.state;
        let tx = call.tx().await?;
        let row = super::visible(tx, input.request_id.id(), account_id, true)
            .await?
            .ok_or(OpError::not_found(REQUEST))?;
        let row =
            super::cancel(tx, state, account_id, &row)
                .await
                .map_err(|error| match error {
                    AnswerError::NotYours => {
                        OpError::invalid("requestId", "Only whoever raised a request withdraws it.")
                    }
                    other => other.into(),
                })?;
        call.audit_target("request", row.id, json!({"status": row.status}));
        Ok(RequestView::from(&row))
    }
}

// ------------------------------------------------------------------ reads

/// One request.
pub struct RequestGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestGetInput {
    /// The request: its id or link.
    pub request_id: IdRef,
}

impl Targeted for RequestGetInput {
    fn target(&self) -> Target {
        Target::thing("request", self.request_id.id())
    }
}

/// A request and whether you may answer it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestDetail {
    pub request: RequestView,
    /// Whether request_respond would take your answer now.
    pub may_answer: bool,
}

impl Operation for RequestGet {
    const NAME: &'static str = "request_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read a request";
    const DESCRIPTION: &'static str = "Read one request: what it asks, its options, who it is for, and its answer once given. Says whether you may answer it.";
    type Input = RequestGetInput;
    type Output = RequestDetail;

    async fn run(call: &mut OpCall<'_>, input: RequestGetInput) -> Result<RequestDetail, OpError> {
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let row = super::visible(tx, input.request_id.id(), caller.account_id, false)
            .await?
            .ok_or(OpError::not_found(REQUEST))?;
        let may_answer = super::may_answer(tx, &caller, &row).await?;
        Ok(RequestDetail {
            request: RequestView::from(&row),
            may_answer,
        })
    }
}

/// Requests for you, or that you raised.
pub struct RequestList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestListInput {
    /// for_me (the default: open requests you may answer) or raised (ones
    /// you asked, with their answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// For raised: open, answered, or all (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Only this space (project).
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// How many, newest first (default 20, at most 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for RequestListInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestListOutput {
    pub requests: Vec<RequestView>,
}

impl Operation for RequestList {
    const NAME: &'static str = "request_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List requests";
    const DESCRIPTION: &'static str = "List open requests you may answer (role for_me, the default), or requests you raised with their answers (role raised; filter by status open, answered or all). Answer with request_respond.";
    type Input = RequestListInput;
    type Output = RequestListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RequestListInput,
    ) -> Result<RequestListOutput, OpError> {
        let caller = call.ctx.caller.clone();
        let workspace_id = call.ctx.workspace_id;
        let space_id = call.ctx.space_id;
        let limit = i64::from(input.limit.unwrap_or(20).clamp(1, 50));
        let raised = match input.role.as_deref() {
            None | Some("for_me") => false,
            Some("raised") => true,
            Some(_) => return Err(OpError::invalid("role", "role is for_me or raised.")),
        };
        let status = match input.status.as_deref() {
            None | Some("all") => None,
            Some(status @ ("open" | "answered")) => Some(status.to_owned()),
            Some(_) => {
                return Err(OpError::invalid(
                    "status",
                    "status is open, answered or all.",
                ));
            }
        };
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<_, RequestRow>(&format!(
            r#"
            SELECT {columns} FROM requests request
            WHERE request.workspace_id = $1
              AND ($2::uuid IS NULL OR request.space_id = $2)
              AND CASE WHEN $4 THEN request.raised_by_account_id = $3
                       ELSE request.raised_by_account_id <> $3 AND request.status = 'open' END
              AND ($5::text IS NULL OR request.status = $5)
              AND request_visible_to(request.id, $3)
            ORDER BY request.created_at DESC, request.id DESC
            LIMIT $6
            "#,
            columns = super::REQUEST_COLUMNS
        ))
        .bind(workspace_id)
        .bind(space_id)
        .bind(caller.account_id)
        .bind(raised)
        .bind(&status)
        // For me: read more, then keep the ones this caller may answer.
        .bind(if raised { limit } else { 200 })
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let mut requests = Vec::new();
        for row in rows {
            if requests.len() as i64 >= limit {
                break;
            }
            if raised || super::may_answer(tx, &caller, &row).await? {
                requests.push(RequestView::from(&row));
            }
        }
        Ok(RequestListOutput { requests })
    }
}
