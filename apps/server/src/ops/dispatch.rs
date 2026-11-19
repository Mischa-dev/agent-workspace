//! The pipeline every call goes through, whichever edge it came in by:
//! find, decode, target, locate, module gate, access, transaction (tenancy
//! set), request id reserved, run, audit row, request id completed, commit.
//! An edge builds a `DispatchRequest` and sends the answer back in its own
//! framing; nothing here knows HTTP or JSON-RPC beyond the status.

use axum::http::StatusCode;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    Caller, Edge, Envelope, Idempotency, OpCall, OpContext, OpError, Operation,
    access_shim::{self},
    error::DenyReason,
    registry,
    target::{self, Target, Targeted},
};
use crate::{
    AppState,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
};

/// A call as an edge hands it over.
#[derive(Clone, Debug)]
pub struct DispatchRequest {
    pub caller: Caller,
    pub edge: Edge,
    /// `x-request-id`, for logs and the audit row's correlation id.
    pub trace_id: Option<String>,
    /// The workspace the call names (its path, or a `workspace` argument).
    pub workspace: Option<Uuid>,
    /// The caller's request id (`Idempotency-Key`, `clientRequestId`).
    pub request_id: Option<String>,
    /// What a write without a request id does (ruling R52).
    pub unkeyed: Unkeyed,
    /// Run and roll back (operations with `PREVIEW`).
    pub preview: bool,
    /// The old name the caller used (`aliases`), counted once the call is done.
    pub renamed_from: Option<&'static str>,
    /// A long call (`Operation::runs_long`) is answered `started` and run by
    /// a job (`long`): the MCP edge, whose clients can follow it.
    pub defer_long: bool,
}

/// What a write that arrives without a request id does (ruling R52). An
/// id is never inferred from equal input or the clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unkeyed {
    /// Refused with a validation error naming the edge's field: the web
    /// app and the v2 route (`Idempotency-Key`), and automations, which
    /// derive theirs from their run and step.
    Refuse(&'static str),
    /// Runs once and is not retry-safe; the answer says `replaySafe: false`
    /// (MCP and the v1 routes, whose callers were never asked for one).
    RunOnce,
}

/// What an edge sends back: the status its outcome maps to and the
/// envelope.
#[derive(Clone, Debug)]
pub struct Answer {
    pub status: StatusCode,
    pub body: Value,
}

impl Answer {
    pub fn error(error: &OpError) -> Self {
        Self {
            status: error.status(),
            body: error.to_json(),
        }
    }

    pub fn outcome(&self) -> &str {
        self.body["outcome"].as_str().unwrap_or_default()
    }

    pub fn is_ok(&self) -> bool {
        self.outcome() == "ok"
    }
}

/// Run the operation called `name`.
pub async fn dispatch(
    state: &AppState,
    request: DispatchRequest,
    name: &str,
    input: Value,
) -> Answer {
    let Some(call) = registry().get(name).map(|entry| entry.call) else {
        return Answer::error(&OpError::not_found("That operation"));
    };
    let (request, input) = match asked_for(state, request, input).await {
        Ok(asked) => asked,
        Err(error) => return Answer::error(&error),
    };
    // K1.25: every write the operation makes says which credential it came
    // through, and for a run's token, the run and who asked for it.
    let acting = crate::access::acting::Acting::of(&request.caller);
    crate::access::acting::within(acting, call(state, request, input)).await
}

/// A2.15: any operation may say what it acts on (`context`), and is then
/// made for that one's asker (`replies::asker`). A run's credential already
/// says who asked, which `context` doesn't change.
async fn asked_for(
    state: &AppState,
    mut request: DispatchRequest,
    mut input: Value,
) -> Result<(DispatchRequest, Value), OpError> {
    let Some(context) = input
        .as_object_mut()
        .and_then(|fields| fields.remove("context"))
        .filter(|context| !context.is_null())
    else {
        return Ok((request, input));
    };
    let mut connection = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(|error| OpError::logged("context", &error.into()))?;
    let asker =
        crate::replies::asker::resolve(&mut connection, request.caller.account_id, context).await?;
    connection
        .commit()
        .await
        .map_err(|error| OpError::logged("context", &error.into()))?;
    if request.caller.requested_by.is_none() {
        request.caller.requested_by = asker;
    }
    Ok((request, input))
}

