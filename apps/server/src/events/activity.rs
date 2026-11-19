//! Activity from the log (K3.20): what happened to a thing, newest first,
//! read from its facts in `events`, with who did it, through which app and
//! for whom. `activity_list` answers it for any thing the caller can see;
//! the workspace's audit trail (`audit_log`) reads the same facts beside
//! `audit_events`.
//!
//! Events carry no text, so neither does an entry: its type, its summary
//! from the catalogue, the keys of what changed and the version it made.
//! An event about a conversation the reader can't see is left out
//! (`conversation_visible_to`); the thing itself was checked first.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// The most entries one read returns.
pub const MAX_ACTIVITY: i64 = 200;

/// One thing that happened.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// Its place in the log: pass the last one as `before` for older
    /// entries.
    pub position: i64,
    pub at: DateTime<Utc>,
    /// The event's type, such as `task.updated`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// In a few words, from the catalogue: "A task changed".
    pub summary: String,
    /// Who did it.
    #[schemars(with = "Option<String>")]
    pub actor_id: Option<Uuid>,
    /// Their name in the workspace.
    pub actor: Option<String>,
    /// The connected app they acted through, when it shows itself.
    pub via: Option<String>,
    /// Who asked for it, when someone asked an agent.
    #[schemars(with = "Option<String>")]
    pub requested_by_id: Option<Uuid>,
    pub requested_by: Option<String>,
    /// Keys of what changed ("title", "status", a field id).
    pub changed: Vec<String>,
    /// The version it made, where the thing keeps versions.
    pub version: Option<i64>,
}

#[derive(FromRow)]
struct Row {
    position: i64,
    created_at: DateTime<Utc>,
    #[sqlx(rename = "type")]
    event_type: String,
    actor_id: Option<Uuid>,
    actor: Option<String>,
    via: Option<String>,
    requested_by_id: Option<Uuid>,
    requested_by: Option<String>,
    changes: Vec<String>,
    thing_version: Option<i64>,
}

/// A name for an account in a workspace: `$1` is the workspace.
pub(crate) fn name_of(column: &str) -> String {
    format!(
        "(SELECT COALESCE(profile.display_name, account.display_name)
            FROM actors who
            LEFT JOIN organization_actor_profiles profile
              ON profile.organization_id = $1 AND profile.actor_id = who.id
            LEFT JOIN users account ON account.actor_id = who.id
           WHERE who.id = {column})"
    )
}

/// The connected app a credential belongs to, when it shows itself.
pub(crate) fn app_of(column: &str) -> String {
    format!(
        "(SELECT client.client_name FROM mcp_oauth_grants grant_row
            JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
           WHERE grant_row.id = {column} AND grant_row.show_via)"
    )
}

/// The audit trail's two sources (`audit_log.rs`) as one `event` relation,
/// for `$1` the workspace: `audit_events` and the log's facts. A fact's
/// action is its type (`task.updated`); its metadata holds the keys of what
/// changed, the version and who asked for it, never content.
pub(crate) fn audit_relation() -> String {
    audit_relation_from("events")
}

/// [`audit_relation`] over `source`: `events`, or
/// [`super::archive::WITH_ARCHIVED`] once a reader has loaded the files it
/// reaches back into.
pub(crate) fn audit_relation_from(source: &str) -> String {
    format!(
        r#"
    WITH event AS (
        SELECT audit.id, audit.organization_id, audit.occurred_at, audit.action,
               audit.outcome, audit.actor_id, audit.project_id, audit.channel,
               audit.client_name, audit.target_kind, audit.target_id, audit.metadata
        FROM audit_events audit
        WHERE audit.organization_id = $1
          AND NOT (audit.outcome = 'succeeded' AND audit.target_id IS NOT NULL
                   AND EXISTS (SELECT 1 FROM {source} fact
                               WHERE fact.thing_id = audit.target_id
                                 AND fact.workspace_id = audit.organization_id
                                 AND fact.class = 'fact'
                                 AND (fact.request_id = audit.request_id
                                      OR fact.created_at = audit.occurred_at)))
        UNION ALL
        SELECT fact.id, fact.workspace_id, fact.created_at, fact.type, 'succeeded',
               fact.actor_account_id, fact.space_id,
               CASE WHEN fact.via_credential_id IS NOT NULL THEN 'mcp' END,
               {app}, COALESCE(fact.thing_kind, 'workspace'), fact.thing_id,
               jsonb_strip_nulls(jsonb_build_object(
                   'changes', to_jsonb(fact.changes),
                   'version', fact.thing_version,
                   'conversation_id', fact.conversation_id,
                   'requestedBy', fact.requested_by_account_id))
        FROM {source} fact
        WHERE fact.workspace_id = $1 AND fact.class = 'fact'
    )
    "#,
        app = app_of("fact.via_credential_id"),
    )
}

