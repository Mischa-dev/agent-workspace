//! Outside data as operations (T1.34, T1.35): connections (list, make,
//! change, remove), saved queries (list, read with their stored answer,
//! make, change, review, share, run, remove). Connections and query text
//! are for roles with `data.connect`; a shared, reviewed query's answer is
//! everyone's in the space.

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    Connection, DEFAULT_ROW_CAP, MAX_CONNECTIONS, MAX_DESCRIPTION_CHARACTERS, MAX_NAME_CHARACTERS,
    MAX_QUERIES, MAX_ROW_CAP, MAX_STATEMENT_CHARACTERS, Parameter, Query, ReviewState, Stored,
    run::{self, Requested},
};
use crate::{
    access::{self, Need, Target as AccessTarget},
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        error::{ConflictReason, DenyReason},
        refs::IdRef,
    },
    outbound::DatabaseAddress,
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

const MODULE: &str = "collections";
/// Rows of an answer a list shows; a read shows them all.
const LISTED_ROWS: usize = 0;
/// A run by someone without data.connect is skipped when the answer is
/// this fresh.
const FRESH_SECONDS: i64 = 60;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<DataConnectionList>()
        .add::<DataConnectionCreate>()
        .add::<DataConnectionUpdate>()
        .add::<DataConnectionDelete>()
        .add::<DataQueryList>()
        .add::<DataQueryGet>()
        .add::<DataQueryCreate>()
        .add::<DataQueryUpdate>()
        .add::<DataQueryReview>()
        .add::<DataQueryShare>()
        .add::<DataQueryRun>()
        .add::<DataQueryDelete>();
}

fn invalid(field: &'static str, message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Invalid {
        field: Cow::Borrowed(field),
        message: message.into(),
    }
}

fn conflict(message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State {
            code: "outside_data",
        },
        message: message.into(),
        current: None,
    }
}

fn version_conflict(
    resource: &'static str,
    current: i64,
    expected: Option<i64>,
) -> Result<(), OpError> {
    match expected {
        Some(expected) if expected != current => Err(AppError::VersionConflict {
            resource,
            current_version: current,
        }
        .into()),
        _ => Ok(()),
    }
}

/// Whether the caller's role holds `permission` in the call's space.
async fn holds(call: &mut OpCall<'_>, permission: Perm) -> Result<bool, OpError> {
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    Ok(access::check(
        tx,
        &caller,
        Need::permission(permission),
        AccessTarget::Space(space),
    )
    .await?
    .is_ok())
}

fn one_line(field: &'static str, value: &str, what: &str) -> Result<String, OpError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_NAME_CHARACTERS
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            field,
            format!(
                "Give the {what} a name of 1 to {MAX_NAME_CHARACTERS} characters, on one line."
            ),
        ));
    }
    Ok(value.to_owned())
}

fn description_of(value: &str) -> Result<String, OpError> {
    if value.chars().count() > MAX_DESCRIPTION_CHARACTERS {
        return Err(invalid(
            "description",
            format!("A description holds at most {MAX_DESCRIPTION_CHARACTERS} characters."),
        ));
    }
    Ok(value.trim().to_owned())
}

fn address_of(value: &str) -> Result<DatabaseAddress, OpError> {
    if value.len() > super::MAX_ADDRESS_CHARACTERS {
        return Err(invalid("address", "That address is too long."));
    }
    DatabaseAddress::parse(value).map_err(|why| invalid("address", why))
}

fn statement_of(value: &str, parameters: &[Parameter]) -> Result<String, OpError> {
    if value.trim().is_empty() || value.chars().count() > MAX_STATEMENT_CHARACTERS {
        return Err(invalid(
            "statement",
            format!("A query is 1 to {MAX_STATEMENT_CHARACTERS} characters of SQL."),
        ));
    }
    super::check_parameters(parameters).map_err(|why| invalid("parameters", why))?;
    super::prepare(value, parameters).map_err(|why| invalid("statement", why))?;
    Ok(value.trim().to_owned())
}

fn row_cap_of(value: Option<i32>) -> Result<i32, OpError> {
    let cap = value.unwrap_or(DEFAULT_ROW_CAP);
    if !(1..=MAX_ROW_CAP).contains(&cap) {
        return Err(invalid(
            "rowCap",
            format!("A query keeps 1 to {MAX_ROW_CAP} rows."),
        ));
    }
    Ok(cap)
}

