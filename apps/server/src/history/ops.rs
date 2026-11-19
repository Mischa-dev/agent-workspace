//! `history_list`, `history_show`, `history_restore` (K3.19): one way to
//! read and restore any thing's history. Each names the thing by id alone;
//! the registry says what kind it is.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{HistoryCaps, HistoryEntry, Restorer, Support};
use crate::{
    modules::{state, things as module_things},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        error::ConflictReason, refs::IdRef, target::Located,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<HistoryList>()
        .add::<HistoryShow>()
        .add::<HistoryRestore>();
}

/// The thing's kind, once the caller is shown to see it: through the kind's
/// own resolver where it has one (a conversation's visibility, the modules
/// on in its space, tasks only where the caller may see tasks), so a thing
/// the caller can't see is `not_found` like one that doesn't exist.
pub(crate) async fn visible_kind(call: &OpCall<'_>, id: Uuid) -> Result<String, OpError> {
    let not_found = || OpError::not_found("That thing");
    let state = call.state;
    let account = call.account_id();
    let mut connection = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(crate::error::AppError::from)?;
    let kind = match crate::things::locate(&mut connection, id).await? {
        Some(located) if !located.deleted => located.kind.to_string(),
        _ => return Err(not_found()),
    };
    // Messages have no resolver yet; the target's own check held them to
    // their conversation's visibility.
    if !module_things::KINDS.contains(&kind.as_str()) {
        return Ok(kind);
    }
    let mut projects =
        crate::search::reachable_projects(&state.pool, account, call.ctx.caller.connection_id)
            .await?;
    if let Some(confined) = call.ctx.caller.confined_space {
        projects.retain(|project| *project == confined);
    }
    let task_projects =
        crate::search::projects_showing_tasks(&state.pool, account, &projects).await?;
    let modules = match call.ctx.space_id {
        Some(space) => state::for_project(&mut connection, space, account)
            .await?
            .unwrap_or_default(),
        None => state::anywhere(&mut connection, account).await?,
    };
    let workspaces = state::by_workspace(&mut connection, account).await?;
    let viewer = module_things::Viewer {
        actor_id: account,
        projects: &projects,
        task_projects: &task_projects,
        modules,
        workspaces: &workspaces,
        addresses: crate::addresses::Addresses::of(state),
    };
    module_things::get(&mut connection, &viewer, &kind, id)
        .await?
        .ok_or_else(not_found)?;
    Ok(kind)
}

fn not_supported(kind: &str, what: &str, reason: &str) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State {
            code: "not_supported",
        },
        message: format!("A {kind}'s history can't {what} yet: {reason}.").into(),
        current: None,
    }
}

/// A version whose values weren't kept, or can't be put back now.
fn not_kept(why: impl Into<String>) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State { code: "not_kept" },
        message: why.into().into(),
        current: None,
    }
}

fn adapter_for(kind: &str) -> Result<&'static super::Adapter, OpError> {
    super::adapter(kind)
        .ok_or_else(|| not_supported(kind, "be read", "this kind keeps no history yet"))
}

