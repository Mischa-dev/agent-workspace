//! The pipeline every call goes through, whichever edge it came in by:
//! find, decode, target, locate, module gate, access, transaction (tenancy
//! set), request id reserved, run, audit row, request id completed, commit.
//! An edge builds a `DispatchRequest` and sends the answer back in its own
//! framing; nothing here knows HTTP or JSON-RPC beyond the status.

use axum::http::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
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
    /// Run and roll back (operations with `PREVIEW`).
    pub preview: bool,
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
    call(state, request, input).await
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

/// The request id a write is held to: the caller's, or (MCP, the public
/// API, automations) one derived from who asked for what in the same ten
/// minutes, so a retried identical call is answered once.
fn request_id_for<O: Operation>(
    request: &DispatchRequest,
    canonical: &Value,
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
    if matches!(request.edge, Edge::Web { .. }) {
        return Err(OpError::Invalid {
            field: "Idempotency-Key".into(),
            message: "A change from the web app carries an Idempotency-Key.".into(),
        });
    }
    let bucket = chrono::Utc::now().timestamp() / 600;
    let mut hash = Sha256::new();
    hash.update(request.caller.account_id.as_bytes());
    hash.update(O::NAME.as_bytes());
    hash.update(canonical.to_string().as_bytes());
    hash.update(bucket.to_be_bytes());
    let derived = format!("d-{}", &hex::encode(hash.finalize())[..40]);
    IdempotencyKey::parse(&derived)
        .map(Some)
        .map_err(|_| OpError::Unavailable)
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
    let decoded =
        serde_json::from_value::<O::Input>(input).map_err(|error| decode_error(&error))?;
    // The input as the operation reads it: what the request id is held to.
    let canonical = serde_json::to_value(&decoded).map_err(|_| OpError::Unavailable)?;
    let target = decoded.target();
    let writes = O::KIND.writes();

    let mut connection = state
        .pool
        .acquire()
        .await
        .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
    // A call that can't name its workspace needs none named when the
    // caller reaches only one.
    if request.workspace.is_none() && matches!(target, Target::Account | Target::Workspace(None)) {
        let reach = access_shim::reach(&mut connection, &request.caller).await?;
        match reach.as_slice() {
            [only] => request.workspace = Some(*only),
            [] => {}
            several => return Err(name_the_workspace(&mut connection, several).await),
        }
    }
    let located = target::locate(
        &mut connection,
        &target,
        request.caller.account_id,
        request.workspace,
    )
    .await?;
    module_gate::<O>(&mut connection, &request.caller, &target, &located).await?;
    let _grant = access_shim::check(
        &mut connection,
        &request.caller,
        &target,
        &located,
        O::PERMISSION,
        writes,
        O::TITLE,
    )
    .await?;
    drop(connection);

    let key = request_id_for::<O>(&request, &canonical)?;
    let ctx = OpContext {
        caller: request.caller,
        edge: request.edge,
        trace_id: request.trace_id,
        workspace_id: located.workspace_id,
        space_id: located.space_id,
        target: target.clone(),
        request_id: key
            .as_ref()
            .map(|key| key.as_str().to_owned())
            .or(request.request_id),
        op: O::NAME,
    };

    let mut call = OpCall {
        state,
        ctx: &ctx,
        tx: None,
        audit: None,
        replayed: false,
    };
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

    let output =
        crate::policy::within_agent_limits(ctx.caller.limits.clone(), O::run(&mut call, decoded))
            .await?;

    let envelope = Envelope::Ok {
        value: output,
        request_id: ctx.request_id.clone(),
        replayed: call.replayed,
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
    if writes {
        let audit = call.audit.take();
        let tx = call.tx().await?;
        let (kind, id, metadata) = match audit {
            Some(facts) => (facts.kind, facts.id, facts.metadata),
            None => {
                let (kind, id) = target.audit(&located);
                (kind, id, serde_json::json!({}))
            }
        };
        audit_row(tx, &ctx, kind, id, metadata).await?;
        if let Some(reservation) = reservation.as_ref() {
            idempotency::complete(tx, reservation, answer.status, &answer.body).await?;
        }
    }
    if let Some(tx) = call.tx.take() {
        tx.commit()
            .await
            .map_err(|error| OpError::logged(O::NAME, &error.into()))?;
    }
    Ok(answer)
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

/// The operation's module must be on where it acts.
async fn module_gate<O: Operation>(
    connection: &mut sqlx::PgConnection,
    caller: &Caller,
    target: &Target,
    located: &target::Located,
) -> Result<(), OpError> {
    let Some(manifest) = crate::modules::manifest::get(O::MODULE) else {
        return Ok(());
    };
    if !manifest.switchable || matches!(target, Target::Account) {
        return Ok(());
    }
    let modules = match located.space_id {
        Some(space) => {
            crate::modules::state::for_project(connection, space, caller.account_id).await?
        }
        None => Some(
            crate::modules::state::for_member(connection, located.workspace_id, caller.account_id)
                .await?,
        ),
    };
    if modules.is_some_and(|modules| !modules.contains(O::MODULE)) {
        return Err(OpError::Denied {
            reason: DenyReason::ModuleOff {
                module: manifest.id.clone(),
            },
            message: format!("{} is turned off here.", manifest.name).into(),
        });
    }
    Ok(())
}

/// Begin a call's transaction with its tenancy scope set (H1.20 replaces
/// this with `db::scoped_tx`) and the connected app it acts through.
pub(super) async fn begin(
    state: &AppState,
    ctx: &OpContext,
) -> Result<Transaction<'static, Postgres>, OpError> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| OpError::logged(ctx.op, &error.into()))?;
    sqlx::query("SELECT set_config('app.tenancy', $1, true)")
        .bind(format!("w:{}|{}", ctx.caller.account_id, ctx.workspace_id))
        .execute(&mut *tx)
        .await
        .map_err(|error| OpError::logged(ctx.op, &error.into()))?;
    crate::domain::set_via_grant(&mut tx, ctx.caller.via_grant_id)
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
            mcp_credential_id, mcp_oauth_grant_id, external_agent_connection_id
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, 'succeeded',
            $10, $11, $12, $13, $14, $15, $16, $17
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