/// A space's key (such as "AWD") in the input's `spaceId` or `projectId`
/// (or a package's `target.spaceId`), replaced by its id, on every edge (MCP's own key resolution does the
/// same for its tools). A key resolves only among spaces the caller is an
/// enabled member of and can reach, in the named workspace when there is
/// one; an unknown or ambiguous key is left for the input to refuse.
async fn space_keys_to_ids(
    state: &AppState,
    request: &DispatchRequest,
    mut input: Value,
) -> Result<Value, OpError> {
    for field in ["/spaceId", "/projectId", "/target/spaceId"] {
        // K4.9: "me" is the caller's personal space in the named workspace,
        // made on first use; a credential held to one space never reaches it.
        if input.pointer(field).and_then(Value::as_str) == Some("me") {
            if let (Some(workspace), None) = (request.workspace, request.caller.confined_space) {
                let mut tx = crate::db::scoped_tx(&state.pool)
                    .await
                    .map_err(|error| OpError::logged("personal space", &error.into()))?;
                let space = crate::spaces::personal::personal(
                    &mut tx,
                    workspace,
                    request.caller.account_id,
                )
                .await
                .map_err(|error| OpError::logged("personal space", &error))?;
                tx.commit()
                    .await
                    .map_err(|error| OpError::logged("personal space", &error.into()))?;
                if let (Some(space), Some(slot)) = (space, input.pointer_mut(field)) {
                    *slot = Value::String(space.to_string());
                }
            }
            continue;
        }
        let Some(key) = input
            .pointer(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| {
                (2..=12).contains(&key.len())
                    && key.starts_with(|c: char| c.is_ascii_alphabetic())
                    && key.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .map(str::to_owned)
        else {
            continue;
        };
        let found = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT project.id
            FROM projects project
            JOIN project_memberships membership
              ON membership.project_id = project.id
             AND membership.actor_id = $1
             AND membership.enabled
            WHERE upper(project.project_key) = upper($3)
              AND project.archived_at IS NULL
              AND actor_can_access_project($1, $2, project.id)
              AND ($4::uuid IS NULL OR project.id = $4)
              AND ($5::uuid IS NULL OR project.organization_id = $5)
            LIMIT 2
            "#,
        )
        .bind(request.caller.account_id)
        .bind(request.caller.connection_id)
        .bind(&key)
        .bind(request.caller.confined_space)
        .bind(request.workspace)
        .fetch_all(&state.pool)
        .await
        .map_err(|error| OpError::logged("space key", &error.into()))?;
        if let ([id], Some(slot)) = (found.as_slice(), input.pointer_mut(field)) {
            *slot = Value::String(id.to_string());
        }
    }
    task_keys_to_ids(state, request, input).await
}

/// A task's key (such as "AWD-12") in a field that names a task, replaced
/// by the task's id (K2.13), the way MCP's own key resolution does for its
/// tools: the task that has the key now, or the one a moved task went to,
/// among spaces the caller is an enabled member of and can reach. A key two
/// of the caller's workspaces share is never guessed; it stays for the
/// input to refuse.
async fn task_keys_to_ids(
    state: &AppState,
    request: &DispatchRequest,
    mut input: Value,
) -> Result<Value, OpError> {
    // K2.14: a bulk change's first task too, which says where it acts (the
    // command resolves the rest).
    for field in [
        "/taskId",
        "/workItemId",
        "/parentId",
        "/parentWorkItemId",
        "/taskIds/0",
        "/links/0/taskId",
    ] {
        let Some((space_key, number)) = input
            .pointer(field)
            .and_then(Value::as_str)
            .and_then(crate::mcp::split_task_key)
            .map(|(key, number)| (key.to_owned(), number))
        else {
            continue;
        };
        let caller = &request.caller;
        let workspace = request.workspace;
        let found = crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query_as::<_, (Uuid, i32)>(
                r#"
            SELECT found.id, min(found.preference) FROM (
                -- The task with the key now or by an older key, alike (two
                -- of them is ambiguous); a moved original only for someone
                -- who can't reach where it went (it is archived, and says
                -- so).
                SELECT item.id,
                       CASE WHEN EXISTS (
                           SELECT 1 FROM work_item_key_aliases moved
                           WHERE moved.source_work_item_id = item.id
                       ) THEN 2 ELSE 0 END AS preference,
                       item.project_id
                FROM task_rows item
                JOIN projects project ON project.id = item.project_id
                WHERE upper(project.project_key) = upper($3) AND item.number = $4
                  AND project.archived_at IS NULL
                UNION ALL
                SELECT alias.work_item_id, 0, item.project_id
                FROM work_item_key_aliases alias
                JOIN task_rows item ON item.id = alias.work_item_id
                WHERE upper(alias.project_key) = upper($3) AND alias.number = $4
            ) found
            JOIN projects project ON project.id = found.project_id
            JOIN project_memberships membership
              ON membership.project_id = found.project_id
             AND membership.actor_id = $1
             AND membership.enabled
            WHERE actor_can_access_project($1, $2, found.project_id)
              AND ($5::uuid IS NULL OR found.project_id = $5)
              AND ($6::uuid IS NULL OR project.organization_id = $6)
            GROUP BY found.id
            ORDER BY min(found.preference)
            LIMIT 3
            "#,
            )
            .bind(caller.account_id)
            .bind(caller.connection_id)
            .bind(&space_key)
            .bind(number)
            .bind(caller.confined_space)
            .bind(workspace)
            .fetch_all(connection)
            .await
        })
        .await
        .map_err(|error: sqlx::Error| OpError::logged("task key", &error.into()))?;
        // The best match only: two tasks tie only across workspaces.
        let best = found.first().map(|(_, preference)| *preference);
        let found = found
            .into_iter()
            .filter(|(_, preference)| Some(*preference) == best)
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        match found.as_slice() {
            [id] => {
                if let Some(slot) = input.pointer_mut(field) {
                    *slot = Value::String(id.to_string());
                }
            }
            [] => return Err(OpError::not_found("That task")),
            _ => {
                return Err(OpError::invalid(
                    "taskId",
                    "That key names tasks in more than one of your workspaces. Use the task's \
                     id or link.",
                ));
            }
        }
    }
    Ok(input)
}

