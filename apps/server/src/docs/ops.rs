//! The Docs tree's operations: `doc_list`, `doc_folder_create`,
//! `doc_folder_update`, `doc_move`, `doc_access_get` and `doc_access_set`.
//! They work the same on every kind of document; making and opening one
//! stays with its own kind's tools.

use std::{borrow::Cow, collections::HashSet};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{DocEntry, DocKind, Found, MAX_FOLDER_DEPTH};
use crate::{
    access::{self, Need, Target as AccessTarget},
    error::AppError,
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<DocList>()
        .add::<DocFolderCreate>()
        .add::<DocFolderUpdate>()
        .add::<DocMove>()
        .add::<DocAccessGet>()
        .add::<DocAccessSet>();
}

const MAX_TITLE: usize = 200;
/// The most people one thing names.
const MAX_PEOPLE: usize = 200;

fn invalid(field: &'static str, message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Invalid {
        field: Cow::Borrowed(field),
        message: message.into(),
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

/// Whether the caller may change things of `kind` here: a document by its
/// kind's edit permission, a folder by any of them.
async fn edits(call: &mut OpCall<'_>, kind: DocKind) -> Result<bool, OpError> {
    match kind.edit() {
        Some(permission) => holds(call, permission).await,
        None => {
            for kind in DocKind::DOCUMENTS {
                if let Some(permission) = kind.edit()
                    && holds(call, permission).await?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

async fn need_edit(call: &mut OpCall<'_>, kind: DocKind) -> Result<(), OpError> {
    if edits(call, kind).await? {
        return Ok(());
    }
    Err(OpError::Denied {
        reason: DenyReason::Role {
            permission: kind.edit().unwrap_or(Perm::PagesEdit),
        },
        message: Cow::Borrowed(match kind {
            DocKind::Folder => {
                "Folders need a role that can edit some kind of document in this space."
            }
            _ => "That needs a role that can edit this kind of document here.",
        }),
    })
}

/// The kinds of document the caller sees in the call's space: the kind's
/// module on there, and its own permission to see it where it has one.
async fn kinds_seen(call: &mut OpCall<'_>) -> Result<Vec<DocKind>, OpError> {
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let account = call.account_id();
    let tx = call.tx().await?;
    let modules = crate::modules::state::for_project(tx, space, account).await?;
    let mut kinds = vec![DocKind::Folder];
    for kind in DocKind::DOCUMENTS {
        let on = kind
            .module()
            .is_some_and(|module| modules.is_some_and(|modules| modules.contains(module)));
        if !on {
            continue;
        }
        if match kind.see() {
            Some(permission) => holds(call, permission).await?,
            None => true,
        } {
            kinds.push(kind);
        }
    }
    Ok(kinds)
}

fn title_of(value: &str) -> Result<String, OpError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_TITLE || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "title",
            format!("Give the folder a name of 1 to {MAX_TITLE} characters, on one line."),
        ));
    }
    Ok(value.to_owned())
}

/// The document or folder the call names, as the caller sees it.
async fn found(call: &mut OpCall<'_>, id: Uuid) -> Result<Found, OpError> {
    let viewer = call.account_id();
    let space = call.ctx.space_id;
    let tx = call.tx().await?;
    super::find(tx, id, viewer)
        .await?
        .filter(|found| Some(found.space_id) == space)
        .ok_or(OpError::not_found("That document or folder"))
}

/// The folder `id` of the call's space, as the caller sees it.
async fn folder(call: &mut OpCall<'_>, field: &'static str, id: Uuid) -> Result<Found, OpError> {
    let viewer = call.account_id();
    let space = call.ctx.space_id;
    let tx = call.tx().await?;
    super::find(tx, id, viewer)
        .await?
        .filter(|found| found.kind == DocKind::Folder && Some(found.space_id) == space)
        .ok_or_else(|| invalid(field, "That isn't a folder of this space you can see."))
}

/// `doc.tree_changed`, heard by the space (ids only), in the change's
/// transaction.
async fn tree_changed(call: &mut OpCall<'_>, kind: DocKind, id: Uuid) -> Result<(), OpError> {
    let workspace_id = call.ctx.workspace_id;
    let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let by = call.account_id();
    let event = crate::events::NewEvent::thing(
        &crate::events::catalog::DOC_TREE_CHANGED,
        space_id,
        kind.key(),
        id,
    )
    .workspace(workspace_id)
    .actor(by);
    let tx = call.tx().await?;
    crate::events::emit(tx, event).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// doc_list

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocListInput {
    /// The space (a project): its id, link or key.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// List what is in this folder. Left out, the top of the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<IdRef>,
    /// Also list everything inside the folders below, the whole way down.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deep: bool,
}

impl Targeted for DocListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocListView {
    /// Folders first, then documents by title. parentId says what each one
    /// is inside.
    pub entries: Vec<DocEntry>,
    /// The kinds you may move, file and restrict here (`folder` among them
    /// when you may make folders).
    pub can_edit: Vec<DocKind>,
}

pub struct DocList;

impl Operation for DocList {
    const NAME: &'static str = "doc_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a space's documents";
    const DESCRIPTION: &'static str = "The Docs tree of a space: its folders and the pages, \
        sheets, whiteboards, collections and dashboards in them, as you may see them. Lists the \
        top of the tree, or one folder (folderId); deep: true lists everything below as well. \
        Each entry has its kind, its title and what it is inside (parentId); restricted means \
        only named people see it. Open one with its kind's own tool (page_get, sheet_get, \
        whiteboard_read, collection_get, dashboard_get), file it with doc_move, and make a new \
        one with its kind's create tool, then doc_move to put it in a folder.";

    type Input = DocListInput;
    type Output = DocListView;

    async fn run(call: &mut OpCall<'_>, input: DocListInput) -> Result<DocListView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let viewer = call.account_id();
        let kinds = kinds_seen(call).await?;
        let mut can_edit = Vec::new();
        for kind in &kinds {
            if edits(call, *kind).await? {
                can_edit.push(*kind);
            }
        }
        let tx = call.tx().await?;
        let all = super::entries(tx, space, viewer, &kinds).await?;
        let under = input.folder_id.map(IdRef::id);
        if let Some(under) = under
            && !all
                .iter()
                .any(|entry| entry.id == under && entry.kind == DocKind::Folder)
        {
            return Err(OpError::not_found("That folder"));
        }
        let entries = match (under, input.deep) {
            (None, true) => all,
            (under, false) => all
                .into_iter()
                .filter(|entry| entry.parent_id == under)
                .collect(),
            (Some(under), true) => {
                let mut inside = HashSet::from([under]);
                // Parents may list after their children, so sweep until
                // nothing more joins.
                loop {
                    let before = inside.len();
                    for entry in &all {
                        if entry
                            .parent_id
                            .is_some_and(|parent| inside.contains(&parent))
                        {
                            inside.insert(entry.id);
                        }
                    }
                    if inside.len() == before {
                        break;
                    }
                }
                all.into_iter()
                    .filter(|entry| entry.id != under && inside.contains(&entry.id))
                    .collect()
            }
        };
        Ok(DocListView { entries, can_edit })
    }
}

// ---------------------------------------------------------------------------
// doc_folder_create

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocFolderCreateInput {
    /// The space (a project): its id, link or key.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// The folder's name.
    pub title: String,
    /// Make it inside this folder. Left out, at the top of the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<IdRef>,
}

impl Targeted for DocFolderCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocPlaced {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: DocKind,
    pub title: String,
    /// The folder it is in now. Left out at the top of the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent_id: Option<Uuid>,
}

pub struct DocFolderCreate;

impl Operation for DocFolderCreate {
    const NAME: &'static str = "doc_folder_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Make a folder";
    const DESCRIPTION: &'static str = "Make a folder in a space's Docs tree, at the top or \
        inside another folder (parentFolderId). A folder holds any kind of document and other \
        folders. Put things in it with doc_move, and say who sees it with doc_access_set. Needs \
        a role that can edit some kind of document there.";
    const EMITS: &'static [&'static str] = &["doc.tree_changed"];

    type Input = DocFolderCreateInput;
    type Output = DocPlaced;

    async fn run(call: &mut OpCall<'_>, input: DocFolderCreateInput) -> Result<DocPlaced, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let by = call.account_id();
        need_edit(call, DocKind::Folder).await?;
        let title = title_of(&input.title)?;
        let parent = match input.parent_folder_id {
            Some(parent) => Some(folder(call, "parentFolderId", parent.id()).await?.id),
            None => None,
        };
        let tx = call.tx().await?;
        super::lock_tree(tx, space_id).await?;
        if let Some(parent) = parent
            && super::folders_above(tx, parent).await?.len() >= MAX_FOLDER_DEPTH
        {
            return Err(invalid(
                "parentFolderId",
                format!("Folders go {MAX_FOLDER_DEPTH} deep at most. Make it higher up."),
            ));
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO doc_folders (id, workspace_id, space_id, title, \
                 created_by_account_id, updated_by_account_id) \
             VALUES ($1, $2, $3, $4, $5, $5)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(&title)
        .bind(by)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let made = Found {
            id,
            kind: DocKind::Folder,
            workspace_id,
            space_id,
            title,
            parent_id: parent,
            restricted_here: false,
        };
        if parent.is_some() {
            super::place(tx, &made, parent).await?;
        }
        call.audit_target("folder", id, json!({ "parent": parent }));
        tree_changed(call, DocKind::Folder, id).await?;
        Ok(DocPlaced {
            id,
            kind: DocKind::Folder,
            title: made.title,
            parent_id: parent,
        })
    }
}

// ---------------------------------------------------------------------------
// doc_folder_update

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocFolderUpdateInput {
    /// The folder: its id.
    pub folder_id: IdRef,
    /// A new name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// true removes the folder. It has to be empty: move what it holds
    /// somewhere else first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

impl Targeted for DocFolderUpdateInput {
    fn target(&self) -> Target {
        Target::thing("doc", self.folder_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocFolderUpdated {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub title: String,
    /// The folder is gone.
    pub removed: bool,
}

pub struct DocFolderUpdate;

impl Operation for DocFolderUpdate {
    const NAME: &'static str = "doc_folder_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Rename or remove a folder";
    const DESCRIPTION: &'static str = "Rename a folder of the Docs tree (title), or remove it \
        (remove: true). Only an empty folder is removed, so nothing is lost with it: move what \
        it holds out with doc_move first. Needs a role that can edit some kind of document \
        there.";
    const EMITS: &'static [&'static str] = &["doc.tree_changed"];

    type Input = DocFolderUpdateInput;
    type Output = DocFolderUpdated;

    async fn run(
        call: &mut OpCall<'_>,
        input: DocFolderUpdateInput,
    ) -> Result<DocFolderUpdated, OpError> {
        let id = input.folder_id.id();
        let by = call.account_id();
        let viewer = by;
        let current = found(call, id).await?;
        if current.kind != DocKind::Folder {
            return Err(invalid(
                "folderId",
                "That is a document, not a folder. Rename it with its own kind's tool.",
            ));
        }
        need_edit(call, DocKind::Folder).await?;
        if input.title.is_none() && !input.remove {
            return Err(invalid("title", "Pass a new title, or remove: true."));
        }
        let title = match input.title.as_deref() {
            Some(title) => title_of(title)?,
            None => current.title.clone(),
        };
        let tx = call.tx().await?;
        super::lock_tree(tx, current.space_id).await?;
        if input.remove {
            let holds_something = super::entries(
                tx,
                current.space_id,
                viewer,
                &[DocKind::Folder]
                    .into_iter()
                    .chain(DocKind::DOCUMENTS)
                    .collect::<Vec<_>>(),
            )
            .await?
            .iter()
            .any(|entry| entry.parent_id == Some(id));
            if holds_something {
                return Err(invalid(
                    "remove",
                    "The folder still holds something. Move what is in it somewhere else first.",
                ));
            }
            // What the caller doesn't see in it (kept to other people, or
            // archived) goes up to where the folder was, never left filed
            // under a folder that is gone. What restricts each stays on it.
            sqlx::query(
                "UPDATE doc_places SET parent_id = $2, updated_at = now() WHERE parent_id = $1",
            )
            .bind(id)
            .bind(current.parent_id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
        sqlx::query(
            "UPDATE doc_folders SET title = $2, updated_by_account_id = $3, updated_at = now(), \
                 version = version + 1, archived_at = CASE WHEN $4 THEN now() END \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&title)
        .bind(by)
        .bind(input.remove)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target("folder", id, json!({ "removed": input.remove }));
        tree_changed(call, DocKind::Folder, id).await?;
        Ok(DocFolderUpdated {
            id,
            title,
            removed: input.remove,
        })
    }
}

// ---------------------------------------------------------------------------
// doc_move

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocMoveInput {
    /// What to move: the id of a page, sheet, whiteboard, collection,
    /// dashboard or folder, or a link to it.
    pub id: IdRef,
    /// The folder it goes into. Left out, the top of the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<IdRef>,
}

impl Targeted for DocMoveInput {
    fn target(&self) -> Target {
        Target::thing("doc", self.id.id())
    }
}

pub struct DocMove;

impl Operation for DocMove {
    const NAME: &'static str = "doc_move";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Move a document or folder";
    const DESCRIPTION: &'static str = "Put a page, sheet, whiteboard, collection, dashboard or \
        folder into a folder of its space's Docs tree (folderId), or at the top (folderId left \
        out). A folder takes everything in it along. Whatever is in a restricted folder is seen \
        only by the people that folder names, so moving something in or out changes who sees \
        it. A page inside another page moves with that page. Needs a role that can edit that \
        kind of document there.";
    const EMITS: &'static [&'static str] = &["doc.tree_changed"];

    type Input = DocMoveInput;
    type Output = DocPlaced;

    async fn run(call: &mut OpCall<'_>, input: DocMoveInput) -> Result<DocPlaced, OpError> {
        let id = input.id.id();
        let moving = found(call, id).await?;
        need_edit(call, moving.kind).await?;
        let to = match input.folder_id {
            Some(to) => Some(folder(call, "folderId", to.id()).await?.id),
            None => None,
        };
        let tx = call.tx().await?;
        super::lock_tree(tx, moving.space_id).await?;
        if moving.kind == DocKind::Page {
            let inside_page = sqlx::query_scalar::<_, bool>(
                "SELECT parent_page_id IS NOT NULL FROM project_pages WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?
            .unwrap_or(false);
            if inside_page {
                return Err(invalid(
                    "id",
                    "That page is inside another page and moves with it. Take it out first \
                     (page_update with parentPageId: null), then move it.",
                ));
            }
        }
        if let Some(to) = to {
            let above = super::folders_above(tx, to).await?;
            if moving.kind == DocKind::Folder {
                if above.contains(&id) {
                    return Err(invalid(
                        "folderId",
                        "A folder can't go inside itself or a folder it holds.",
                    ));
                }
                if above.len() + super::height(tx, id).await? > MAX_FOLDER_DEPTH {
                    return Err(invalid(
                        "folderId",
                        format!("Folders go {MAX_FOLDER_DEPTH} deep at most. Put it higher up."),
                    ));
                }
            }
        }
        super::place(tx, &moving, to).await?;
        call.audit_target(
            moving.kind.key(),
            id,
            json!({ "from": moving.parent_id, "to": to }),
        );
        tree_changed(call, moving.kind, id).await?;
        // What the people there may see changed: live connections check
        // what they watch again.
        let tx = call.tx().await?;
        access_changed(tx, moving.workspace_id, moving.space_id).await?;
        Ok(DocPlaced {
            id,
            kind: moving.kind,
            title: moving.title,
            parent_id: to,
        })
    }
}

/// Stages `account.access_changed` for everyone in the space, as a
/// membership change does for one account.
async fn access_changed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: Uuid,
    space: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "SELECT stage_access_changed($1, $2, membership.actor_id) \
         FROM project_memberships membership \
         WHERE membership.project_id = $2 AND membership.enabled",
    )
    .bind(workspace)
    .bind(space)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// doc_access_get, doc_access_set

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocAccessGetInput {
    /// The document or folder: its id or a link to it.
    pub id: IdRef,
}

impl Targeted for DocAccessGetInput {
    fn target(&self) -> Target {
        Target::thing("doc", self.id.id())
    }
}

/// Someone a document or folder names.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocPerson {
    #[schemars(with = "String")]
    pub account: Uuid,
    pub display_name: String,
}

/// The restricted thing above that decides who sees this one.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocAbove {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Its title, when you may see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocAccessView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: DocKind,
    /// Restricted on its own: only `people` see it. false means everyone
    /// who sees what it is inside sees it.
    pub restricted: bool,
    /// The people it names. They count while it is restricted.
    pub people: Vec<DocPerson>,
    /// Set when it isn't restricted itself but something above it is: the
    /// people that one names are the ones who see this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherited_from: Option<DocAbove>,
    /// You may change this with doc_access_set.
    pub can_change: bool,
    /// Your own account: doc_access_set always keeps it on the list.
    #[schemars(with = "String")]
    pub you: Uuid,
}

async fn access_view(call: &mut OpCall<'_>, thing: &Found) -> Result<DocAccessView, OpError> {
    let can_change = edits(call, thing.kind).await?;
    let viewer = call.account_id();
    let tx = call.tx().await?;
    let restricted = sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE((SELECT restricted FROM doc_places WHERE thing_id = $1), false)",
    )
    .bind(thing.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let people = sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT named.grantee_account_id, \
                (SELECT profile.display_name \
                 FROM account_profile(named.grantee_account_id, named.workspace_id) profile) \
         FROM thing_grants named \
         WHERE named.thing_id = $1 AND named.grantee_account_id IS NOT NULL \
           AND named.revoked_at IS NULL \
           AND (named.expires_at IS NULL OR named.expires_at > now()) \
         ORDER BY named.created_at, named.id",
    )
    .bind(thing.id)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?
    .into_iter()
    .map(|(account, name)| DocPerson {
        account,
        display_name: name.unwrap_or_default(),
    })
    .collect();
    let above = if restricted {
        None
    } else {
        sqlx::query_scalar::<_, Uuid>(
            "WITH RECURSIVE up(id, depth) AS ( \
                 SELECT parent_id, 1 FROM doc_places WHERE thing_id = $1 \
                 UNION ALL \
                 SELECT place.parent_id, up.depth + 1 \
                 FROM up JOIN doc_places place ON place.thing_id = up.id \
                 WHERE up.depth < 64) \
             SELECT up.id FROM up JOIN doc_places place ON place.thing_id = up.id \
             WHERE place.restricted ORDER BY up.depth LIMIT 1",
        )
        .bind(thing.id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
    };
    let inherited_from = match above {
        Some(id) => Some(DocAbove {
            id,
            title: super::find(tx, id, viewer).await?.map(|found| found.title),
        }),
        None => None,
    };
    Ok(DocAccessView {
        id: thing.id,
        kind: thing.kind,
        restricted,
        people,
        inherited_from,
        can_change,
        you: viewer,
    })
}

pub struct DocAccessGet;

impl Operation for DocAccessGet {
    const NAME: &'static str = "doc_access_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See who can see a document or folder";
    const DESCRIPTION: &'static str = "Who sees one document or folder of the Docs tree: \
        everyone in its space, or only the people it names (restricted), or the people named by \
        a restricted folder or page above it (inheritedFrom). Anyone who sees the document may \
        ask.";

    type Input = DocAccessGetInput;
    type Output = DocAccessView;

    async fn run(
        call: &mut OpCall<'_>,
        input: DocAccessGetInput,
    ) -> Result<DocAccessView, OpError> {
        let thing = found(call, input.id.id()).await?;
        access_view(call, &thing).await
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocAccessSetInput {
    /// The document or folder: its id or a link to it.
    pub id: IdRef,
    /// true: only the people named see it, and for a folder, everything in
    /// it. false: everyone who sees what it is inside sees it again.
    pub restricted: bool,
    /// With restricted: true, the whole list of accounts that see it (ids;
    /// people_search finds them). You are always kept on it. Anyone
    /// named before and left out here stops seeing it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<IdRef>,
}

impl Targeted for DocAccessSetInput {
    fn target(&self) -> Target {
        Target::thing("doc", self.id.id())
    }
}

pub struct DocAccessSet;

impl Operation for DocAccessSet {
    const NAME: &'static str = "doc_access_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Say who can see a document or folder";
    const DESCRIPTION: &'static str = "Restrict a document or folder of the Docs tree to the \
        people you name (restricted: true with people), or open it again to everyone who sees \
        what it is inside (restricted: false). A restricted folder covers everything in it. \
        Everyone else in the space stops finding it anywhere: lists, search, links and its \
        comments. What the named people may do with it still follows their role. Needs a role \
        that can edit that kind of document there.";
    const EMITS: &'static [&'static str] = &["doc.tree_changed"];

    type Input = DocAccessSetInput;
    type Output = DocAccessView;

    async fn run(
        call: &mut OpCall<'_>,
        input: DocAccessSetInput,
    ) -> Result<DocAccessView, OpError> {
        let thing = found(call, input.id.id()).await?;
        need_edit(call, thing.kind).await?;
        let by = call.account_id();
        if input.people.len() > MAX_PEOPLE {
            return Err(invalid(
                "people",
                format!("Name {MAX_PEOPLE} people at most."),
            ));
        }
        let tx = call.tx().await?;
        super::lock_tree(tx, thing.space_id).await?;
        // Looked at again now that the tree is this call's: someone left off
        // the list while this waited doesn't write it back.
        if super::find(tx, thing.id, by).await?.is_none() {
            return Err(OpError::not_found("That document or folder"));
        }
        if input.restricted && thing.kind == DocKind::Page {
            let guide =
                sqlx::query_scalar::<_, bool>("SELECT is_guide FROM project_pages WHERE id = $1")
                    .bind(thing.id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
            if guide == Some(true) {
                return Err(invalid(
                    "restricted",
                    "This page is the guide, which everyone here reads. Make another page the \
                     guide first.",
                ));
            }
        }
        if input.restricted {
            let mut people = vec![by];
            for person in &input.people {
                if !people.contains(&person.id()) {
                    people.push(person.id());
                }
            }
            let known = sqlx::query_scalar::<_, Uuid>(
                "SELECT membership.actor_id FROM organization_memberships membership \
                 JOIN actors account ON account.id = membership.actor_id \
                  AND account.status = 'active' \
                 WHERE membership.organization_id = $1 AND membership.actor_id = ANY($2) \
                   AND membership.enabled",
            )
            .bind(thing.workspace_id)
            .bind(&people)
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if known.len() != people.len() {
                return Err(invalid(
                    "people",
                    "One of those accounts isn't an active member of this workspace.",
                ));
            }
            // Whoever is left out stops being named; whoever is new is
            // named to see it (what they do with it follows their role).
            sqlx::query(
                "UPDATE thing_grants SET revoked_at = now() \
                 WHERE thing_id = $1 AND grantee_account_id IS NOT NULL \
                   AND revoked_at IS NULL \
                   AND (NOT (grantee_account_id = ANY($2)) OR expires_at <= now())",
            )
            .bind(thing.id)
            .bind(&people)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
            sqlx::query(
                "INSERT INTO thing_grants (id, workspace_id, space_id, thing_kind, thing_id, \
                     grantee_account_id, level, granted_by_account_id) \
                 SELECT gen_random_uuid(), $1, $2, $3, $4, person, 'see', $6 \
                 FROM unnest($5::uuid[]) AS person \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM thing_grants named \
                     WHERE named.thing_id = $4 AND named.grantee_account_id = person \
                       AND named.revoked_at IS NULL \
                       AND (named.expires_at IS NULL OR named.expires_at > now()))",
            )
            .bind(thing.workspace_id)
            .bind(thing.space_id)
            .bind(thing.kind.key())
            .bind(thing.id)
            .bind(&people)
            .bind(by)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        } else if thing.kind != DocKind::Page {
            // A page's named people are its shares, which stand on their
            // own; nothing else is shared one by one.
            sqlx::query(
                "UPDATE thing_grants SET revoked_at = now() \
                 WHERE thing_id = $1 AND revoked_at IS NULL",
            )
            .bind(thing.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
        super::restrict(tx, &thing, input.restricted).await?;
        access_changed(tx, thing.workspace_id, thing.space_id).await?;
        call.audit_target(
            thing.kind.key(),
            thing.id,
            json!({ "restricted": input.restricted }),
        );
        tree_changed(call, thing.kind, thing.id).await?;
        access_view(call, &thing).await
    }
}
