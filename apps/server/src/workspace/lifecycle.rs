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

/// The calls a workspace that is deleting still answers.
const ALLOWED_WHILE_DELETING: &[&str] = &["workspace_delete_cancel"];

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
    let id = Uuid::now_v7();
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
    Ok(id)
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
    let lifecycle =
        sqlx::query_scalar::<_, String>("SELECT lifecycle FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_optional(&mut *connection)
            .await?;
    match lifecycle.as_deref() {
        Some("suspended") => Err(AppError::WorkspaceSuspended),
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
    Ok(())
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
    Ok(())
}

fn delete_job_key(workspace: Uuid) -> String {
    format!("{DELETE_JOB_KIND}:{workspace}")
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
    let delete_after = Utc::now() + Duration::days(DELETION_GRACE_DAYS);
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
            .unique(delete_job_key(workspace))
            .run_at(delete_after)
            .max_attempts(20),
    )
    .await?;
    Ok(delete_after)
}

/// Keep a workspace that was going to be deleted.
pub async fn cancel_deletion(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<(), AppError> {
    if lock(connection, workspace).await? != Lifecycle::Deleting {
        return Err(AppError::Conflict("workspace", "not being deleted"));
    }
    sqlx::query(
        r#"
        UPDATE organizations
        SET lifecycle = 'active', lifecycle_changed_at = now(), delete_after = NULL,
            deletion_requested_by_actor_id = NULL, version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    crate::jobs::cancel_queued(&mut *connection, &delete_job_key(workspace)).await?;
    Ok(())
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

pub fn register_jobs(registry: &mut JobRegistry) {
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
    let due = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_scalar::<_, bool>(
            "SELECT lifecycle = 'deleting' AND ($2 OR delete_after <= now()) \
             FROM organizations WHERE id = $1",
        )
        .bind(workspace)
        .bind(ignore_date)
        .fetch_optional(connection)
        .await
    })
    .await?;
    if due != Some(true) {
        return Ok(None);
    }
    // The bytes first: deleting them twice is harmless, and a failure here
    // leaves the rows for the job's next attempt.
    crate::files::store::FileStore::from_config(&state.config)
        .delete_workspace(workspace)
        .await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let connection = &mut *transaction;
    // Checked again under the row lock: an owner may have cancelled.
    let still_due = sqlx::query_scalar::<_, bool>(
        "SELECT lifecycle = 'deleting' AND ($2 OR delete_after <= now()) \
         FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(workspace)
    .bind(ignore_date)
    .fetch_optional(&mut *connection)
    .await?;
    if still_due != Some(true) {
        return Ok(None);
    }
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
    let rows = delete_rows(connection, workspace).await?;
    sqlx::query(
        "INSERT INTO deleted_workspaces (id, requested_by_actor_id) VALUES ($1, $2) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace)
    .bind(requested_by)
    .execute(&mut *connection)
    .await?;
    transaction.commit().await?;
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
}

/// Server-wide tables the deletion walks into: the hosts the workspace owns
/// and what hangs off them. Every other server-wide table is never
/// touched (accounts stay, R19).
const OWNED_GLOBAL_TABLES: &[(&str, &str)] = &[
    ("hosts", "owner_workspace_id"),
    ("host_enrolments", "owner_workspace_id"),
    ("host_agents", ""),
    ("host_notices", ""),
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
               ) AS parent_columns
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
            let added = sqlx::query(&format!(
                "INSERT INTO {child_temporary} SELECT {child_list} FROM {} child \
                 JOIN {} parent ON {reference} \
                 JOIN {parent_temporary} gone ON {gone} \
                 ON CONFLICT DO NOTHING",
                ident(&key.child),
                ident(&key.parent),
            ))
            .execute(&mut *connection)
            .await?
            .rows_affected();
            if added > 0 {
                next.insert(key.child.clone());
            }
        }
        grown = next;
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
