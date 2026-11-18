//! Workspace lifecycle (H1.24, H1 section 3.6): the one way a workspace is
//! made, and how it is suspended, resumed, scheduled for deletion and
//! deleted.
//!
//! - [`create`] is the only `INSERT INTO organizations`: bootstrap, the
//!   development seed and single sign-on sign-up call it, and first run,
//!   sign-up and import will.
//! - A `suspended` workspace refuses MCP, API and operation calls with
//!   `workspace_suspended` ([`ensure_open`]); people can still sign in and
//!   see why.
//! - [`request_deletion`] marks it `deleting` and queues the
//!   `workspace.delete` job for 14 days later; [`cancel_deletion`] undoes
//!   both. While it is deleting, every call but cancelling is refused.
//! - The job ([`delete_now`]) deletes the workspace's file bytes, retires the
//!   agent and app accounts it owns (credentials revoked, memberships
//!   elsewhere ended), then removes every row that names the workspace or
//!   hangs off one that does, in one statement, and leaves one line in
//!   `deleted_workspaces`. Accounts are server-wide and stay (R19); DMs in
//!   the direct scope stay with their participants, losing only references
//!   to the workspace's things.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    jobs::{JobError, JobRegistry, NewJob},
    tenancy::{self, Tenancy},
};

/// How long an owner has to change their mind.
pub const DELETION_GRACE_DAYS: i64 = 14;

/// The job that deletes a workspace once its grace period is over.
pub const DELETE_JOB_KIND: &str = "workspace.delete";

/// The job that stops a closed workspace's runs, after the change commits.
pub const STOP_RUNS_JOB_KIND: &str = "workspace.stop_runs";

/// The calls a workspace that is deleting still answers.
// Asking again answers with the schedule it has, so a retried request
// (its response lost) still succeeds.
const ALLOWED_WHILE_DELETING: &[&str] = &["workspace_delete_cancel", "workspace_delete"];

/// Where a workspace is in its life.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    /// Being filled by an import (H1.10).
    Importing,
    /// Stopped by the server's operators or staff.
    Suspended,
    /// An owner asked to delete it; it goes at `delete_after`.
    Deleting,
}

impl Lifecycle {
    pub fn parse(stored: &str) -> Result<Self, AppError> {
        Ok(match stored {
            "active" => Self::Active,
            "importing" => Self::Importing,
            "suspended" => Self::Suspended,
            "deleting" => Self::Deleting,
            _ => return Err(AppError::Internal("unknown workspace lifecycle")),
        })
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Importing => "importing",
            Self::Suspended => "suspended",
            Self::Deleting => "deleting",
        }
    }
}

/// Why a workspace was suspended. Stored as its key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuspendReason {
    /// The server's operator stopped it (the `workspace-suspend` command).
    Operator,
    /// Staff found abuse: owners can't export either (H1.29).
    Abuse,
    /// Unpaid (H1.27).
    Billing,
}

impl SuspendReason {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "operator" => Self::Operator,
            "abuse" => Self::Abuse,
            "billing" => Self::Billing,
            _ => return None,
        })
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Abuse => "abuse",
            Self::Billing => "billing",
        }
    }
}

/// A workspace to make.
#[derive(Clone, Debug)]
pub struct NewWorkspace {
    pub name: String,
    /// Its address. Left out, a readable one from the name (K4.5). A slug
    /// a person chose is checked with `slug::check_new` by the caller.
    pub slug: Option<String>,
    /// Its id, when it keeps the one it had elsewhere (an import, H1.10);
    /// left out, a new one.
    pub id: Option<Uuid>,
}

/// Make a workspace with `owner` as its owner. The one way a workspace row
/// is written.
pub async fn create(
    connection: &mut PgConnection,
    owner: Uuid,
    input: NewWorkspace,
) -> Result<Uuid, AppError> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("workspace name"));
    }
    let slug = match input.slug {
        Some(slug) => slug,
        None => super::slug::readable_for_new(connection, name).await?,
    };
    let id = input.id.unwrap_or_else(Uuid::now_v7);
    let inserted = sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(&slug)
        .execute(&mut *connection)
        .await;
    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23505") => {
            return Err(AppError::Conflict(
                "workspace slug",
                "that address belongs to another workspace",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, actor_id, role, enabled)
        VALUES ($1, $2, 'owner', true)
        "#,
    )
    .bind(id)
    .bind(owner)
    .execute(&mut *connection)
    .await?;
    crate::metering::start_plan(connection, id).await?;
    Ok(id)
}

/// Marks a workspace just made as being imported (H1.10): it refuses
/// writes until [`finish_import`].
pub async fn begin_import(connection: &mut PgConnection, workspace: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE organizations SET lifecycle = 'importing', lifecycle_changed_at = now() \
         WHERE id = $1 AND lifecycle = 'active'",
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Opens a workspace whose import finished.
pub async fn finish_import(connection: &mut PgConnection, workspace: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE organizations SET lifecycle = 'active', lifecycle_changed_at = now() \
         WHERE id = $1 AND lifecycle = 'importing'",
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// A workspace's state, as the edges and the web read it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleView {
    pub lifecycle: Lifecycle,
    /// Why it was suspended: `operator`, `abuse` or `billing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When a deleting workspace goes for good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<DateTime<Utc>>,
}

/// The state of `workspace`, or `None` when there is no such workspace.
pub async fn read(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Option<LifecycleView>, AppError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<DateTime<Utc>>)>(
        "SELECT lifecycle, lifecycle_reason, delete_after FROM organizations WHERE id = $1",
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|(lifecycle, reason, delete_after)| {
        Ok(LifecycleView {
            lifecycle: Lifecycle::parse(&lifecycle)?,
            reason,
            delete_after,
        })
    })
    .transpose()
}