/// The pipeline for one operation; `registry::entry` erases it.
pub(super) async fn run<O: Operation>(
    state: &AppState,
    request: DispatchRequest,
    input: Value,
) -> Answer {
    match pipeline::<O>(state, request, input).await {
        Ok(answer) => answer,
        Err(error) => Answer::error(&error),
    }
}

/// The request id a write is held to: the caller's, checked. A write
/// without one is refused or runs once, as its edge says (R52).
fn request_id_for<O: Operation>(
    request: &DispatchRequest,
) -> Result<Option<IdempotencyKey>, OpError> {
    if O::IDEMPOTENCY == Idempotency::None || !O::KIND.writes() {
        return Ok(None);
    }
    if let Some(given) = request.request_id.as_deref() {
        let parsed = IdempotencyKey::parse(given)
            .ok()
            .filter(|key| key.as_str().len() <= 128);
        return parsed.map(Some).ok_or_else(|| OpError::Invalid {
            field: "requestId".into(),
            message: "A request id is 1 to 128 letters, digits, '-', '_' or '.'.".into(),
        });
    }
    match request.unkeyed {
        Unkeyed::Refuse(field) => Err(OpError::Invalid {
            field: field.into(),
            message: format!(
                "A change through this route carries a request id ({field}): your own id \
                 for this change, kept the same when you retry it."
            )
            .into(),
        }),
        Unkeyed::RunOnce => Ok(None),
    }
}