/// The thing whose history to read.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryListInput {
    /// The thing: its id or link (a page, board, message, record, task or
    /// sheet).
    #[serde(alias = "id")]
    pub thing: IdRef,
    /// Only entries older than this version (from a previous page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// How many, newest first (default 30, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for HistoryListInput {
    fn target(&self) -> Target {
        Target::thing("thing", self.thing.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryListOutput {
    pub kind: String,
    /// What this kind's history can do.
    pub caps: HistoryCaps,
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
}

pub struct HistoryList;

impl Operation for HistoryList {
    const NAME: &'static str = "history_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List history";
    const DESCRIPTION: &'static str = "What happened to a thing, newest first: each version or \
        change, who made it and what it touched. caps says whether history_show can open a \
        version and history_restore can put one back.";

    type Input = HistoryListInput;
    type Output = HistoryListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: HistoryListInput,
    ) -> Result<HistoryListOutput, OpError> {
        let id = input.thing.id();
        let kind = visible_kind(call, id).await?;
        let mut connection = crate::db::scoped_tx(&call.state.pool)
            .await
            .map_err(crate::error::AppError::from)?;
        let entries = super::list(
            &mut connection,
            &kind,
            id,
            call.ctx.workspace_id,
            input.before,
            i64::from(input.limit.unwrap_or(30)),
        )
        .await?;
        Ok(HistoryListOutput {
            caps: super::caps(&kind),
            kind,
            entries,
        })
    }
}

/// One version of a thing.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryShowInput {
    /// The thing: its id or link.
    #[serde(alias = "id")]
    pub thing: IdRef,
    /// The version, as history_list gives it.
    pub version: i64,
}