fn secret_name(connection_id: Uuid) -> String {
    format!("data_connection:{connection_id}")
}

async fn store_address(
    call: &mut OpCall<'_>,
    connection_id: Uuid,
    address: &str,
) -> Result<Uuid, OpError> {
    let keys = secrets::keyring(&call.state.config).map_err(|_| OpError::Unavailable)?;
    let owner = SecretOwner::Project {
        organization_id: call.ctx.workspace_id,
        project_id: call.ctx.space_id.ok_or(OpError::Unavailable)?,
    };
    let me = call.ctx.caller.account_id;
    let name = secret_name(connection_id);
    let value = SecretValue::new(address.trim());
    let tx = call.tx().await?;
    let stored = secrets::put(
        tx,
        keys.as_ref(),
        NewSecret {
            owner,
            name: &name,
            description: Some("An outside database's address"),
            value: &value,
            actor_id: Some(me),
            system_managed: true,
        },
    )
    .await?;
    Ok(stored.id)
}

// ---------------------------------------------------------------------------
// Entries

/// A connection as someone who connects data sees it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// The protocol it speaks: postgresql.
    pub kind: String,
    /// user@host:port/database; the password is never shown.
    pub address: String,
    /// Saved queries that run on it.
    pub queries: i64,
    pub version: i64,
    pub updated_at: String,
}

fn connection_entry(connection: &Connection, queries: i64) -> ConnectionEntry {
    ConnectionEntry {
        id: connection.id,
        name: connection.name.clone(),
        kind: connection.kind.clone(),
        address: connection.address.clone(),
        queries,
        version: connection.version,
        updated_at: connection.updated_at.to_rfc3339(),
    }
}

/// A query's stored answer.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnswerEntry {
    /// From a run while the query was reviewed (the one a shared query
    /// shows), or a draft run.
    pub reviewed: bool,
    /// A run is under way; the answer here is the last one until it ends.
    pub running: bool,
    /// When the last run ended; null before the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ran_at: Option<String>,
    /// The parameter values it ran with.
    pub parameters: Value,
    pub columns: Vec<String>,
    /// Each row's values, in column order. Left out of a list.
    pub rows: Vec<Value>,
    pub row_count: i32,
    /// More rows came back than the query keeps (its row cap, or the size
    /// an answer may take): the first ones are here.
    pub truncated: bool,
    /// Why the last run failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i32>,
}

pub fn answer_entry(stored: Stored, full: bool, rows: Option<usize>) -> AnswerEntry {
    let mut all = match stored.rows {
        Value::Array(rows) => rows,
        _ => Vec::new(),
    };
    if let Some(most) = rows {
        all.truncate(most);
    }
    AnswerEntry {
        reviewed: stored.reviewed,
        running: stored.pending_since.is_some(),
        ran_at: stored.ran_at.map(|at| at.to_rfc3339()),
        parameters: stored.parameters,
        columns: serde_json::from_value(stored.columns).unwrap_or_default(),
        rows: all,
        row_count: stored.row_count,
        truncated: stored.truncated,
        // The database's own words are for whoever manages the query.
        error: stored.error.map(|error| {
            if full {
                error
            } else {
                "The last run didn't finish.".to_owned()
            }
        }),
        duration_ms: stored.duration_ms,
    }
}

/// A saved query as the reader sees it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// draft or reviewed. A change to what it runs makes it a draft again.
    pub review: ReviewState,
    /// Who last reviewed it, and when.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub reviewed_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<String>,
    /// Its reviewed answer is everyone's in the space.
    pub shared: bool,
    /// Its connection, text and row cap: for roles that connect data.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub connection: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    pub parameters: Vec<Parameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_cap: Option<i32>,
    /// Whether you may run it (a draft needs a role that connects data).
    pub can_run: bool,
    pub version: i64,
    pub updated_at: String,
    /// The answer you see: the latest run with data.connect, else the last
    /// reviewed run of a shared query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<AnswerEntry>,
}