/// Refuses a call into a workspace that is suspended, or deleting unless
/// the call is one that still works then (cancelling). `call` is the
/// operation or tool name.
pub async fn ensure_open(
    connection: &mut PgConnection,
    workspace: Uuid,
    call: &str,
) -> Result<(), AppError> {
    check_open(connection, workspace, call, false).await
}

async fn check_open(
    connection: &mut PgConnection,
    workspace: Uuid,
    call: &str,
    hold: bool,
) -> Result<(), AppError> {
    let lifecycle = sqlx::query_scalar::<_, String>(if hold {
        "SELECT lifecycle FROM organizations WHERE id = $1 FOR SHARE"
    } else {
        "SELECT lifecycle FROM organizations WHERE id = $1"
    })
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?;
    match lifecycle.as_deref() {
        // Nobody writes into a workspace while it is being imported.
        Some("suspended" | "importing") => Err(AppError::WorkspaceSuspended),
        Some("deleting") if !ALLOWED_WHILE_DELETING.contains(&call) => {
            Err(AppError::WorkspaceSuspended)
        }
        _ => Ok(()),
    }
}

/// Like [`ensure_open`], for the workspace a project (space) is in.
pub async fn ensure_open_for_space(
    connection: &mut PgConnection,
    space: Uuid,
    call: &str,
) -> Result<Option<Uuid>, AppError> {
    let workspace =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(space)
            .fetch_optional(&mut *connection)
            .await?;
    if let Some(workspace) = workspace {
        ensure_open(connection, workspace, call).await?;
    }
    Ok(workspace)
}

/// Like [`ensure_open_for_space`], and holds the workspace open until the
/// caller's transaction ends: a suspension or deletion request waits for
/// it, so a write that passed here commits before the workspace closes.
/// For writes that set work going (a run, an approval).
pub async fn hold_open_for_space(
    connection: &mut PgConnection,
    space: Uuid,
    call: &str,
) -> Result<Option<Uuid>, AppError> {
    let workspace =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(space)
            .fetch_optional(&mut *connection)
            .await?;
    if let Some(workspace) = workspace {
        check_open(connection, workspace, call, true).await?;
    }
    Ok(workspace)
}

/// The browser's v1 routes refuse writes into a suspended or deleting
/// workspace, as operations and tools do. The workspace comes from the
/// route the way the module gate finds it (`/projects/{id}`,
/// `/organizations/{id}`, content registered by id), plus a conversation
/// or message named by id. Reads still answer, so people can see their
/// work and the state; keeping a workspace is an operation (`/api/v2`),
/// which checks for itself. Routes that name no workspace (a person's own
/// account, sign-in) are not the workspace's to refuse.
pub async fn refuse_writes_when_closed(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{RequestExt, http::Method, response::IntoResponse};

    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return next.run(request).await;
    }
    let Some(template) = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
    else {
        return next.run(request).await;
    };
    if !template.starts_with("/api/v1/") {
        return next.run(request).await;
    }
    let params = request
        .extract_parts::<axum::extract::RawPathParams>()
        .await
        .ok();
    let param = |name: &str| {
        params
            .as_ref()?
            .iter()
            .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
    };
    match closed_for_route(&state, &template, param).await {
        Ok(false) => next.run(request).await,
        Ok(true) => AppError::WorkspaceSuspended.into_response(),
        Err(error) => error.into_response(),
    }
}

async fn closed_for_route(
    state: &AppState,
    template: &str,
    param: impl Fn(&str) -> Option<String>,
) -> Result<bool, AppError> {
    use crate::modules::gate::{self, Scope};

    let found = match gate::scope(template, &param) {
        Some(found) => Some(found),
        None => match gate::id_scope(state, template, &param).await? {
            Some(found) => Some(found),
            None => conversation_scope(state, template, &param).await?,
        },
    };
    let Some(found) = found else {
        return Ok(false);
    };
    let lifecycle = crate::db::scoped_one(&state.pool, async |connection| match found {
        Scope::Workspace(workspace) => {
            sqlx::query_scalar::<_, String>("SELECT lifecycle FROM organizations WHERE id = $1")
                .bind(workspace)
                .fetch_optional(connection)
                .await
        }
        Scope::Project(space) => {
            sqlx::query_scalar::<_, String>(
                "SELECT organization.lifecycle FROM projects space \
                 JOIN organizations organization ON organization.id = space.organization_id \
                 WHERE space.id = $1",
            )
            .bind(space)
            .fetch_optional(connection)
            .await
        }
    })
    .await?;
    Ok(matches!(
        lifecycle.as_deref(),
        Some("suspended" | "deleting")
    ))
}