impl Targeted for HistoryShowInput {
    fn target(&self) -> Target {
        Target::thing("thing", self.thing.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryShowOutput {
    pub kind: String,
    pub version: i64,
    /// The version as it was: a page's title and body, a board's
    /// elements, a message's text.
    pub content: Value,
}

pub struct HistoryShow;

impl Operation for HistoryShow {
    const NAME: &'static str = "history_show";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Show a version";
    const DESCRIPTION: &'static str = "One earlier version of a thing as it was, from \
        history_list. Kinds that keep only changes say so instead.";

    type Input = HistoryShowInput;
    type Output = HistoryShowOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: HistoryShowInput,
    ) -> Result<HistoryShowOutput, OpError> {
        let id = input.thing.id();
        let kind = visible_kind(call, id).await?;
        if let Support::Not(reason) = adapter_for(&kind)?.show {
            return Err(not_supported(&kind, "show a version", reason));
        }
        let mut connection = crate::db::scoped_tx(&call.state.pool)
            .await
            .map_err(crate::error::AppError::from)?;
        let content = if kind == "record" {
            match crate::collections::record_versions::at_version(
                &mut connection,
                id,
                input.version,
            )
            .await?
            {
                None => return Err(OpError::not_found("That version")),
                Some(Err(why)) => return Err(not_kept(why)),
                Some(Ok(earlier)) => earlier.to_json(input.version),
            }
        } else {
            super::show(&mut connection, &kind, id, input.version)
                .await?
                .ok_or_else(|| OpError::not_found("That version"))?
        };
        Ok(HistoryShowOutput {
            kind,
            version: input.version,
            content,
        })
    }
}

/// Put an earlier version back.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRestoreInput {
    /// The thing: its id or link.
    #[serde(alias = "id")]
    pub thing: IdRef,
    /// The version to put back, as history_list gives it.
    pub version: i64,
    /// The thing's version now (a page's version, a board's sceneSeq):
    /// refused if it changed since you read it.
    pub expected_version: i64,
}

impl Targeted for HistoryRestoreInput {
    fn target(&self) -> Target {
        Target::Thing {
            kind: "thing",
            id: self.thing.id(),
            expected_version: Some(self.expected_version),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRestoreOutput {
    pub kind: String,
    /// The version put back.
    pub restored: i64,
    /// The thing's version now.
    pub version: i64,
}

pub struct HistoryRestore;

impl Operation for HistoryRestore {
    const NAME: &'static str = "history_restore";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    // Editing the thing: which permission depends on its kind, checked in
    // `run` by the same check the dispatcher makes.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Restore a version";
    const DESCRIPTION: &'static str = "Put an earlier version of a page, whiteboard, sheet or \
        record back, as a new version (nothing is lost: the current one stays in history). For \
        a record, the fields changed since that version get the values they had then. Pass \
        expectedVersion from the thing as you read it. Other kinds say why they can't yet.";
    const EMITS: &'static [&'static str] = &[
        "page.version_restored",
        "whiteboard.version_restored",
        "sheet.version_restored",
        "record.version_restored",
        "record.updated",
    ];

    type Input = HistoryRestoreInput;
    type Output = HistoryRestoreOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: HistoryRestoreInput,
    ) -> Result<HistoryRestoreOutput, OpError> {
        let id = input.thing.id();
        let kind = visible_kind(call, id).await?;
        if let Support::Not(reason) = adapter_for(&kind)?.restore {
            return Err(not_supported(&kind, "be restored", reason));
        }
        let permission = match kind.as_str() {
            "page" => Perm::PagesEdit,
            "sheet" => Perm::SheetsEdit,
            "record" => Perm::CollectionsEdit,
            _ => Perm::WhiteboardsEdit,
        };
        {
            let located = Located {
                workspace_id: call.ctx.workspace_id,
                space_id: call.ctx.space_id,
                direct: None,
            };
            let mut connection = crate::db::scoped_tx(&call.state.pool)
                .await
                .map_err(crate::error::AppError::from)?;
            crate::ops::access_shim::check(
                &mut connection,
                &call.ctx.caller,
                &call.ctx.target,
                &located,
                Some(permission),
                true,
                Self::TITLE,
            )
            .await?;
        }
        if kind == "record" {
            return restore_record(call, id, input.version, input.expected_version).await;
        }
        let restorer = Restorer {
            page: crate::memory::Writer::from(call.ctx),
            board: crate::whiteboards::Caller::from(call.ctx),
            space_id: call.ctx.space_id,
            workspace_id: call.ctx.workspace_id,
            actor_id: call.account_id(),
        };
        let tx = call.tx().await?;
        let version = super::restore(
            tx,
            &restorer,
            &kind,
            id,
            input.version,
            input.expected_version,
        )
        .await?;
        let audited = match kind.as_str() {
            "page" => "page",
            "sheet" => "sheet",
            _ => "whiteboard",
        };
        call.audit_target(audited, id, json!({ "restoredVersion": input.version }));
        Ok(HistoryRestoreOutput {
            kind,
            restored: input.version,
            version,
        })
    }
}

/// A record's version put back (T1.41): the fields changed since get the
/// values they had then, as one change by the caller.
async fn restore_record(
    call: &mut OpCall<'_>,
    id: Uuid,
    version: i64,
    expected_version: i64,
) -> Result<HistoryRestoreOutput, OpError> {
    use crate::collections::record_versions::{self, Restored};
    let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let workspace_id = call.ctx.workspace_id;
    let actor = call.account_id();
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    let may_build = crate::access::check(
        tx,
        &caller,
        crate::access::Need::permission(Perm::CollectionsBuild),
        crate::access::Target::Space(space_id),
    )
    .await?
    .is_ok();
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(space_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(crate::error::AppError::from)?;
    let restored = record_versions::restore(
        tx,
        crate::work_item_fields::Writer {
            actor_id: actor,
            project_key: &project_key,
        },
        may_build,
        id,
        version,
        expected_version,
    )
    .await?;
    let now = match restored {
        Restored::Done(now) => now,
        Restored::NotFound => return Err(OpError::not_found("That version")),
        Restored::Moved(now) => {
            return Err(crate::error::AppError::VersionConflict {
                resource: "record",
                current_version: now,
            }
            .into());
        }
        Restored::Refused(why) => return Err(not_kept(why)),
    };
    crate::events::emit(
        tx,
        crate::events::NewEvent::thing(
            &crate::events::catalog::RECORD_VERSION_RESTORED,
            space_id,
            "record",
            id,
        )
        .version(now)
        .actor(actor)
        .workspace(workspace_id)
        .payload(json!({ "restoredVersion": version })),
    )
    .await?;
    call.audit_target("record", id, json!({ "restoredVersion": version }));
    Ok(HistoryRestoreOutput {
        kind: "record".to_owned(),
        restored: version,
        version: now,
    })
}