/// The field a decode error names, when serde says.
fn decode_error(error: &serde_json::Error) -> OpError {
    let text = error.to_string();
    let field = text
        .split('`')
        .nth(1)
        .filter(|field| {
            !field.is_empty()
                && field
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
        .unwrap_or("input")
        .to_owned();
    // serde's text names the line and column of a JSON value; drop it.
    let message = text.split(" at line ").next().unwrap_or(&text).to_owned();
    OpError::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

/// Where a call acts, located, once the module gate and access let the
/// caller run `O` there. A call that can't name its workspace gets the one
/// the caller reaches, when there is one; a spanning read that names none
/// lists across every one (`spans`).
async fn admit<O: Operation>(
    state: &AppState,
    caller: &Caller,
    workspace: &mut Option<Uuid>,
    target: &Target,
) -> Result<Admitted, OpError> {
    let writes = O::KIND.writes();
    let mut connection = state
        .pool
        .acquire()
        .await
        .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
    // A call that can't name its workspace needs none named when the
    // caller reaches only one.
    let mut spans = false;
    if workspace.is_none() && matches!(target, Target::Account | Target::Workspace(None)) {
        let reach = access_shim::reach(&mut connection, caller).await?;
        match reach.as_slice() {
            [only] => *workspace = Some(*only),
            [] => {}
            _ if O::SPANS_WORKSPACES && !writes => spans = true,
            several => return Err(name_the_workspace(&mut connection, several).await),
        }
    }
    if spans {
        let located = target::Located {
            workspace_id: Uuid::nil(),
            space_id: None,
            direct: None,
        };
        // Across workspaces only the credential is checked here; the read
        // keeps to what the caller may see (and has on) in each one.
        let _grant = crate::access::check(
            &mut connection,
            caller,
            crate::access::Need::SEE,
            crate::access::Target::Account,
        )
        .await
        .map_err(|error| OpError::logged(O::NAME, &error))?
        .map_err(|denied| denied_error::<O>(denied, caller, &located))?;
        return Ok(Admitted {
            located,
            paused: false,
            spans,
        });
    }
    let located = target::locate(&mut connection, target, caller.account_id, *workspace).await?;
    let module = module_here::<O>(&mut connection, caller, &located).await?;
    let need = crate::access::Need::maybe(O::PERMISSION)
        .writing(writes)
        .in_module(module);
    let _grant = crate::access::check(
        &mut connection,
        caller,
        need,
        access_target(target, &located),
    )
    .await
    .map_err(|error| OpError::logged(O::NAME, &error))?
    .map_err(|denied| denied_error::<O>(denied, caller, &located))?;
    // H1.24: a suspended workspace, or one being deleted, refuses every
    // call but the ones that still work then (keeping it).
    crate::workspace::lifecycle::ensure_open(&mut connection, located.workspace_id, O::NAME)
        .await?;
    // A2.10: a paused account's changes do nothing (the caller answers ok,
    // saying so).
    let paused = writes
        && !runs_while_paused(O::NAME)
        && crate::statuses::is_paused(&mut connection, caller.account_id, located.workspace_id)
            .await
            .map_err(|error| OpError::logged(O::NAME, &error))?;
    drop(connection);
    Ok(Admitted {
        located,
        paused,
        spans,
    })
}

/// What `admit` found: where the call lands, whether the caller is paused
/// there, and whether a read spans every workspace the caller reaches.
struct Admitted {
    located: target::Located,
    paused: bool,
    spans: bool,
}

/// Whether `caller` may run `O` on `target`, by the checks a call makes,
/// without running it: for a control shown unavailable with the reason.
pub(super) async fn check<O: Operation>(
    state: &AppState,
    caller: &Caller,
    workspace: Option<Uuid>,
    target: &Target,
) -> Result<(), OpError> {
    let mut workspace = workspace;
    admit::<O>(state, caller, &mut workspace, target)
        .await
        .map(|_| ())
}

/// Whether `caller` may run the operation called `name` on `target`
/// (`check`), such as the ones a button field calls.
pub async fn may_run(
    state: &AppState,
    caller: &Caller,
    workspace: Option<Uuid>,
    name: &str,
    target: &Target,
) -> Result<(), OpError> {
    let Some(check) = registry().get(name).map(|entry| entry.check) else {
        return Err(OpError::not_found("That operation"));
    };
    check(state, caller, workspace, target).await
}

async fn pipeline<O: Operation>(
    state: &AppState,
    mut request: DispatchRequest,
    input: Value,
) -> Result<Answer, OpError> {
    if request.preview && !O::PREVIEW {
        return Err(OpError::Invalid {
            field: "preview".into(),
            message: format!("{} has no preview.", O::NAME).into(),
        });
    }
    let input = space_keys_to_ids(state, &request, input).await?;
    let decoded =
        serde_json::from_value::<O::Input>(input).map_err(|error| decode_error(&error))?;
    // The input as the operation reads it: what the request id is held to.
    let canonical = serde_json::to_value(&decoded).map_err(|_| OpError::Unavailable)?;
    let target = match decoded.target() {
        Target::Workspace(None) if O::HELD_SPACE_BY_DEFAULT => request
            .caller
            .confined_space
            .map_or(Target::Workspace(None), Target::Space),
        target => target,
    };
    let writes = O::KIND.writes();

    let Admitted {
        located,
        paused,
        spans,
    } = admit::<O>(state, &request.caller, &mut request.workspace, &target).await?;
    // A2.10: a paused account's changes do nothing here; each answers ok,
    // saying so, so a client that retries doesn't loop on a refusal.
    if paused {
        let mut body = Envelope::Ok {
            value: serde_json::json!({ "status": "paused" }),
            request_id: request.request_id,
            replayed: false,
            replay_safe: Some(true),
            preview: request.preview,
        }
        .to_json();
        body["paused"] = true.into();
        return Ok(Answer {
            status: StatusCode::OK,
            body,
        });
    }
    // H1.1: an agent's or app's call spends its workspace's shared budget.
    // After the connection goes back: the shared counter takes its own.
    if !spans && matches!(request.edge, Edge::Mcp { .. } | Edge::PublicApi { .. }) {
        state
            .rate_limits
            .check_workspace_call(located.workspace_id)
            .await?;
    }

    let key = request_id_for::<O>(&request)?;
    // K2.21: checked, then kept and run by a job; the job dispatches it
    // again (checked again) as the same caller.
    if request.defer_long && O::LONG && !request.preview && !spans && O::runs_long(&decoded) {
        // What the call would be refused for (a thing in another
        // workspace, a bad field) is answered now, not by the job: an
        // operation that previews is previewed first, and only a clean
        // preview is kept to run.
        if O::PREVIEW {
            let checked = DispatchRequest {
                preview: true,
                renamed_from: None,
                defer_long: false,
                ..request.clone()
            };
            let preview = Box::pin(pipeline::<O>(state, checked, canonical.clone())).await?;
            if preview.outcome() != "ok" {
                return Ok(preview);
            }
        }
        return super::long::start(
            state,
            &request,
            O::NAME,
            &located,
            &canonical,
            key.as_ref().map(IdempotencyKey::as_str),
        )
        .await;
    }
    let ctx = OpContext {
        caller: request.caller,
        edge: request.edge,
        trace_id: request.trace_id,
        workspace_id: located.workspace_id,
        space_id: located.space_id,
        direct: located.direct,
        requested_workspace: request.workspace,
        target: target.clone(),
        request_id: key
            .as_ref()
            .map(|key| key.as_str().to_owned())
            .or(request.request_id),
        op: O::NAME,
        preview: request.preview,
    };

    let mut call = OpCall {
        state,
        ctx: &ctx,
        tx: None,
        audit: None,
        replayed: false,
        start_queued_runs: false,
        keyed: false,
        audited: false,
    };
    // A2.14: a call through the API or an agent tool spends its caller's
    // budgets (the app's own pages are bounded by people's hands). Before
    // the call's transaction opens: see `budgets::admit_call`.
    if matches!(ctx.edge, Edge::Mcp { .. } | Edge::PublicApi { .. }) && !request.preview {
        crate::budgets::admit_call(
            &state.pool,
            ctx.workspace_id,
            ctx.space_id,
            ctx.caller.account_id,
            writes,
            ctx.request_id.as_deref(),
        )
        .await?;
    }
    let mut reservation = None;
    if let Some(key) = key.as_ref()
        && O::IDEMPOTENCY == Idempotency::Dispatcher
    {
        let scope = format!("op.{}.{}", O::NAME, target.scope_id(&located).simple());
        let hash = RequestHash::for_value(&canonical).map_err(|_| OpError::Unavailable)?;
        let tx = call.tx().await?;
        match idempotency::reserve(tx, ctx.caller.account_id, &scope, key, hash).await? {
            IdempotencyOutcome::Reserved(reserved) => reservation = Some(reserved),
            IdempotencyOutcome::Replay(replay) => {
                if let Some(tx) = call.tx.take() {
                    tx.commit()
                        .await
                        .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
                }
                let mut body = replay.response_body().clone();
                body["replayed"] = true.into();
                return Ok(Answer {
                    status: replay.status(),
                    body,
                });
            }
        }
    }

    // A2.27: the workspace's guards see every change before it runs: one
    // may stop it, or hold it for review (answered ok, saying so, with the
    // request that asks the reviewers). After a retry of a finished call has
    // had its answer above, so a lost answer doesn't ask the reviewers again;
    // a held or stopped call gives its reservation back (its transaction
    // rolls back).
    let mut guard_hold = None;
    if writes && !request.preview {
        // Boxed: every operation's dispatch would otherwise carry the
        // check's whole state on its stack.
        match Box::pin(crate::guards::check(
            state,
            &ctx.caller,
            &located,
            O::NAME,
            target.audit(&located),
            &canonical,
        ))
        .await?
        {
            crate::guards::Verdict::Allow { hold } => guard_hold = hold,
            crate::guards::Verdict::Deny { message } => {
                return Err(OpError::Denied {
                    reason: crate::ops::DenyReason::Rule,
                    message: message.into(),
                });
            }
            crate::guards::Verdict::Held { request_id } => {
                let mut body = Envelope::Ok {
                    value: serde_json::json!({
                        "status": "pending_review",
                        "requestId": request_id,
                    }),
                    request_id: ctx.request_id.clone(),
                    replayed: false,
                    replay_safe: Some(true),
                    preview: false,
                }
                .to_json();
                body["pendingReview"] = true.into();
                return Ok(Answer {
                    status: StatusCode::OK,
                    body,
                });
            }
        }
    }
    if let Some(hold) = guard_hold {
        let tx = call.tx().await?;
        crate::guards::use_hold(tx, hold).await?;
    }
    let output =
        crate::policy::within_agent_limits(ctx.caller.limits.clone(), O::run(&mut call, decoded))
            .await?;

    let envelope = Envelope::Ok {
        value: output,
        request_id: ctx.request_id.clone(),
        replayed: call.replayed,
        // A write without a request id ran once; a retry would run again.
        // A command that keeps its own idempotency is retry-safe only when
        // it says it deduplicated this call.
        replay_safe: writes.then_some(match O::IDEMPOTENCY {
            Idempotency::Dispatcher => ctx.request_id.is_some(),
            Idempotency::Command | Idempotency::None => call.keyed,
        }),
        preview: request.preview,
    }
    .to_json();
    let answer = Answer {
        status: StatusCode::OK,
        body: envelope,
    };

    if request.preview {
        // Dropping the transaction rolls it back.
        return Ok(answer);
    }
    // A spanning read has no one workspace to count its alias use in.
    if let Some(alias) = request.renamed_from.filter(|_| !spans) {
        let workspace_id = ctx.workspace_id;
        let tx = call.tx().await?;
        super::aliases::count(tx, workspace_id, alias)
            .await
            .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
    }
    if writes {
        let audit = call.audit.take();
        let audited = call.audited;
        let tx = call.tx().await?;
        let (kind, id, metadata) = match audit {
            Some(facts) => (facts.kind, facts.id, facts.metadata),
            None => {
                let (kind, id) = target.audit(&located);
                (kind, id, serde_json::json!({}))
            }
        };
        if !audited {
            audit_row(tx, &ctx, kind, id, metadata).await?;
        }
        if let Some(reservation) = reservation.as_ref() {
            // A secret shown once is never kept for a replay (it would sit
            // in plain text beside the request id).
            let mut stored = answer.body.clone();
            if let Some(value) = stored
                .get_mut("value")
                .and_then(|value| value.as_object_mut())
            {
                for field in O::SHOWN_ONCE {
                    if value.contains_key(*field) {
                        value.insert((*field).to_owned(), serde_json::Value::Null);
                    }
                }
            }
            idempotency::complete(tx, reservation, answer.status, &stored).await?;
        }
    }
    if let Some(tx) = call.tx.take() {
        tx.commit()
            .await
            .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
    }
    if call.start_queued_runs {
        crate::scheduler::kick(state.clone());
    }
    Ok(answer)
}

/// The changes a paused account still makes (A2.10): its status, lifting a
/// pause, its connections and reporting on runs it already has.
fn runs_while_paused(name: &str) -> bool {
    matches!(
        name,
        "status_set"
            | "status_delete"
            | "agent_resume"
            | "run_activity_report"
            | "run_result_report"
            | "run_usage_report"
            | "run_update"
    ) || name.starts_with("connection_")
}

/// A caller reaching several workspaces, calling an operation whose target
/// doesn't say which, must name one; the refusal lists them.
async fn name_the_workspace(connection: &mut sqlx::PgConnection, reach: &[Uuid]) -> OpError {
    let slugs = sqlx::query_scalar::<_, String>(
        "SELECT slug FROM organizations WHERE id = ANY($1) ORDER BY slug LIMIT 20",
    )
    .bind(reach)
    .fetch_all(&mut *connection)
    .await
    .unwrap_or_default();
    OpError::Invalid {
        field: "workspace".into(),
        message: format!(
            "You reach more than one workspace ({}): say which with workspace (its id or slug).",
            slugs.join(", ")
        )
        .into(),
    }
}

/// The module that must be on where an operation acts: its own, except
/// that a conversation's follows the space it is in (registered as
/// conversation filters: Messages in the direct space, Channels in a
/// project), as the older tools decide, and that an operation another
/// module also serves (`ModuleFilter::AlsoServes`, such as forms' links
/// under Collections) is held to that module where its own is off.
async fn module_here<O: Operation>(
    connection: &mut sqlx::PgConnection,
    caller: &Caller,
    located: &target::Located,
) -> Result<&'static str, OpError> {
    let registry = crate::modules::registry::Registry::global();
    if located.direct.is_none() && registry.also_serving(O::NAME).next().is_some() {
        let modules = match located.space_id {
            Some(space) => {
                crate::modules::state::for_project(connection, space, caller.account_id).await
            }
            None => crate::modules::state::for_member(
                connection,
                located.workspace_id,
                caller.account_id,
            )
            .await
            .map(Some),
        }
        .map_err(|error| OpError::logged(O::NAME, &error))?;
        if let Some(modules) = modules
            && !modules.contains(O::MODULE)
            && let Some(also) = registry
                .also_serving(O::NAME)
                .find(|also| modules.contains(also))
        {
            return Ok(also);
        }
    }
    if located.space_id.is_none() && located.direct.is_none() {
        return Ok(O::MODULE);
    }
    if !registry.governs_conversations(O::MODULE) {
        return Ok(O::MODULE);
    }
    Ok(registry
        .conversation_module(located.direct.is_some())
        .unwrap_or(O::MODULE))
}

/// What `access` is asked about for a located target.
fn access_target(target: &Target, located: &target::Located) -> crate::access::Target {
    match (target, located.space_id, located.direct) {
        (Target::Account, _, _) => crate::access::Target::Account,
        (_, Some(space), _) => crate::access::Target::Space(space),
        (_, None, Some(conversation)) => crate::access::Target::Conversation(conversation),
        (_, None, None) => crate::access::Target::Workspace(located.workspace_id),
    }
}

/// `access`'s refusal in the words the operation layer uses.
fn denied_error<O: Operation>(
    denied: crate::access::Denied,
    caller: &Caller,
    located: &target::Located,
) -> OpError {
    use crate::access::{Denied, DenyReason as Because};
    match denied {
        Denied::NotFound => match (located.space_id, located.direct) {
            (Some(_), _) => OpError::not_found("That space"),
            (None, Some(_)) => OpError::not_found("That conversation"),
            (None, None) => OpError::not_found("That workspace"),
        },
        Denied::ModuleOff { module } => OpError::Denied {
            reason: DenyReason::ModuleOff {
                module: module.to_owned(),
            },
            message: format!(
                "{} is turned off here.",
                crate::modules::manifest::get(module).map_or(module, |manifest| manifest.name.as_str())
            )
            .into(),
        },
        Denied::Permission {
            because: Because::ReadOnlyCredential,
            ..
        } => OpError::Denied {
            reason: DenyReason::ReadOnlyCredential,
            message: "This connection is read-only. Reconnect it with full access to make changes."
                .into(),
        },
        Denied::Permission {
            because: Because::CredentialCannotRead,
            ..
        } => OpError::Denied {
            reason: DenyReason::ReadOnlyCredential,
            message: "This connection can't read.".into(),
        },
        Denied::Permission { permission, .. } => match permission {
            Some(permission) => OpError::Denied {
                reason: DenyReason::Role { permission },
                message: format!(
                    "{} needs \"{}\" here, which your role doesn't give you{}. Ask a workspace owner or admin.",
                    O::TITLE,
                    permission.info().label,
                    if caller.limits.is_some() {
                        " through this app"
                    } else {
                        ""
                    },
                )
                .into(),
            },
            None => OpError::not_found("That workspace"),
        },
    }
}

/// Begin a call's transaction narrowed to the call's workspace
/// (`db::scoped_tx` under `Tenancy::Workspace`, H1.20) and the connected
/// app it acts through.
pub(super) async fn begin(
    state: &AppState,
    ctx: &OpContext,
) -> Result<Transaction<'static, Postgres>, OpError> {
    let mut tx = crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: Some(ctx.caller.account_id),
            workspace: ctx.workspace_id,
        },
        crate::db::scoped_tx(&state.pool),
    )
    .await
    .map_err(|error| OpError::logged(ctx.op, &error.into()))?;
    crate::domain::set_via_grant(&mut tx, ctx.caller.via_grant_id)
        .await
        .map_err(|error| OpError::logged(ctx.op, &error))?;
    // Every event the operation emits names who acted, through which app,
    // for whom, in which run, and the request id (K3.2).
    crate::events::set_context(
        &mut tx,
        &crate::events::EventContext {
            actor: Some(ctx.caller.account_id),
            via_credential: ctx.caller.via_grant_id,
            credential: ctx.caller.credential_id,
            requested_by: ctx.caller.requested_by,
            run: ctx.caller.run_id,
            request_id: ctx.request_id.clone(),
        },
    )
    .await
    .map_err(|error| OpError::logged(ctx.op, &error))?;
    Ok(tx)
}