fn query_entry(query: &Query, full: bool, answer: Option<AnswerEntry>) -> QueryEntry {
    QueryEntry {
        id: query.id,
        name: query.name.clone(),
        description: query.description.clone(),
        review: query.review,
        reviewed_by: query.reviewed_by_actor_id,
        reviewed_at: query.reviewed_at.map(|at| at.to_rfc3339()),
        shared: query.shared,
        connection: full.then_some(query.connection_id),
        statement: full.then(|| query.statement.clone()),
        parameters: query.parameters.clone(),
        row_cap: full.then_some(query.row_cap),
        can_run: full || (query.shared && query.review == ReviewState::Reviewed),
        version: query.version,
        updated_at: query.updated_at.to_rfc3339(),
        answer,
    }
}

async fn read_connection(
    call: &mut OpCall<'_>,
    id: Uuid,
    lock: bool,
) -> Result<Connection, OpError> {
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let tx = call.tx().await?;
    super::connection(tx, space, id, lock)
        .await?
        .ok_or_else(|| OpError::not_found("That connection"))
}

/// The query, if the caller sees it: all of a space's with data.connect,
/// else the shared ones.
async fn read_query(
    call: &mut OpCall<'_>,
    id: Uuid,
    full: bool,
    lock: bool,
) -> Result<Query, OpError> {
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let tx = call.tx().await?;
    super::query(tx, space, id, lock)
        .await?
        .filter(|query| full || query.shared)
        .ok_or_else(|| OpError::not_found("That query"))
}

async fn with_answer(
    call: &mut OpCall<'_>,
    query: &Query,
    full: bool,
    rows: Option<usize>,
) -> Result<QueryEntry, OpError> {
    let tx = call.tx().await?;
    let answers = super::stored(tx, query.id).await?;
    let answer =
        super::answer_for(query, answers, full).map(|stored| answer_entry(stored, full, rows));
    Ok(query_entry(query, full, answer))
}

/// Audits a write and says what changed (ids only).
async fn changed(
    call: &mut OpCall<'_>,
    kind: &'static str,
    id: Uuid,
    action: &str,
) -> Result<(), OpError> {
    let workspace = call.ctx.workspace_id;
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let actor = call.ctx.caller.account_id;
    let tx = call.tx().await?;
    super::emit_changed(tx, workspace, space, kind, id, Some(actor)).await?;
    call.audit_target(kind, id, json!({ "action": action }));
    Ok(())
}

// ---------------------------------------------------------------------------
// data_connection_list

pub struct DataConnectionList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceInput {
    /// The project: its id or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
}

impl Targeted for SpaceInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataConnectionListOutput {
    pub connections: Vec<ConnectionEntry>,
}

impl Operation for DataConnectionList {
    const NAME: &'static str = "data_connection_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List outside data connections";
    const DESCRIPTION: &'static str = "List a project's connections to databases kept elsewhere: \
        each one's name and where it points (never its password), and how many saved queries \
        run on it. Needs a role that connects outside data.";