/// The other id-routed v1 families the module gate leaves alone
/// (`modules::gate::ID_EXEMPT`) that write in one workspace, and how each
/// finds its space. Bug reports and forms by token are checked by their
/// handlers; saved-for-later items are a person's own, across workspaces.
const OTHER_ID_FAMILIES: &[(&str, &str)] = &[
    (
        "/api/v1/files/{",
        "SELECT project_id FROM files \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE id = $1",
    ),
    (
        "/api/v1/bug-report-links/{",
        "SELECT project_id FROM bug_report_links \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE id = $1",
    ),
    (
        "/api/v1/project-invitations/{",
        "SELECT project_id FROM project_invitations \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE id = $1",
    ),
    (
        "/api/v1/questions/{",
        "SELECT project_id FROM person_questions \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE id = $1",
    ),
    (
        "/api/v1/reply-requests/{",
        "SELECT project_id FROM reply_requests \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE id = $1",
    ),
    (
        "/api/v1/webhooks/{",
        "SELECT COALESCE(webhook.project_id, ( \
             SELECT space.id FROM projects space \
             WHERE space.organization_id = webhook.organization_id \
             ORDER BY space.created_at LIMIT 1)) \
         FROM webhook_subscriptions webhook \
         -- visibility: the space only, to refuse a write; the handler checks access\n \
         WHERE webhook.id = $1",
    ),
];

/// The space of a conversation (channel, thread), message, scheduled
/// message or run a v1 route names by id.
async fn conversation_scope(
    state: &AppState,
    template: &str,
    param: impl Fn(&str) -> Option<String>,
) -> Result<Option<crate::modules::gate::Scope>, AppError> {
    let (rest, lookup) = if let Some(rest) = template.strip_prefix("/api/v1/conversations/{") {
        (
            rest,
            "SELECT project_id FROM conversations \
             -- visibility: the space only, to refuse a write; the handler checks access\n \
             WHERE id = $1",
        )
    } else if let Some(rest) = ["/api/v1/threads/{", "/api/v1/channels/{"]
        .iter()
        .find_map(|prefix| template.strip_prefix(prefix))
    {
        (
            rest,
            "SELECT project_id FROM conversations \
             -- visibility: the space only, to refuse a write; the handler checks access\n \
             WHERE id = $1",
        )
    } else if let Some(rest) = template.strip_prefix("/api/v1/agent-runs/{") {
        (rest, crate::runs::SPACE_OF_RUN)
    } else if let Some(rest) = template.strip_prefix("/api/v1/scheduled-messages/{") {
        (
            rest,
            "SELECT conversation.project_id FROM scheduled_messages scheduled \
             JOIN conversations conversation ON conversation.id = scheduled.conversation_id \
             -- visibility: the space only, to refuse a write; the handler checks access\n \
             WHERE scheduled.id = $1",
        )
    } else if let Some(rest) = template.strip_prefix("/api/v1/messages/{") {
        (
            rest,
            "SELECT conversation.project_id FROM messages message \
             JOIN conversations conversation ON conversation.id = message.conversation_id \
             -- visibility: the space only, to refuse a write; the handler checks access\n \
             WHERE message.id = $1",
        )
    } else if let Some((rest, lookup)) = OTHER_ID_FAMILIES
        .iter()
        .find_map(|(prefix, lookup)| template.strip_prefix(prefix).map(|rest| (rest, *lookup)))
    {
        (rest, lookup)
    } else {
        return Ok(None);
    };
    let Some(name) = rest.split_once('}').map(|(name, _)| name) else {
        return Ok(None);
    };
    let Some(id) = param(name).and_then(|raw| Uuid::parse_str(&raw).ok()) else {
        return Ok(None);
    };
    let space = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_scalar::<_, Uuid>(lookup)
            .bind(id)
            .fetch_optional(connection)
            .await
    })
    .await?;
    Ok(space.map(crate::modules::gate::Scope::Project))
}

/// Lock the workspace row and read its lifecycle.
async fn lock(connection: &mut PgConnection, workspace: Uuid) -> Result<Lifecycle, AppError> {
    let stored = sqlx::query_scalar::<_, String>(
        "SELECT lifecycle FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace"))?;
    Lifecycle::parse(&stored)
}

/// Tell open screens and other processes the workspace's lifecycle changed
/// (rule 6), in the same transaction. The envelope needs a space: the
/// workspace's own.
async fn announce(
    connection: &mut PgConnection,
    workspace: Uuid,
    lifecycle: Lifecycle,
) -> Result<(), AppError> {
    let Some(space) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 \
         ORDER BY (kind = $2) DESC, created_at LIMIT 1",
    )
    .bind(workspace)
    .bind(crate::spaces::SpaceKind::Workspace.stored())
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(());
    };
    let event = crate::outbox::NewOutboxEvent::new(
        space,
        crate::outbox::OutboxEventPayload::WorkspaceLifecycleChanged(
            crate::outbox::WorkspaceLifecycleChangedPayload {
                workspace_id: workspace,
                project_id: space,
                lifecycle: lifecycle.key().to_owned(),
            },
        ),
    )
    .map_err(|_| AppError::Internal("workspace lifecycle event"))?;
    crate::outbox::enqueue_on(connection, &event).await?;
    Ok(())
}