/// The audit row a write leaves: the action is the operation's name, and
/// the edge says how it came.
async fn audit_row(
    tx: &mut Transaction<'static, Postgres>,
    ctx: &OpContext,
    target_kind: &str,
    target_id: Uuid,
    mut metadata: Value,
) -> Result<(), OpError> {
    struct Columns<'a> {
        session_id: Option<Uuid>,
        authentication_class: &'a str,
        channel: &'static str,
        client_name: Option<&'a str>,
        credential_id: Option<Uuid>,
        oauth_grant_id: Option<Uuid>,
        connection_id: Option<Uuid>,
    }
    let mut extra = serde_json::Map::new();
    let columns = match &ctx.edge {
        Edge::Web { session_id } => Columns {
            session_id: Some(*session_id),
            authentication_class: "password_session",
            channel: "browser",
            client_name: None,
            credential_id: None,
            oauth_grant_id: None,
            connection_id: None,
        },
        Edge::Mcp {
            credential_id,
            oauth_grant_id,
            connection_id,
            client_name,
            authentication_class,
        }
        | Edge::PublicApi {
            credential_id,
            oauth_grant_id,
            connection_id,
            client_name,
            authentication_class,
        } => Columns {
            session_id: None,
            authentication_class,
            channel: if matches!(ctx.edge, Edge::Mcp { .. }) {
                "mcp"
            } else {
                "api"
            },
            client_name: Some(client_name.as_str()),
            credential_id: *credential_id,
            oauth_grant_id: *oauth_grant_id,
            connection_id: *connection_id,
        },
        Edge::Automation { rule_id, run_id } => {
            extra.insert("automationRuleId".into(), rule_id.to_string().into());
            extra.insert("automationRunId".into(), run_id.to_string().into());
            Columns {
                session_id: None,
                authentication_class: "automation",
                channel: "automation",
                client_name: Some("automation"),
                credential_id: None,
                oauth_grant_id: None,
                connection_id: None,
            }
        }
        Edge::Job { job_id, source } => {
            extra.insert("jobId".into(), job_id.to_string().into());
            Columns {
                session_id: None,
                authentication_class: "background_job",
                channel: "system",
                client_name: Some(source),
                credential_id: None,
                oauth_grant_id: None,
                connection_id: None,
            }
        }
        Edge::Proposal {
            proposal_id,
            accepted_by,
        } => {
            extra.insert("proposalId".into(), proposal_id.to_string().into());
            extra.insert("acceptedBy".into(), accepted_by.to_string().into());
            Columns {
                session_id: None,
                authentication_class: "proposal",
                channel: "system",
                client_name: Some("proposal"),
                credential_id: None,
                oauth_grant_id: None,
                connection_id: None,
            }
        }
    };
    if !metadata.is_object() {
        metadata = serde_json::json!({});
    }
    if let Some(object) = metadata.as_object_mut() {
        object.extend(extra);
    }
    let client_name = columns
        .client_name
        .map(|name| name.chars().take(80).collect::<String>())
        .filter(|name| !name.is_empty());
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, organization_id, session_id,
            authentication_class, action, target_kind, target_id, outcome,
            request_id, correlation_id, metadata, channel, client_name,
            mcp_credential_id, mcp_oauth_grant_id, external_agent_connection_id,
            via_credential_id, requested_by_account_id, run_id
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, 'succeeded',
            $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(ctx.caller.account_id)
    .bind(ctx.space_id)
    .bind(ctx.workspace_id)
    .bind(columns.session_id)
    .bind(columns.authentication_class)
    .bind(ctx.op)
    .bind(target_kind)
    .bind(target_id)
    .bind(ctx.request_id.as_deref())
    .bind(
        ctx.trace_id
            .as_deref()
            .map(|trace| trace.chars().take(128).collect::<String>()),
    )
    .bind(metadata)
    .bind(columns.channel)
    .bind(client_name)
    .bind(columns.credential_id)
    .bind(columns.oauth_grant_id)
    .bind(columns.connection_id)
    .bind(ctx.caller.credential_id)
    .bind(ctx.caller.requested_by)
    .bind(ctx.caller.run_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| OpError::logged(ctx.op, &error.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decode_error_names_its_field() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Input {
            title: String,
        }
        let error = serde_json::from_value::<Input>(serde_json::json!({})).expect_err("missing");
        match decode_error(&error) {
            OpError::Invalid { field, message } => {
                assert_eq!(field, "title");
                assert!(message.contains("missing field"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