/// The facts about `thing` in `workspace` that `reader` may see, newest
/// first, before `before` (a position) when given, read from `source`
/// (`events`, or [`super::archive::WITH_ARCHIVED`]). The caller has already
/// shown that `reader` sees the thing.
pub async fn for_thing(
    connection: &mut PgConnection,
    source: &str,
    workspace: Uuid,
    reader: Uuid,
    thing: Uuid,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<ActivityEntry>, AppError> {
    let rows = sqlx::query_as::<_, Row>(&format!(
        r#"
        SELECT fact.position, fact.created_at, fact.type,
               fact.actor_account_id AS actor_id, {actor} AS actor, {via} AS via,
               fact.requested_by_account_id AS requested_by_id,
               {requested_by} AS requested_by,
               fact.changes, fact.thing_version
        FROM {source} fact
        WHERE fact.workspace_id = $1
          AND fact.thing_id = $2
          AND fact.class = 'fact'
          AND (fact.conversation_id IS NULL OR conversation_visible_to(fact.conversation_id, $3))
          AND ($4::bigint IS NULL OR fact.position < $4)
        ORDER BY fact.position DESC
        LIMIT $5
        "#,
        actor = name_of("fact.actor_account_id"),
        via = app_of("fact.via_credential_id"),
        requested_by = name_of("fact.requested_by_account_id"),
    ))
    .bind(workspace)
    .bind(thing)
    .bind(reader)
    .bind(before)
    .bind(limit.clamp(1, MAX_ACTIVITY))
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ActivityEntry {
            summary: super::catalog::get(&row.event_type)
                .map_or_else(|| row.event_type.clone(), |ty| ty.summary.to_owned()),
            position: row.position,
            at: row.created_at,
            event_type: row.event_type,
            actor_id: row.actor_id,
            actor: row.actor,
            via: row.via,
            requested_by_id: row.requested_by_id,
            requested_by: row.requested_by,
            changed: row.changes,
            version: row.thing_version,
        })
        .collect())
}

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ActivityList>();
}

/// The thing whose activity to read.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityListInput {
    /// The thing: its id or link (a task, page, board, sheet, record,
    /// conversation, run or pull request).
    #[serde(alias = "id")]
    pub thing: IdRef,
    /// Only entries older than this position (the last one of a previous
    /// page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// How many, newest first (default 30, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for ActivityListInput {
    fn target(&self) -> Target {
        Target::thing("thing", self.thing.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityListOutput {
    pub kind: String,
    /// Newest first.
    pub entries: Vec<ActivityEntry>,
}

pub struct ActivityList;

impl Operation for ActivityList {
    const NAME: &'static str = "activity_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List activity";
    const DESCRIPTION: &'static str = "What happened to a thing, newest first: each change \
        with who made it, the app they used, who asked for it and the keys of what changed. \
        No content: open the thing for that, or history_show for an older version.";

    type Input = ActivityListInput;
    type Output = ActivityListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ActivityListInput,
    ) -> Result<ActivityListOutput, OpError> {
        let id = input.thing.id();
        let kind = crate::history::ops::visible_kind(call, id).await?;
        let workspace = call.ctx.workspace_id;
        let limit = i64::from(input.limit.unwrap_or(30)).clamp(1, MAX_ACTIVITY);
        let (entries, archives) = {
            let mut connection = crate::db::scoped_tx(&call.state.pool)
                .await
                .map_err(AppError::from)?;
            let entries = for_thing(
                &mut connection,
                "events",
                workspace,
                call.account_id(),
                id,
                input.before,
                limit,
            )
            .await?;
            // The cold log's files naming the thing (K3.25): every one a
            // short page may go on into, and any reaching into a full one.
            let full = entries.len() as i64 >= limit;
            let oldest = entries.last().map(|entry| entry.position);
            let archives = super::archive::reaching_back(
                &mut connection,
                workspace,
                Some(id),
                input.before,
                None,
            )
            .await?
            .into_iter()
            .filter(|archive| !full || oldest.is_some_and(|oldest| archive.last_position >= oldest))
            .collect::<Vec<_>>();
            (entries, archives)
        };
        if archives.is_empty() {
            return Ok(ActivityListOutput { kind, entries });
        }
        let mut rows = Vec::new();
        for archive in &archives {
            let thing = id.to_string();
            rows.extend(
                super::archive::fetch(call.state, archive)
                    .await?
                    .into_iter()
                    .filter(|row| row.get("thing_id").and_then(|v| v.as_str()) == Some(&thing)),
            );
        }
        let mut connection = crate::db::scoped_tx(&call.state.pool)
            .await
            .map_err(AppError::from)?;
        super::archive::load(&mut connection, rows).await?;
        let entries = for_thing(
            &mut connection,
            super::archive::WITH_ARCHIVED,
            workspace,
            call.account_id(),
            id,
            input.before,
            limit,
        )
        .await?;
        Ok(ActivityListOutput { kind, entries })
    }
}
