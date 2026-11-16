//! The directory's operations (M1.11) and shapes (M1.17).
//!
//! `directory_list` says what can be installed here; a workspace adds its
//! own URL lists with `directory_source_create` and drops them with
//! `directory_source_remove`. `workspace_shape_set` installs one of the
//! five shapes (3.8b) as a package: it turns modules on and off and adds
//! starter parts, and never deletes anything. With `preview: true` it runs
//! and rolls back, so a person sees what changing shape does first.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    apply::Installed,
    builtin,
    directory::{self, EntryView},
    install::Place,
    ops::install_planned,
    source::Origin,
};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
    refs::parse_id,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<DirectoryList>();
    registry.add::<DirectorySourceCreate>();
    registry.add::<DirectorySourceRemove>();
    registry.add::<WorkspaceShapeSet>();
}

// ------------------------------------------------------------ directory_list

/// What can be installed.
pub struct DirectoryList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryListInput {
    /// Words to find in names, ids and descriptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Only shapes (true) or only the rest (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shapes: Option<bool>,
}

impl Targeted for DirectoryListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// A source of the directory this workspace added.
#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub url: String,
    pub name: String,
    /// Why it couldn't be read the last time, if it couldn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When it was last read; empty until the first read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListOutput {
    pub entries: Vec<EntryView>,
    /// The workspace's own URL lists.
    pub sources: Vec<SourceView>,
    /// Whether this server lets a workspace add its own URL lists.
    pub own_sources_allowed: bool,
    /// The shape this workspace last set, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

const SOURCES_SELECT: &str = "SELECT id, location AS url, name, error, \
     fetched_at::text AS fetched_at FROM directory_sources \
     WHERE workspace_id = $1 ORDER BY created_at, id";

impl Operation for DirectoryList {
    const NAME: &'static str = "directory_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List what can be installed";
    const DESCRIPTION: &'static str = "List the packages this workspace can install: the built-in \
        ones (workspace templates and the shapes a workspace starts from), the server's own \
        folder and lists, the workspace's lists and its saved templates. Each entry names what \
        to pass as package_preview's and package_install's entry. An entry that couldn't be \
        read is listed with its error. Also says which shape the workspace has.";

    type Input = DirectoryListInput;
    type Output = DirectoryListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: DirectoryListInput,
    ) -> Result<DirectoryListOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let allowed = call.state.config.directory_workspace_sources;
        let tx = call.tx().await?;
        let mut entries = directory::list(tx, workspace, input.query.as_deref()).await?;
        if let Some(shapes) = input.shapes {
            entries.retain(|entry| entry.shape == shapes);
        }
        let sources = sqlx::query_as::<_, SourceView>(SOURCES_SELECT)
            .bind(workspace)
            .fetch_all(&mut **tx)
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        let shape = current_shape(tx, workspace).await?;
        Ok(DirectoryListOutput {
            entries,
            sources,
            own_sources_allowed: allowed,
            shape,
        })
    }
}

/// The shape a workspace last installed.
async fn current_shape(
    connection: &mut sqlx::PgConnection,
    workspace: Uuid,
) -> Result<Option<String>, OpError> {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT package.package_key FROM package_installs install
        JOIN packages package ON package.id = install.package_id
        WHERE install.workspace_id = $1 AND install.state = 'installed'
          AND package.source = 'builtin' AND package.package_key LIKE 'shapes.%'
        ORDER BY install.created_at DESC, install.id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| OpError::logged("directory_list", &error.into()))
}

// --------------------------------------------------- directory_source_create

/// Add a URL list to the workspace's directory.
pub struct DirectorySourceCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySourceCreateInput {
    /// The list's address: an https URL serving a JSON index
    /// ({"format": "agent-workspace-directory/1", "entries": [...]}).
    pub url: String,
    /// A name for it, shown beside its entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Targeted for DirectorySourceCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for DirectorySourceCreate {
    const NAME: &'static str = "directory_source_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["directory.source_added"];
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Add a package list";
    const DESCRIPTION: &'static str = "Add a URL list to this workspace's directory: an https \
        address serving a JSON index of packages. It is read within a few minutes and then \
        hourly; its packages show in directory_list, and one that can't be read is listed \
        with its error. Only where the server allows workspaces their own lists.";

    type Input = DirectorySourceCreateInput;
    type Output = SourceView;