    type Input = SpaceInput;
    type Output = DataConnectionListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SpaceInput,
    ) -> Result<DataConnectionListOutput, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let connections = super::connections(tx, space).await?;
        let counts = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT connection_id, count(*) FROM data_queries WHERE space_id = $1 \
             GROUP BY connection_id",
        )
        .bind(space)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(DataConnectionListOutput {
            connections: connections
                .iter()
                .map(|connection| {
                    let count = counts
                        .iter()
                        .find(|(id, _)| *id == connection.id)
                        .map_or(0, |(_, count)| *count);
                    connection_entry(connection, count)
                })
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// data_connection_create

pub struct DataConnectionCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataConnectionCreateInput {
    /// The project: its id or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    pub name: String,
    /// postgresql://user:password@host:port/database, optionally with
    /// ?sslmode=require. Kept as a secret; connect as a user that can only
    /// read.
    pub address: String,
}

impl Targeted for DataConnectionCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for DataConnectionCreate {
    const NAME: &'static str = "data_connection_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_connection.changed"];
    // An automation would keep the address, password and all, in its rule.
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Connect outside data";
    const DESCRIPTION: &'static str = "Connect a project to a database kept elsewhere \
        (postgresql://user:password@host:port/database). The address is kept as a secret and \
        never shown again; queries on it only read. Needs a role that connects outside data.";

    type Input = DataConnectionCreateInput;
    type Output = ConnectionEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataConnectionCreateInput,
    ) -> Result<ConnectionEntry, OpError> {
        let name = one_line("name", &input.name, "connection")?;
        let address = address_of(&input.address)?;
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let me = call.ctx.caller.account_id;
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        let (count, taken) = sqlx::query_as::<_, (i64, bool)>(
            "SELECT count(*), coalesce(bool_or(lower(name) = lower($2)), false) \
             FROM data_connections WHERE space_id = $1",
        )
        .bind(space)
        .bind(&name)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if taken {
            return Err(conflict(
                "Another connection here already has that name. Pick another.",
            ));
        }
        if count >= MAX_CONNECTIONS {
            return Err(conflict(format!(
                "This project has {MAX_CONNECTIONS} connections already. Remove one first."
            )));
        }
        sqlx::query(
            "INSERT INTO data_connections \
                 (id, workspace_id, space_id, name, kind, address, created_by_actor_id) \
             VALUES ($1, $2, $3, $4, 'postgresql', $5, $6)",
        )
        .bind(id)
        .bind(workspace)
        .bind(space)
        .bind(&name)
        .bind(address.shown())
        .bind(me)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let secret = store_address(call, id, &input.address).await?;
        let tx = call.tx().await?;
        sqlx::query("UPDATE data_connections SET secret_id = $2 WHERE id = $1")
            .bind(id)
            .bind(secret)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        let connection = super::connection(tx, space, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(call, "data_connection", id, "created").await?;
        Ok(connection_entry(&connection, 0))
    }
}

// ---------------------------------------------------------------------------
// data_connection_update

pub struct DataConnectionUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataConnectionUpdateInput {
    /// The connection, by id.
    pub connection: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new address. Every query on the connection becomes a draft again,
    /// to be reviewed before it is shared or run by others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

impl Targeted for DataConnectionUpdateInput {
    fn target(&self) -> Target {
        Target::thing("data_connection", self.connection.id())
    }
}

impl Operation for DataConnectionUpdate {
    const NAME: &'static str = "data_connection_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_connection.changed"];
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Change an outside data connection";
    const DESCRIPTION: &'static str = "Rename a connection, or point it at a new address (its \
        queries then become drafts until reviewed again). Needs a role that connects outside \
        data.";

    type Input = DataConnectionUpdateInput;
    type Output = ConnectionEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataConnectionUpdateInput,
    ) -> Result<ConnectionEntry, OpError> {
        let name = input
            .name
            .as_deref()
            .map(|name| one_line("name", name, "connection"))
            .transpose()?;
        let address = input.address.as_deref().map(address_of).transpose()?;
        let current = read_connection(call, input.connection.id(), true).await?;
        version_conflict("data_connection", current.version, input.expected_version)?;
        let space = current.space_id;
        if let Some(name) = &name {
            let tx = call.tx().await?;
            let taken = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM data_connections \
                 WHERE space_id = $1 AND id <> $2 AND lower(name) = lower($3))",
            )
            .bind(space)
            .bind(current.id)
            .bind(name)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if taken {
                return Err(conflict(
                    "Another connection here already has that name. Pick another.",
                ));
            }
        }
        let secret = match (&address, &input.address) {
            (Some(_), Some(raw)) => Some(store_address(call, current.id, raw).await?),
            _ => None,
        };
        let me = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        sqlx::query(
            "UPDATE data_connections SET name = coalesce($2, name), \
                 address = coalesce($3, address), secret_id = coalesce($4, secret_id), \
                 version = version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(current.id)
        .bind(name.as_deref())
        .bind(address.as_ref().map(DatabaseAddress::shown))
        .bind(secret)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if address.is_some() {
            sqlx::query(
                "UPDATE data_queries SET review_state = 'draft', version = version + 1, \
                     updated_by_actor_id = $2, updated_at = now() \
                 WHERE connection_id = $1 AND review_state = 'reviewed'",
            )
            .bind(current.id)
            .bind(me)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
        let connection = super::connection(tx, space, current.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        let queries = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM data_queries WHERE connection_id = $1",
        )
        .bind(current.id)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        changed(call, "data_connection", current.id, "updated").await?;
        Ok(connection_entry(&connection, queries))
    }
}

// ---------------------------------------------------------------------------
// data_connection_delete

pub struct DataConnectionDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataConnectionDeleteInput {
    /// The connection, by id.
    pub connection: IdRef,
}

impl Targeted for DataConnectionDeleteInput {
    fn target(&self) -> Target {
        Target::thing("data_connection", self.connection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovedOutput {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub removed: bool,
}

impl Operation for DataConnectionDelete {
    const NAME: &'static str = "data_connection_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_connection.changed"];
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove an outside data connection";
    const DESCRIPTION: &'static str = "Remove a connection and its kept address. A connection \
        that saved queries still run on stays: remove or move those first.";

    type Input = DataConnectionDeleteInput;
    type Output = RemovedOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataConnectionDeleteInput,
    ) -> Result<RemovedOutput, OpError> {
        let current = read_connection(call, input.connection.id(), true).await?;
        let tx = call.tx().await?;
        let used = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM data_queries WHERE connection_id = $1",
        )
        .bind(current.id)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if used > 0 {
            return Err(conflict(format!(
                "{used} saved {} still run on this connection. Remove them, or point them at \
                 another connection, first.",
                if used == 1 { "query" } else { "queries" }
            )));
        }
        sqlx::query("DELETE FROM data_connections WHERE id = $1")
            .bind(current.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        if let Some(secret) = current.secret_id {
            secrets::delete(&mut **tx, secret).await?;
        }
        changed(call, "data_connection", current.id, "removed").await?;
        Ok(RemovedOutput {
            id: current.id,
            removed: true,
        })
    }
}

// ---------------------------------------------------------------------------
// data_query_list

pub struct DataQueryList;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataQueryListOutput {
    /// With data.connect every saved query; otherwise the shared ones. Each
    /// answer without its rows (data_query_get reads them).
    pub queries: Vec<QueryEntry>,
    /// Whether you can connect data, write and review queries here.
    pub can_connect: bool,
    /// Whether you can share a reviewed query's answer.
    pub can_share: bool,
}

impl Operation for DataQueryList {
    const NAME: &'static str = "data_query_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List saved outside data queries";
    const DESCRIPTION: &'static str = "List a project's saved queries over outside data, each \
        with whether it's reviewed and shared, and its last answer's size. With a role that \
        connects data, every query and its SQL; otherwise the shared ones.";

    type Input = SpaceInput;
    type Output = DataQueryListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SpaceInput,
    ) -> Result<DataQueryListOutput, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let full = holds(call, Perm::DataConnect).await?;
        let can_share = holds(call, Perm::DataShare).await?;
        let tx = call.tx().await?;
        let queries = super::queries(tx, space, !full).await?;
        let mut entries = Vec::with_capacity(queries.len());
        for query in &queries {
            entries.push(with_answer(call, query, full, Some(LISTED_ROWS)).await?);
        }
        Ok(DataQueryListOutput {
            queries: entries,
            can_connect: full,
            can_share,
        })
    }
}

// ---------------------------------------------------------------------------
// data_query_get

pub struct DataQueryGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryInput {
    /// The saved query, by id.
    pub query: IdRef,
}

impl Targeted for QueryInput {
    fn target(&self) -> Target {
        Target::thing("data_query", self.query.id())
    }
}

impl Operation for DataQueryGet {
    const NAME: &'static str = "data_query_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read a saved outside data query";
    const DESCRIPTION: &'static str = "Read a saved query and its stored answer: its columns and \
        rows, when it ran, and whether rows past its cap were left out. Without a role that \
        connects data, only a shared query, its last reviewed answer and no SQL.";

    type Input = QueryInput;
    type Output = QueryEntry;

    async fn run(call: &mut OpCall<'_>, input: QueryInput) -> Result<QueryEntry, OpError> {
        let full = holds(call, Perm::DataConnect).await?;
        let query = read_query(call, input.query.id(), full, false).await?;
        with_answer(call, &query, full, None).await
    }
}

// ---------------------------------------------------------------------------
// data_query_create

pub struct DataQueryCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryCreateInput {
    /// The project: its id or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// The connection it runs on, by id (data_connection_list).
    pub connection: IdRef,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// One SELECT. {{name}} marks a parameter, bound as a value (never
    /// pasted in).
    pub statement: String,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    /// Rows kept of an answer, 1 to 5,000 (1,000 when left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_cap: Option<i32>,
}

impl Targeted for DataQueryCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for DataQueryCreate {
    const NAME: &'static str = "data_query_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed"];
    const TITLE: &'static str = "Save an outside data query";
    const DESCRIPTION: &'static str = "Save a read-only query over an outside data connection, \
        with {{name}} parameters. It starts as a draft: only roles that connect data run it until \
        someone reviews it (data_query_review). Run it with data_query_run.";

    type Input = DataQueryCreateInput;
    type Output = QueryEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataQueryCreateInput,
    ) -> Result<QueryEntry, OpError> {
        let name = one_line("name", &input.name, "query")?;
        let description = description_of(input.description.as_deref().unwrap_or(""))?;
        let statement = statement_of(&input.statement, &input.parameters)?;
        let row_cap = row_cap_of(input.row_cap)?;
        let connection = read_connection(call, input.connection.id(), false).await?;
        let space = connection.space_id;
        let workspace = call.ctx.workspace_id;
        let me = call.ctx.caller.account_id;
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        let (count, taken) = sqlx::query_as::<_, (i64, bool)>(
            "SELECT count(*), coalesce(bool_or(lower(name) = lower($2)), false) \
             FROM data_queries WHERE space_id = $1",
        )
        .bind(space)
        .bind(&name)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if taken {
            return Err(conflict(
                "Another query here already has that name. Pick another.",
            ));
        }
        if count >= MAX_QUERIES {
            return Err(conflict(format!(
                "This project has {MAX_QUERIES} saved queries already. Remove one first."
            )));
        }
        sqlx::query(
            "INSERT INTO data_queries (id, workspace_id, space_id, connection_id, name, \
                 description, statement, parameters, row_cap, created_by_actor_id, \
                 updated_by_actor_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
        )
        .bind(id)
        .bind(workspace)
        .bind(space)
        .bind(connection.id)
        .bind(&name)
        .bind(&description)
        .bind(&statement)
        .bind(json!(input.parameters))
        .bind(row_cap)
        .bind(me)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let query = super::query(tx, space, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(call, "data_query", id, "created").await?;
        Ok(query_entry(&query, true, None))
    }
}

// ---------------------------------------------------------------------------
// data_query_update

pub struct DataQueryUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryUpdateInput {
    /// The saved query, by id.
    pub query: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A change to the SQL, parameters, row cap or connection makes a
    /// reviewed query a draft again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<Parameter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_cap: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<IdRef>,
}

impl Targeted for DataQueryUpdateInput {
    fn target(&self) -> Target {
        Target::thing("data_query", self.query.id())
    }
}

impl Operation for DataQueryUpdate {
    const NAME: &'static str = "data_query_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed"];
    const TITLE: &'static str = "Change a saved outside data query";
    const DESCRIPTION: &'static str = "Change a saved query's name, description, SQL, \
        parameters, row cap or connection. Changing what it runs makes it a draft again; its \
        shared answer stays the last reviewed one until it is reviewed and run.";

    type Input = DataQueryUpdateInput;
    type Output = QueryEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataQueryUpdateInput,
    ) -> Result<QueryEntry, OpError> {
        let current = read_query(call, input.query.id(), true, true).await?;
        version_conflict("data_query", current.version, input.expected_version)?;
        let name = input
            .name
            .as_deref()
            .map(|name| one_line("name", name, "query"))
            .transpose()?;
        let description = input
            .description
            .as_deref()
            .map(description_of)
            .transpose()?;
        let parameters = input
            .parameters
            .clone()
            .unwrap_or(current.parameters.clone());
        let statement = statement_of(
            input.statement.as_deref().unwrap_or(&current.statement),
            &parameters,
        )?;
        let row_cap = match input.row_cap {
            Some(cap) => row_cap_of(Some(cap))?,
            None => current.row_cap,
        };
        let connection = match input.connection {
            Some(connection) => read_connection(call, connection.id(), false).await?.id,
            None => current.connection_id,
        };
        let changes_what_runs = statement != current.statement
            || parameters != current.parameters
            || row_cap != current.row_cap
            || connection != current.connection_id;
        let space = current.space_id;
        let me = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        if let Some(name) = &name {
            let taken = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM data_queries \
                 WHERE space_id = $1 AND id <> $2 AND lower(name) = lower($3))",
            )
            .bind(space)
            .bind(current.id)
            .bind(name)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if taken {
                return Err(conflict(
                    "Another query here already has that name. Pick another.",
                ));
            }
        }
        sqlx::query(
            "UPDATE data_queries SET name = coalesce($2, name), \
                 description = coalesce($3, description), statement = $4, parameters = $5, \
                 row_cap = $6, connection_id = $7, \
                 review_state = CASE WHEN $8 THEN 'draft' ELSE review_state END, \
                 updated_by_actor_id = $9, version = version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(current.id)
        .bind(name.as_deref())
        .bind(description.as_deref())
        .bind(&statement)
        .bind(json!(parameters))
        .bind(row_cap)
        .bind(connection)
        .bind(changes_what_runs)
        .bind(me)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let query = super::query(tx, space, current.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(call, "data_query", current.id, "updated").await?;
        with_answer(call, &query, true, Some(LISTED_ROWS)).await
    }
}

// ---------------------------------------------------------------------------
// data_query_review

pub struct DataQueryReview;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryReviewInput {
    /// The saved query, by id.
    pub query: IdRef,
    /// The version you read and reviewed; a change made since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// reviewed, or draft to take a review back.
    pub state: ReviewState,
}

impl Targeted for DataQueryReviewInput {
    fn target(&self) -> Target {
        Target::thing("data_query", self.query.id())
    }
}

impl Operation for DataQueryReview {
    const NAME: &'static str = "data_query_review";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed"];
    const TITLE: &'static str = "Review a saved outside data query";
    const DESCRIPTION: &'static str = "Mark a saved query reviewed (its SQL read and found \
        right), or a draft again. Only a reviewed query can be shared, or run by people who \
        don't connect data. Pass the version you read.";

    type Input = DataQueryReviewInput;
    type Output = QueryEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DataQueryReviewInput,
    ) -> Result<QueryEntry, OpError> {
        let current = read_query(call, input.query.id(), true, true).await?;
        version_conflict("data_query", current.version, input.expected_version)?;
        let me = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        if input.state != current.review {
            sqlx::query(
                "UPDATE data_queries SET review_state = $2, \
                     reviewed_by_actor_id = CASE WHEN $2 = 'reviewed' THEN $3 \
                                                 ELSE reviewed_by_actor_id END, \
                     reviewed_at = CASE WHEN $2 = 'reviewed' THEN now() ELSE reviewed_at END, \
                     version = version + 1, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(current.id)
            .bind(input.state.as_str())
            .bind(me)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
        let query = super::query(tx, current.space_id, current.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(call, "data_query", current.id, input.state.as_str()).await?;
        with_answer(call, &query, true, Some(LISTED_ROWS)).await
    }
}

// ---------------------------------------------------------------------------
// data_query_share

pub struct DataQueryShare;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryShareInput {
    /// The saved query, by id.
    pub query: IdRef,
    /// true shares its reviewed answer with everyone in the project (and
    /// lets them run it again); false stops sharing it.
    pub shared: bool,
}

impl Targeted for DataQueryShareInput {
    fn target(&self) -> Target {
        Target::thing("data_query", self.query.id())
    }
}

impl Operation for DataQueryShare {
    const NAME: &'static str = "data_query_share";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataShare);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed", "dashboard.updated"];
    const TITLE: &'static str = "Share an outside data query's answer";
    const DESCRIPTION: &'static str = "Share a reviewed query's stored answer with everyone in \
        the project, so dashboards show it to people without access to the connection, or stop \
        sharing it. Needs a role that shares outside data.";

    type Input = DataQueryShareInput;
    type Output = QueryEntry;

    async fn run(call: &mut OpCall<'_>, input: DataQueryShareInput) -> Result<QueryEntry, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let current = super::query(tx, space, input.query.id(), true)
            .await?
            .ok_or_else(|| OpError::not_found("That query"))?;
        if input.shared && current.review != ReviewState::Reviewed {
            return Err(conflict(
                "Only a reviewed query is shared. Someone who connects data reviews it first.",
            ));
        }
        if input.shared != current.shared {
            sqlx::query(
                "UPDATE data_queries SET shared = $2, version = version + 1, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(current.id)
            .bind(input.shared)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
            super::announce_answer(tx, space, current.id).await?;
        }
        let query = super::query(tx, space, current.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        let full = holds(call, Perm::DataConnect).await?;
        changed(
            call,
            "data_query",
            current.id,
            if input.shared { "shared" } else { "unshared" },
        )
        .await?;
        with_answer(call, &query, full, Some(LISTED_ROWS)).await
    }
}

// ---------------------------------------------------------------------------
// data_query_run

pub struct DataQueryRun;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataQueryRunInput {
    /// The saved query, by id.
    pub query: IdRef,
    /// Values by parameter name, over its defaults. Only roles that connect
    /// data give values; anyone else runs a shared query with its defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
}

impl Targeted for DataQueryRunInput {
    fn target(&self) -> Target {
        Target::thing("data_query", self.query.id())
    }
}

impl Operation for DataQueryRun {
    const NAME: &'static str = "data_query_run";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed"];
    const TITLE: &'static str = "Run a saved outside data query";
    const DESCRIPTION: &'static str = "Run a saved query again: it runs shortly, read-only, and \
        its answer replaces the stored one (data_query_get reads it; answer.running says it's \
        under way). A draft runs only for roles that connect data; anyone in the project can run \
        a shared, reviewed one.";

    type Input = DataQueryRunInput;
    type Output = QueryEntry;

    async fn run(call: &mut OpCall<'_>, input: DataQueryRunInput) -> Result<QueryEntry, OpError> {
        let full = holds(call, Perm::DataConnect).await?;
        let query = read_query(call, input.query.id(), full, true).await?;
        if !full {
            if query.review != ReviewState::Reviewed {
                return Err(OpError::Denied {
                    reason: DenyReason::Role {
                        permission: Perm::DataConnect,
                    },
                    message: Cow::Borrowed(
                        "This query changed since it was reviewed. Someone who connects data \
                         reviews it before anyone else runs it.",
                    ),
                });
            }
            if input
                .parameters
                .as_ref()
                .is_some_and(|values| !values.is_empty())
            {
                return Err(invalid(
                    "parameters",
                    "Only someone who connects data runs a query with values of their own.",
                ));
            }
        }
        let values = input.parameters.unwrap_or_default();
        for name in values.keys() {
            let Some(parameter) = query.parameters.iter().find(|p| &p.name == name) else {
                return Err(invalid(
                    "parameters",
                    format!("{name} isn't one of the query's parameters."),
                ));
            };
            super::Bound::of(parameter.kind, values.get(name))
                .map_err(|why| invalid("parameters", format!("{name}: {why}.")))?;
        }
        // A run with values of its own lands in the draft place, seen only
        // by those who connect data: the shared, reviewed answer is the
        // query's defaults, and only those.
        let reviewed = query.review == ReviewState::Reviewed && values.is_empty();
        let me = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let fresh = !full
            && sqlx::query_scalar::<_, bool>(&format!(
                "SELECT EXISTS (SELECT 1 FROM data_query_results WHERE query_id = $1 \
                 AND reviewed AND ran_at > now() - interval '{FRESH_SECONDS} seconds')"
            ))
            .bind(query.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
        if !fresh && run::request(tx, &query, reviewed, me, &values).await? == Requested::Queued {
            changed(call, "data_query", query.id, "run").await?;
        }
        with_answer(call, &query, full, Some(LISTED_ROWS)).await
    }
}

// ---------------------------------------------------------------------------
// data_query_delete

pub struct DataQueryDelete;

impl Operation for DataQueryDelete {
    const NAME: &'static str = "data_query_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = MODULE;
    const PERMISSION: Option<Perm> = Some(Perm::DataConnect);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["data_query.changed", "dashboard.updated"];
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove a saved outside data query";
    const DESCRIPTION: &'static str = "Remove a saved query and its stored answers. Dashboard \
        tiles that showed it say it's gone.";

    type Input = QueryInput;
    type Output = RemovedOutput;

    async fn run(call: &mut OpCall<'_>, input: QueryInput) -> Result<RemovedOutput, OpError> {
        let current = read_query(call, input.query.id(), true, true).await?;
        let tx = call.tx().await?;
        sqlx::query("DELETE FROM data_queries WHERE id = $1")
            .bind(current.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        if current.shared {
            super::announce_answer(tx, current.space_id, current.id).await?;
        }
        changed(call, "data_query", current.id, "removed").await?;
        Ok(RemovedOutput {
            id: current.id,
            removed: true,
        })
    }
}