/// Stop a workspace. An active or importing workspace only.
pub async fn suspend(
    connection: &mut PgConnection,
    workspace: Uuid,
    reason: SuspendReason,
) -> Result<(), AppError> {
    match lock(connection, workspace).await? {
        Lifecycle::Active | Lifecycle::Importing => {}
        Lifecycle::Suspended => {
            return Err(AppError::Conflict("workspace", "already suspended"));
        }
        Lifecycle::Deleting => {
            return Err(AppError::Conflict("workspace", "being deleted"));
        }
    }
    sqlx::query(
        r#"
        UPDATE organizations
        SET lifecycle = 'suspended', lifecycle_reason = $2, lifecycle_changed_at = now(),
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(workspace)
    .bind(reason.key())
    .execute(&mut *connection)
    .await?;
    // Nothing keeps running in a suspended workspace: its runs stop once
    // this commits, and their drivers stop the harnesses' turns.
    stop_runs_after_commit(connection, workspace).await?;
    announce(connection, workspace, Lifecycle::Suspended).await
}

/// Let a suspended workspace work again.
pub async fn resume(connection: &mut PgConnection, workspace: Uuid) -> Result<(), AppError> {
    if lock(connection, workspace).await? != Lifecycle::Suspended {
        return Err(AppError::Conflict("workspace", "not suspended"));
    }
    sqlx::query(
        r#"
        UPDATE organizations
        SET lifecycle = 'active', lifecycle_reason = NULL, lifecycle_changed_at = now(),
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    announce(connection, workspace, Lifecycle::Active).await
}

/// One job per schedule: asking again after keeping the workspace makes a
/// new job, even while an old one is still finishing (it finds the
/// workspace kept, or not yet due, and ends).
fn delete_job_key(workspace: Uuid, delete_after: DateTime<Utc>) -> String {
    format!(
        "{DELETE_JOB_KIND}:{workspace}:{}",
        delete_after.timestamp_micros()
    )
}

/// Schedule the workspace's deletion [`DELETION_GRACE_DAYS`] from now.
/// Returns when it goes. Asking again while it is deleting answers the
/// date already set.
pub async fn request_deletion(
    connection: &mut PgConnection,
    workspace: Uuid,
    by: Uuid,
) -> Result<DateTime<Utc>, AppError> {
    match lock(connection, workspace).await? {
        Lifecycle::Active => {}
        Lifecycle::Deleting => {
            return sqlx::query_scalar::<_, DateTime<Utc>>(
                "SELECT delete_after FROM organizations WHERE id = $1",
            )
            .bind(workspace)
            .fetch_one(&mut *connection)
            .await
            .map_err(AppError::from);
        }
        Lifecycle::Importing => {
            return Err(AppError::Conflict("workspace", "still importing"));
        }
        Lifecycle::Suspended => return Err(AppError::WorkspaceSuspended),
    }
    // Whole seconds, so the job key made from it matches the stored time.
    let delete_after =
        chrono::SubsecRound::trunc_subsecs(Utc::now() + Duration::days(DELETION_GRACE_DAYS), 0);
    sqlx::query(
        r#"
        UPDATE organizations
        SET lifecycle = 'deleting', lifecycle_reason = NULL, lifecycle_changed_at = now(),
            delete_after = $2, deletion_requested_by_actor_id = $3,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(workspace)
    .bind(delete_after)
    .bind(by)
    .execute(&mut *connection)
    .await?;
    // A server job, not the workspace's own: the deletion removes the
    // workspace's jobs with everything else.
    crate::jobs::enqueue(
        &mut *connection,
        NewJob::new(DELETE_JOB_KIND)
            .payload(DeletePayload { workspace })
            .unique(delete_job_key(workspace, delete_after))
            .run_at(delete_after)
            .max_attempts(20),
    )
    .await?;
    // A workspace being deleted takes no work, so its runs stop too.
    stop_runs_after_commit(connection, workspace).await?;
    announce(connection, workspace, Lifecycle::Deleting).await?;
    Ok(delete_after)
}

/// Queue the stop of `workspace`'s runs. Not inside the closing
/// transaction: it holds the workspace's row, and stopping a run writes to
/// its thread, which a run start locks before it asks whether the
/// workspace is open. Once this commits no run starts there, and the job
/// stops the ones already going. A server job, as the deletion is. Each
/// closure queues its own: one still running from an earlier closure may
/// have read the workspace open in between and stopped nothing.
async fn stop_runs_after_commit(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<(), AppError> {
    crate::jobs::enqueue(
        &mut *connection,
        NewJob::new(STOP_RUNS_JOB_KIND)
            .payload(StopRunsPayload { workspace })
            .max_attempts(20),
    )
    .await?;
    Ok(())
}

/// Stop every unfinished run in `workspace` while it is suspended or being
/// deleted; a workspace that opened again keeps its runs. How many stopped.
pub async fn stop_runs(state: &AppState, workspace: Uuid) -> Result<usize, AppError> {
    tenancy::scoped(
        Tenancy::Workspace {
            account: None,
            workspace,
        },
        async {
            let mut transaction = crate::db::scoped_tx(&state.pool).await?;
            let lifecycle = sqlx::query_scalar::<_, String>(
                "SELECT lifecycle FROM organizations WHERE id = $1",
            )
            .bind(workspace)
            .fetch_optional(&mut *transaction)
            .await?;
            let why = match lifecycle.as_deref().map(Lifecycle::parse).transpose()? {
                Some(Lifecycle::Suspended) => "workspace_suspended",
                Some(Lifecycle::Deleting) => "workspace_deleting",
                _ => return Ok(0),
            };
            let mut stopped = crate::hosts::builtin::http::interrupt_workspace_runs(
                &mut transaction,
                workspace,
                why,
            )
            .await?;
            // Runs that report for themselves end through the run service.
            stopped += crate::runs::ops::end_for_closed_workspace(
                &mut transaction,
                workspace,
                "Stopped because the workspace was suspended or is being deleted.",
            )
            .await?;
            transaction.commit().await?;
            Ok::<_, AppError>(stopped)
        },
    )
    .await
}

/// Keep a workspace that was going to be deleted.
pub async fn cancel_deletion(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<(), AppError> {
    if lock(connection, workspace).await? != Lifecycle::Deleting {
        return Err(AppError::Conflict("workspace", "not being deleted"));
    }
    let delete_after = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"
        UPDATE organizations organization
        SET lifecycle = 'active', lifecycle_changed_at = now(), delete_after = NULL,
            deletion_requested_by_actor_id = NULL, version = organization.version + 1,
            updated_at = now()
        FROM (SELECT delete_after FROM organizations WHERE id = $1) previous
        WHERE organization.id = $1
        RETURNING previous.delete_after
        "#,
    )
    .bind(workspace)
    .fetch_one(&mut *connection)
    .await?;
    if let Some(delete_after) = delete_after {
        crate::jobs::cancel_queued(&mut *connection, &delete_job_key(workspace, delete_after))
            .await?;
    }
    announce(connection, workspace, Lifecycle::Active).await
}

/// The agent and app accounts `workspace` owns that are still active:
/// they retire with it.
pub async fn owned_accounts(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM actors WHERE owner_workspace_id = $1 AND status = 'active' ORDER BY id",
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletePayload {
    workspace: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StopRunsPayload {
    workspace: Uuid,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(STOP_RUNS_JOB_KIND, |context| async move {
        let payload = context.job.payload_as::<StopRunsPayload>()?;
        stop_runs(&context.state, payload.workspace)
            .await
            .map_err(JobError::from)?;
        Ok(())
    });
    registry.register_with_timeout(
        DELETE_JOB_KIND,
        std::time::Duration::from_secs(60 * 60),
        |context| async move {
            let payload = context.job.payload_as::<DeletePayload>()?;
            delete_now(&context.state, payload.workspace, false)
                .await
                .map_err(JobError::from)?;
            Ok(())
        },
    );
}

/// What a deletion removed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Deleted {
    /// Rows removed, per table.
    pub rows: BTreeMap<String, u64>,
    /// Accounts the workspace owned, now retired.
    pub retired_accounts: Vec<Uuid>,
}

/// Delete `workspace` now: its file bytes, then every row. Only a workspace
/// that is deleting and past its date, unless `ignore_date` (tests and the
/// operator's command). A workspace that is gone, or was kept, is left as
/// it is and `None` is returned.
pub async fn delete_now(
    state: &AppState,
    workspace: Uuid,
    ignore_date: bool,
) -> Result<Option<Deleted>, AppError> {
    tenancy::scoped(
        Tenancy::Workspace {
            account: None,
            workspace,
        },
        delete_in_scope(state, workspace, ignore_date),
    )
    .await
}

async fn delete_in_scope(
    state: &AppState,
    workspace: Uuid,
    ignore_date: bool,
) -> Result<Option<Deleted>, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let connection = &mut *transaction;
    // The decision is taken under the workspace's row lock, which keeping
    // the workspace (`cancel_deletion`) also takes: once this transaction
    // holds it, a cancellation waits and then finds no workspace.
    let still_due = sqlx::query_scalar::<_, bool>(
        "SELECT lifecycle = 'deleting' AND ($2 OR delete_after <= now()) \
         FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(workspace)
    .bind(ignore_date)
    .fetch_optional(&mut *connection)
    .await?;
    match still_due {
        Some(true) => {}
        Some(false) => return Ok(None),
        None => {
            // Already deleted: a previous attempt committed the rows and
            // then failed to remove the bytes. Remove them now.
            let repositories = sqlx::query_scalar::<_, Vec<Uuid>>(
                "SELECT repository_ids FROM deleted_workspaces WHERE id = $1",
            )
            .bind(workspace)
            .fetch_optional(&mut *connection)
            .await?;
            drop(transaction);
            if let Some(repositories) = repositories {
                remove_bytes(state, workspace, &repositories).await?;
            }
            return Ok(None);
        }
    }
    // Repository mirrors and worktrees live on disk by repository id, which
    // the rows below are the only record of: the ids are kept with the
    // deleted workspace, and the bytes go once the deletion commits.
    let repositories = sqlx::query_scalar::<_, Uuid>(
        "SELECT repository.id FROM repositories repository \
         JOIN projects project ON project.id = repository.project_id \
         WHERE project.organization_id = $1",
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    sqlx::query("SELECT set_config('aw.workspace_delete', $1, true)")
        .bind(workspace.to_string())
        .execute(&mut *connection)
        .await?;
    let requested_by = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT deletion_requested_by_actor_id FROM organizations WHERE id = $1",
    )
    .bind(workspace)
    .fetch_one(&mut *connection)
    .await?;
    let retired_accounts = retire_owned_accounts(connection, workspace).await?;
    sqlx::query(
        "UPDATE users SET default_organization_id = NULL, updated_at = now() \
         WHERE default_organization_id = $1",
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    crate::delivery::connections::rehome_from(connection, workspace).await?;
    let rows = delete_rows(connection, workspace).await?;
    sqlx::query(
        "INSERT INTO deleted_workspaces (id, requested_by_actor_id, repository_ids) \
         VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace)
    .bind(requested_by)
    .bind(&repositories)
    .execute(&mut *connection)
    .await?;
    transaction.commit().await?;
    // The bytes go only once the rows are gone for good, so a kept
    // workspace never loses a file or a commit. If this fails, the job's
    // next attempt finds the workspace in `deleted_workspaces` and tries
    // again.
    remove_bytes(state, workspace, &repositories).await?;
    tracing::info!(
        target: "agent_workspace::lifecycle",
        workspace_id = %workspace,
        tables = rows.len(),
        retired_accounts = retired_accounts.len(),
        "workspace deleted"
    );
    Ok(Some(Deleted {
        rows,
        retired_accounts,
    }))
}

/// Remove what a deleted workspace kept on disk: its repositories'
/// mirrors and worktrees, and its file folder. Missing pieces are fine, so
/// a retry finishes a removal that failed part way.
async fn remove_bytes(
    state: &AppState,
    workspace: Uuid,
    repositories: &[Uuid],
) -> Result<(), AppError> {
    for repository in repositories {
        crate::git_runtime::remove_repository_data(&state.config.data_root, *repository).await?;
    }
    let own = crate::files::workspace_store::all_of(state, workspace).await?;
    for store in crate::files::store::FileStore::all(&state.config)
        .into_iter()
        .chain(own)
    {
        store.delete_workspace(workspace).await?;
    }
    Ok(())
}

/// Retire the agent and app accounts the workspace owns: disabled, with no
/// owner, their credentials revoked and their memberships everywhere
/// ended. People own themselves, so this never touches a person.
async fn retire_owned_accounts(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let owned = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE actors
        SET status = 'disabled', owner_workspace_id = NULL, updated_at = now()
        WHERE owner_workspace_id = $1
        RETURNING id
        "#,
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    if owned.is_empty() {
        return Ok(owned);
    }
    for statement in [
        "UPDATE organization_memberships SET enabled = false, version = version + 1, \
         updated_at = now() WHERE actor_id = ANY($1) AND enabled",
        "UPDATE project_memberships SET enabled = false, disabled_at = now(), \
         version = version + 1, updated_at = now() WHERE actor_id = ANY($1) AND enabled",
        "UPDATE mcp_client_credentials SET status = 'revoked', revoked_at = now(), \
         version = version + 1, updated_at = now() WHERE actor_id = ANY($1) AND status = 'active'",
        "UPDATE mcp_oauth_grants SET revoked_at = now() \
         WHERE actor_id = ANY($1) AND revoked_at IS NULL",
        "UPDATE sessions SET revoked_at = now(), updated_at = now() \
         WHERE revoked_at IS NULL AND user_id IN (SELECT id FROM users WHERE actor_id = ANY($1))",
    ] {
        sqlx::query(statement)
            .bind(&owned)
            .execute(&mut *connection)
            .await?;
    }
    Ok(owned)
}

/// One foreign key, as the catalog describes it.
#[derive(Clone, Debug)]
struct ForeignKey {
    child: String,
    child_columns: Vec<String>,
    parent: String,
    parent_columns: Vec<String>,
    /// Every child column may be null, so a row of another workspace can
    /// let go of the reference instead of going with it.
    detachable: bool,
}

/// The tenant column of a workspace-owned table.
fn tenant_column(table: &str) -> Option<&'static str> {
    crate::tenancy::tables::TENANT_TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, column)| *column)
}

/// Server-wide tables the deletion walks into: the hosts the workspace owns,
/// the connections still at home there (`delivery::connections::rehome_from`
/// first moves those that reach on elsewhere), and what hangs off them.
/// Every other server-wide table is never touched (accounts stay, R19).
const OWNED_GLOBAL_TABLES: &[(&str, &str)] = &[
    ("hosts", "owner_workspace_id"),
    ("host_enrolments", "owner_workspace_id"),
    ("host_agents", ""),
    ("host_notices", ""),
    ("agent_connections", "home_workspace_id"),
    ("connection_signing_secrets", ""),
    ("event_subscriptions", ""),
];

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Remove every row of the workspace: rows whose tenant column names it
/// (`tenancy::tables`), the hosts it owns, and, following foreign keys that
/// don't set the reference to null, every row that points at one of those,
/// to the end. The rows to go are gathered first into temporary tables
/// (dropped at commit), then deleted in one statement, so foreign keys are
/// checked once the whole workspace is gone and no table order is needed.
async fn delete_rows(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<BTreeMap<String, u64>, AppError> {
    let tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT class.relname::text
        FROM pg_class class
        JOIN pg_namespace namespace ON namespace.oid = class.relnamespace
        WHERE namespace.nspname = current_schema() AND class.relkind IN ('r', 'p')
        "#,
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let walkable = |table: &str| {
        tables.contains(table)
            && (!crate::tenancy::tables::is_global(table)
                || OWNED_GLOBAL_TABLES.iter().any(|(owned, _)| *owned == table))
    };

    let mut primary_keys = BTreeMap::<String, Vec<String>>::new();
    for row in sqlx::query(
        r#"
        SELECT class.relname::text AS table_name,
               array_agg(attribute.attname::text ORDER BY key.position) AS columns
        FROM pg_index index
        JOIN pg_class class ON class.oid = index.indrelid
        JOIN pg_namespace namespace ON namespace.oid = class.relnamespace
        CROSS JOIN LATERAL unnest(index.indkey) WITH ORDINALITY AS key(attnum, position)
        JOIN pg_attribute attribute
          ON attribute.attrelid = class.oid AND attribute.attnum = key.attnum
        WHERE index.indisprimary AND namespace.nspname = current_schema()
        GROUP BY class.relname
        "#,
    )
    .fetch_all(&mut *connection)
    .await?
    {
        primary_keys.insert(row.try_get("table_name")?, row.try_get("columns")?);
    }

    let mut foreign_keys = Vec::new();
    for row in sqlx::query(
        r#"
        SELECT child.relname::text AS child, parent.relname::text AS parent,
               ARRAY(
                   SELECT attribute.attname::text
                   FROM unnest(constraint_row.conkey) WITH ORDINALITY AS key(attnum, position)
                   JOIN pg_attribute attribute
                     ON attribute.attrelid = constraint_row.conrelid
                    AND attribute.attnum = key.attnum
                   ORDER BY key.position
               ) AS child_columns,
               ARRAY(
                   SELECT attribute.attname::text
                   FROM unnest(constraint_row.confkey) WITH ORDINALITY AS key(attnum, position)
                   JOIN pg_attribute attribute
                     ON attribute.attrelid = constraint_row.confrelid
                    AND attribute.attnum = key.attnum
                   ORDER BY key.position
               ) AS parent_columns,
               NOT EXISTS (
                   SELECT 1
                   FROM unnest(constraint_row.conkey) AS key(attnum)
                   JOIN pg_attribute attribute
                     ON attribute.attrelid = constraint_row.conrelid
                    AND attribute.attnum = key.attnum
                   WHERE attribute.attnotnull
               ) AS detachable
        FROM pg_constraint constraint_row
        JOIN pg_class child ON child.oid = constraint_row.conrelid
        JOIN pg_namespace namespace ON namespace.oid = child.relnamespace
        JOIN pg_class parent ON parent.oid = constraint_row.confrelid
        WHERE constraint_row.contype = 'f'
          AND namespace.nspname = current_schema()
          -- A reference that becomes null when its row goes doesn't take
          -- the row holding it along (a DM forwarding a message keeps
          -- itself).
          AND constraint_row.confdeltype NOT IN ('n', 'd')
        "#,
    )
    .fetch_all(&mut *connection)
    .await?
    {
        let key = ForeignKey {
            child: row.try_get("child")?,
            parent: row.try_get("parent")?,
            child_columns: row.try_get("child_columns")?,
            parent_columns: row.try_get("parent_columns")?,
            detachable: row.try_get("detachable")?,
        };
        if walkable(&key.child) && walkable(&key.parent) {
            foreign_keys.push(key);
        }
    }

    // A temporary table of primary keys per table that has rows to go.
    let mut marked = BTreeMap::<String, String>::new();
    async fn mark_table(
        connection: &mut PgConnection,
        marked: &mut BTreeMap<String, String>,
        primary_keys: &BTreeMap<String, Vec<String>>,
        table: &str,
    ) -> Result<Option<String>, AppError> {
        if let Some(temporary) = marked.get(table) {
            return Ok(Some(temporary.clone()));
        }
        let Some(columns) = primary_keys.get(table) else {
            return Ok(None);
        };
        let temporary = format!("aw_workspace_delete_{}", marked.len());
        let list = columns
            .iter()
            .map(|c| ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "CREATE TEMPORARY TABLE {temporary} ON COMMIT DROP AS \
             SELECT {list} FROM {} WHERE false",
            ident(table)
        ))
        .execute(&mut *connection)
        .await?;
        sqlx::query(&format!("CREATE UNIQUE INDEX ON {temporary} ({list})"))
            .execute(&mut *connection)
            .await?;
        marked.insert(table.to_owned(), temporary.clone());
        Ok(Some(temporary))
    }

    // Seeds: the workspace row, rows naming it in their tenant column, and
    // the server-wide rows it owns.
    let mut seeds = vec![("organizations".to_owned(), "id".to_owned())];
    for (table, column) in crate::tenancy::tables::TENANT_TABLES {
        seeds.push(((*table).to_owned(), (*column).to_owned()));
    }
    for (table, column) in OWNED_GLOBAL_TABLES {
        if !column.is_empty() {
            seeds.push(((*table).to_owned(), (*column).to_owned()));
        }
    }
    let mut grown = BTreeSet::<String>::new();
    for (table, column) in seeds {
        if !tables.contains(&table) {
            continue;
        }
        let Some(temporary) =
            mark_table(&mut *connection, &mut marked, &primary_keys, &table).await?
        else {
            continue;
        };
        let list = primary_keys[&table]
            .iter()
            .map(|c| ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let added = sqlx::query(&format!(
            "INSERT INTO {temporary} SELECT {list} FROM {} WHERE {} = $1 ON CONFLICT DO NOTHING",
            ident(&table),
            ident(&column)
        ))
        .bind(workspace)
        .execute(&mut *connection)
        .await?
        .rows_affected();
        if added > 0 {
            grown.insert(table);
        }
    }

    // Follow references until nothing new turns up.
    while !grown.is_empty() {
        let mut next = BTreeSet::new();
        for key in foreign_keys
            .iter()
            .filter(|key| grown.contains(&key.parent))
        {
            let Some(parent_temporary) = marked.get(&key.parent).cloned() else {
                continue;
            };
            let Some(child_temporary) =
                mark_table(&mut *connection, &mut marked, &primary_keys, &key.child).await?
            else {
                continue;
            };
            let child_list = primary_keys[&key.child]
                .iter()
                .map(|c| format!("child.{}", ident(c)))
                .collect::<Vec<_>>()
                .join(", ");
            let reference = key
                .child_columns
                .iter()
                .zip(&key.parent_columns)
                .map(|(child, parent)| format!("child.{} = parent.{}", ident(child), ident(parent)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let gone = primary_keys[&key.parent]
                .iter()
                .map(|c| format!("parent.{column} = gone.{column}", column = ident(c)))
                .collect::<Vec<_>>()
                .join(" AND ");
            // A row of another workspace that can let go of the reference
            // stays; it is detached below, just before the delete.
            let own_only = match tenant_column(&key.child) {
                Some(column) if key.detachable => {
                    format!("WHERE child.{} = $1", ident(column))
                }
                _ => "WHERE $1::uuid IS NOT NULL".to_owned(),
            };
            let added = sqlx::query(&format!(
                "INSERT INTO {child_temporary} SELECT {child_list} FROM {} child \
                 JOIN {} parent ON {reference} \
                 JOIN {parent_temporary} gone ON {gone} \
                 {own_only} \
                 ON CONFLICT DO NOTHING",
                ident(&key.child),
                ident(&key.parent),
            ))
            .bind(workspace)
            .execute(&mut *connection)
            .await?
            .rows_affected();
            if added > 0 {
                next.insert(key.child.clone());
            }
        }
        grown = next;
    }

    // Rows of other workspaces that point at a row going let go of it: a
    // request one workspace raised through another's host keeps itself.
    for key in &foreign_keys {
        let (Some(column), true) = (tenant_column(&key.child), key.detachable) else {
            continue;
        };
        let Some(parent_temporary) = marked.get(&key.parent) else {
            continue;
        };
        let clear = key
            .child_columns
            .iter()
            .map(|c| format!("{} = NULL", ident(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let reference = key
            .child_columns
            .iter()
            .zip(&key.parent_columns)
            .map(|(child, parent)| format!("child.{} = parent.{}", ident(child), ident(parent)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let gone = primary_keys[&key.parent]
            .iter()
            .map(|c| format!("parent.{column} = gone.{column}", column = ident(c)))
            .collect::<Vec<_>>()
            .join(" AND ");
        sqlx::query(&format!(
            "UPDATE {} child SET {clear} FROM {} parent \
             JOIN {parent_temporary} gone ON {gone} \
             WHERE {reference} AND child.{} IS DISTINCT FROM $1",
            ident(&key.child),
            ident(&key.parent),
            ident(column),
        ))
        .bind(workspace)
        .execute(&mut *connection)
        .await?;
    }

    // One statement: every table's rows at once.
    let mut parts = Vec::new();
    let mut names = Vec::new();
    for (index, (table, temporary)) in marked.iter().enumerate() {
        let matching = primary_keys[table]
            .iter()
            .map(|c| format!("target.{column} = gone.{column}", column = ident(c)))
            .collect::<Vec<_>>()
            .join(" AND ");
        parts.push(format!(
            "d{index} AS (DELETE FROM {} target USING {temporary} gone WHERE {matching} \
             RETURNING 1)",
            ident(table)
        ));
        names.push((table.clone(), format!("d{index}")));
    }
    if parts.is_empty() {
        return Ok(BTreeMap::new());
    }
    let counts = names
        .iter()
        .map(|(table, name)| {
            format!(
                "SELECT '{}'::text AS table_name, (SELECT count(*) FROM {name}) AS removed",
                table.replace('\'', "''")
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let statement = format!("WITH {} {counts}", parts.join(", "));
    let mut removed = BTreeMap::new();
    for row in sqlx::query(&statement).fetch_all(&mut *connection).await? {
        let count: i64 = row.try_get("removed")?;
        if count > 0 {
            removed.insert(
                row.try_get::<String, _>("table_name")?,
                u64::try_from(count).unwrap_or(0),
            );
        }
    }
    Ok(removed)
}

/// What an operator's lifecycle command does.
#[derive(Clone, Copy, Debug)]
pub enum OperatorAction {
    Suspend(SuspendReason),
    Resume,
}

/// The `workspace-suspend` and `workspace-resume` commands: an operator
/// stops or restarts a workspace by its address.
pub async fn operator_command(
    pool: &sqlx::PgPool,
    slug: &str,
    action: OperatorAction,
) -> Result<Uuid, AppError> {
    tenancy::system("workspace.operator", async {
        crate::db::scoped_one(pool, async |connection| {
            let workspace =
                sqlx::query_scalar::<_, Uuid>("SELECT id FROM organizations WHERE slug = $1")
                    .bind(slug.trim().to_ascii_lowercase())
                    .fetch_optional(&mut *connection)
                    .await?
                    .ok_or(AppError::NotFound("workspace"))?;
            match action {
                OperatorAction::Suspend(reason) => suspend(connection, workspace, reason).await?,
                OperatorAction::Resume => resume(connection, workspace).await?,
            }
            Ok::<_, AppError>(workspace)
        })
        .await
    })
    .await
}