    async fn run(
        call: &mut OpCall<'_>,
        input: DirectorySourceCreateInput,
    ) -> Result<SourceView, OpError> {
        let config = &call.state.config;
        if !config.directory_workspace_sources {
            return Err(OpError::invalid(
                "url",
                "This server doesn't let a workspace add its own package lists.",
            ));
        }
        let url = input.url.trim().to_owned();
        let parsed = url::Url::parse(&url).ok();
        let usable = parsed.as_ref().is_some_and(|parsed| {
            (parsed.scheme() == "https"
                || (parsed.scheme() == "http" && config.allow_private_addresses))
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.fragment().is_none()
        }) && url.chars().count() <= 2000;
        if !usable {
            return Err(OpError::invalid(
                "url",
                "Give an https address with no user name, password or #fragment.",
            ));
        }
        if !config.allow_private_addresses
            && parsed
                .as_ref()
                .and_then(url::Url::host_str)
                .is_some_and(crate::outbound::plainly_private)
        {
            return Err(OpError::invalid(
                "url",
                "That address is on a private network this server doesn't reach.",
            ));
        }
        let name = input
            .name
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let tx = call.tx().await?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM directory_sources WHERE workspace_id = $1",
        )
        .bind(workspace)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        if count >= 20 {
            return Err(OpError::invalid(
                "url",
                "A workspace has at most 20 package lists. Remove one first.",
            ));
        }
        let id = Uuid::now_v7();
        let inserted = sqlx::query(
            r#"
            INSERT INTO directory_sources (
                id, workspace_id, kind, location, name, created_by_actor_id
            ) VALUES ($1, $2, 'url_list', $3, $4, $5)
            ON CONFLICT (workspace_id, kind, location) WHERE workspace_id IS NOT NULL
                DO NOTHING
            "#,
        )
        .bind(id)
        .bind(workspace)
        .bind(&url)
        .bind(&name)
        .bind(actor)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        if inserted.rows_affected() == 0 {
            return Err(OpError::invalid(
                "url",
                "The workspace's directory already reads that list.",
            ));
        }
        // Read soon: the refresh job reads sources never read yet.
        crate::jobs::enqueue(
            &mut **tx,
            crate::jobs::NewJob::new(directory::REFRESH_KIND)
                .workspace(workspace)
                .unique(format!("directory_refresh:{id}")),
        )
        .await?;
        call.audit_target("directory_source", id, json!({ "kind": "url_list" }));
        Ok(SourceView {
            id,
            url,
            name,
            error: None,
            fetched_at: None,
        })
    }
}

// --------------------------------------------------- directory_source_remove

/// Drop a URL list from the workspace's directory.
pub struct DirectorySourceRemove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySourceRemoveInput {
    /// The list's id, from directory_list's sources.
    pub source_id: String,
}

impl Targeted for DirectorySourceRemoveInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceRemoved {
    #[schemars(with = "String")]
    pub id: Uuid,
}

impl Operation for DirectorySourceRemove {
    const NAME: &'static str = "directory_source_remove";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["directory.source_removed"];
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Remove a package list";
    const DESCRIPTION: &'static str = "Stop reading one of this workspace's own package lists: \
        its packages leave directory_list. Packages already installed from it stay as they \
        are.";

    type Input = DirectorySourceRemoveInput;
    type Output = SourceRemoved;

    async fn run(
        call: &mut OpCall<'_>,
        input: DirectorySourceRemoveInput,
    ) -> Result<SourceRemoved, OpError> {
        let id = parse_id(&input.source_id).ok_or(OpError::not_found("That list"))?;
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        // Only the workspace's own lists; the server's stay.
        let removed = sqlx::query(
            "DELETE FROM directory_sources WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        if removed.rows_affected() == 0 {
            return Err(OpError::not_found("That list"));
        }
        call.audit_target("directory_source", id, json!({ "removed": true }));
        Ok(SourceRemoved { id })
    }
}

// ------------------------------------------------------- workspace_shape_set

/// Change what the workspace is for.
pub struct WorkspaceShapeSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceShapeSetInput {
    /// friends, team, just_me, documents or blank (directory_list with
    /// shapes: true lists them).
    pub shape: String,
    /// package_preview's planHash for the shape's entry: refused if the
    /// plan changed since. Left out, the plan as it is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
}

impl Targeted for WorkspaceShapeSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShapeSet {
    /// The shape's id, such as "shapes.friends".
    pub shape: String,
    pub name: String,
    pub installed: Installed,
}

impl Operation for WorkspaceShapeSet {
    const NAME: &'static str = "workspace_shape_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["package.installed"];
    const PREVIEW: bool = true;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Change what the workspace is for";
    const DESCRIPTION: &'static str = "Give the workspace a shape: Friends (just chat), Team \
        (everything), Just me and my agents, Documents, or Blank (chat only, nothing made). It \
        turns modules on and off, sets the starting layout and its words, and adds starter \
        channels and pages; it never deletes anything, and a module turned off is hidden, not \
        removed. Pass preview: true to see what it would do first.";

    type Input = WorkspaceShapeSetInput;
    type Output = ShapeSet;

    async fn run(call: &mut OpCall<'_>, input: WorkspaceShapeSetInput) -> Result<ShapeSet, OpError> {
        let Some(shape) = builtin::shape(&input.shape) else {
            let known = builtin::shapes()
                .map(|shape| shape.package.id.trim_start_matches("shapes."))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(OpError::Invalid {
                field: "shape".into(),
                message: format!("There's no shape {}. Use one of {known}.", input.shape.trim())
                    .into(),
            });
        };
        let place = Place::of(&shape.package, None, None)?;
        let origin = Origin {
            package: shape.package.clone(),
            document: shape.document.clone(),
            source: "builtin",
            source_ref: Some(format!("builtin:{}", shape.folder)),
            template: None,
        };
        let installed = install_planned(call, origin, place, input.plan_hash.as_deref()).await?;
        Ok(ShapeSet {
            shape: shape.package.id.clone(),
            name: shape.package.name.clone(),
            installed,
        })
    }
}
