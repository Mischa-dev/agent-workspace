//! Views as operations (T1.6): the views of a collection (Tasks included)
//! that a reader sees, made, changed, removed, pinned and chosen as the one
//! a collection opens on. A view is shared with everyone who can see the
//! collection, or personal: only its owner sees it. Anyone who can see a
//! collection keeps views of their own; shared views, sharing a personal
//! one and a role's default need `collections.build`.

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::definition::{
    self as views, ChangeViewOutcome, CreateViewOutcome, DefaultFor, NewView, RemoveViewOutcome,
    Scope, View, ViewChange, ViewConfig,
};
use crate::{
    access::{self, Need, Target as AccessTarget},
    error::AppError,
    fields::FieldDefinition,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        error::{ConflictReason, DenyReason},
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ViewList>()
        .add::<ViewCreate>()
        .add::<ViewUpdate>()
        .add::<ViewDelete>()
        .add::<ViewPin>()
        .add::<ViewDefaultSet>();
}

/// The collection a view call acts on, locked for writes.
struct Held {
    project_id: Uuid,
    collection_id: Uuid,
    name: String,
    archived: bool,
}

/// Finds the collection in the call's space and checks the caller may see
/// its records (a collection whose records are tasks needs `tasks.view`),
/// and may build it when `build` is set.
async fn collection(
    call: &mut OpCall<'_>,
    collection_id: Uuid,
    lock: bool,
    build: bool,
) -> Result<Held, OpError> {
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    let row = sqlx::query_as::<_, (String, bool, bool)>(&format!(
        "SELECT name, archived_at IS NOT NULL, acts_like_task OR builtin IS NOT NULL \
         FROM collections WHERE project_id = $1 AND id = $2{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let Some((name, archived, tasks)) = row else {
        return Err(OpError::not_found("That collection"));
    };
    if tasks {
        need(tx, &caller, Perm::TasksView, project_id, "That collection").await?;
    }
    if build {
        need(
            tx,
            &caller,
            Perm::CollectionsBuild,
            project_id,
            "That collection",
        )
        .await?;
    }
    Ok(Held {
        project_id,
        collection_id,
        name,
        archived,
    })
}

async fn need(
    tx: &mut Transaction<'static, Postgres>,
    caller: &access::Caller,
    permission: Perm,
    space: Uuid,
    what: &'static str,
) -> Result<(), OpError> {
    match access::check(tx, caller, Need::permission(permission), AccessTarget::Space(space))
        .await?
    {
        Ok(_) => Ok(()),
        Err(access::Denied::NotFound) => Err(OpError::not_found(what)),
        Err(_) => Err(OpError::Denied {
            reason: DenyReason::Role { permission },
            message: Cow::Borrowed(match permission {
                Perm::CollectionsBuild => {
                    "Shared views and the defaults for a role need a role that can build \
                     collections. Make a view of your own instead."
                }
                _ => "Your role can't see these records.",
            }),
        }),
    }
}

/// The view a call names, with its collection: shared, or the caller's own.
async fn view_of(call: &mut OpCall<'_>, view_id: Uuid) -> Result<View, OpError> {
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let reader = call.ctx.caller.account_id;
    let tx = call.tx().await?;
    let collection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT collection_id FROM collection_views WHERE project_id = $1 AND id = $2 \
           AND (owner_actor_id IS NULL OR owner_actor_id = $3)",
    )
    .bind(project_id)
    .bind(view_id)
    .bind(reader)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That view"))?;
    views::one(tx, project_id, collection_id, view_id)
        .await?
        .ok_or_else(|| OpError::not_found("That view"))
}

fn conflict(message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State { code: "view" },
        message: message.into(),
        current: None,
    }
}

fn invalid(field: &'static str, message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Invalid {
        field: Cow::Borrowed(field),
        message: message.into(),
    }
}

fn archived(held: &Held) -> OpError {
    conflict(format!(
        "{} is archived. Restore it to change its views.",
        held.name
    ))
}

fn name_of(value: &str) -> Result<String, OpError> {
    crate::collections::normalize_name(value).ok_or_else(|| {
        invalid(
            "name",
            format!(
                "Give the view a name of 1 to {} characters, on one line.",
                crate::collections::MAX_NAME_CHARACTERS
            ),
        )
    })
}

fn config_of(value: Option<Value>) -> Result<Option<ViewConfig>, OpError> {
    value
        .map(|value| {
            serde_json::from_value::<ViewConfig>(value).map_err(|error| {
                invalid(
                    "config",
                    format!(
                        "A view's config has filters, sort, groupBy, groupBucket, columns, \
                         titleWidth and dateField: {error}"
                    ),
                )
            })
        })
        .transpose()
}

async fn checked(
    tx: &mut Transaction<'static, Postgres>,
    project_id: Uuid,
    config: &ViewConfig,
    definitions: &[FieldDefinition],
) -> Result<(), OpError> {
    views::check(config, definitions).map_err(|message| invalid("config", message))?;
    views::check_references(tx, project_id, config, definitions)
        .await?
        .map_err(|message| invalid("config", message))
}

/// Tells open screens the collection's views changed; bumps its version
/// when a shared view did.
async fn announce(
    call: &mut OpCall<'_>,
    held: &Held,
    shared: bool,
    action: &str,
    view: &View,
) -> Result<(), OpError> {
    let tx = call.tx().await?;
    if shared {
        crate::collections::touch(tx, held.project_id, held.collection_id).await?;
    }
    crate::outbox::enqueue_collection_updated(tx, held.project_id, held.collection_id, &[])
        .await?;
    call.audit_target(
        "collection",
        held.collection_id,
        json!({ "view": view.id, "action": action, "shared": view.owner_actor_id.is_none() }),
    );
    Ok(())
}

/// One view as a reader sees it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub name: String,
    /// table, board, list, gallery, calendar, timeline or chart.
    #[serde(rename = "type")]
    pub view_type: String,
    /// Its filters, sort, grouping and columns, by field id ("me" in a
    /// person filter is whoever reads it).
    pub config: Value,
    /// True for a view everyone who can see the collection sees; false for
    /// one of your own.
    pub shared: bool,
    /// One of the views every Tasks collection starts with: my_tasks,
    /// ready_for_agents, blocked or recently_changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    /// In your sidebar.
    pub pinned: bool,
    /// Its place among your pinned views (0 is first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_position: Option<i32>,
    /// 1 is first, among the shared views or among your own.
    pub position: i32,
    pub version: i64,
}

fn entry(
    view: &View,
    definitions: &[FieldDefinition],
    pins: &std::collections::HashMap<Uuid, (bool, i32)>,
) -> Result<ViewEntry, OpError> {
    let (pinned, pin_position) = match pins.get(&view.id) {
        Some((pinned, position)) => (*pinned, pinned.then_some(*position)),
        None => (views::pinned_unless_unpinned(view), None),
    };
    Ok(ViewEntry {
        id: view.id,
        collection_id: view.collection_id,
        name: view.name.clone(),
        view_type: view.view_type.as_str().to_owned(),
        config: serde_json::to_value(view.config.active(definitions))
            .map_err(|_| AppError::Internal("encode view config"))?,
        shared: view.owner_actor_id.is_none(),
        builtin: view.builtin.clone(),
        pinned,
        pin_position,
        position: view.position + 1,
        version: view.version,
    })
}

async fn entry_for(call: &mut OpCall<'_>, view: &View) -> Result<ViewEntry, OpError> {
    let reader = call.ctx.caller.account_id;
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let tx = call.tx().await?;
    let definitions =
        crate::collections::field_definitions(tx, project_id, view.collection_id, true).await?;
    let pins = views::pins(tx, project_id, view.collection_id, reader).await?;
    entry(view, &definitions, &pins)
}

// ---------------------------------------------------------------------------
// view_list

pub struct ViewList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewListInput {
    /// The collection (the project's Tasks collection for task views): its
    /// id, or a link to it.
    pub collection: IdRef,
}

impl Targeted for ViewListInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewListOutput {
    /// The shared views in order, then your own.
    pub views: Vec<ViewEntry>,
    /// The view the collection opens on for you: your own default, else
    /// your role's, else the first shared view.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub default_view_id: Option<Uuid>,
}

impl Operation for ViewList {
    const NAME: &'static str = "view_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List views";
    const DESCRIPTION: &'static str = "List the views of a collection you can use: the shared \
        ones, then your own, with which are in your sidebar and the one it opens on for you. \
        Pass a view's id to collection_query or task_list to read the records it shows.";

    type Input = ViewListInput;
    type Output = ViewListOutput;

    async fn run(call: &mut OpCall<'_>, input: ViewListInput) -> Result<ViewListOutput, OpError> {
        let held = collection(call, input.collection.id(), false, false).await?;
        let reader = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let found = views::visible(tx, held.project_id, held.collection_id, reader).await?;
        let definitions =
            crate::collections::field_definitions(tx, held.project_id, held.collection_id, true)
                .await?;
        let pins = views::pins(tx, held.project_id, held.collection_id, reader).await?;
        let default_view_id =
            views::default_view(tx, held.project_id, held.collection_id, reader).await?;
        Ok(ViewListOutput {
            views: found
                .iter()
                .map(|view| entry(view, &definitions, &pins))
                .collect::<Result<_, _>>()?,
            default_view_id,
        })
    }
}

// ---------------------------------------------------------------------------
// view_create

pub struct ViewCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewCreateInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    pub name: String,
    /// table, board, list, gallery, calendar, timeline or chart (what the
    /// collection's records can show).
    #[serde(rename = "type")]
    pub view_type: String,
    /// Filters, sort, groupBy, groupBucket, columns, titleWidth and
    /// dateField, fields by id. A board without groupBy, or a calendar
    /// without dateField, takes the collection's first field that fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    /// True to share it with everyone who can see the collection (needs a
    /// role that can build collections); a view of your own by default.
    #[serde(default)]
    pub shared: bool,
    /// 1 is first; the end when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

impl Targeted for ViewCreateInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

impl Operation for ViewCreate {
    const NAME: &'static str = "view_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Make a view";
    const DESCRIPTION: &'static str = "Save a way to look at a collection's records (Tasks \
        included): a name, a type and its filters, sort, grouping and columns. It is yours \
        alone unless shared is true.";

    type Input = ViewCreateInput;
    type Output = ViewEntry;

    async fn run(call: &mut OpCall<'_>, input: ViewCreateInput) -> Result<ViewEntry, OpError> {
        let name = name_of(&input.name)?;
        let view_type = views::parse_view_type(&input.view_type)
            .ok_or_else(|| invalid("type", views::VIEW_TYPE_MESSAGE))?;
        let config = config_of(input.config)?.unwrap_or_default();
        let held = collection(call, input.collection.id(), true, input.shared).await?;
        if held.archived {
            return Err(archived(&held));
        }
        let actor = call.ctx.caller.account_id;
        let scope = if input.shared {
            Scope::Shared
        } else {
            Scope::Personal(actor)
        };
        let tx = call.tx().await?;
        let definitions =
            crate::collections::field_definitions(tx, held.project_id, held.collection_id, true)
                .await?;
        let config = config
            .for_type(view_type, &definitions)
            .map_err(|message| invalid("config", message))?;
        checked(tx, held.project_id, &config, &definitions).await?;
        let view = match views::create(
            tx,
            held.project_id,
            held.collection_id,
            actor,
            NewView {
                name,
                view_type,
                config,
                position: input.position,
                scope,
            },
        )
        .await?
        {
            CreateViewOutcome::Created(view) => view,
            CreateViewOutcome::NameTaken(view) => {
                return Err(conflict(format!(
                    "A view is already called \"{}\". Pick another name.",
                    view.name
                )));
            }
            CreateViewOutcome::Full => {
                return Err(conflict(format!(
                    "There are {} such views already. Remove one first.",
                    match scope {
                        Scope::Shared => views::MAX_VIEWS,
                        Scope::Personal(_) => views::MAX_PERSONAL_VIEWS,
                    }
                )));
            }
        };
        announce(call, &held, input.shared, "created", &view).await?;
        entry_for(call, &view).await
    }
}

// ---------------------------------------------------------------------------
// view_update

pub struct ViewUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewUpdateInput {
    /// The view: its id.
    pub view: IdRef,
    /// The version you last read; a change since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its whole config: filters, sort, groupBy, groupBucket, columns,
    /// titleWidth and dateField.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    /// 1 is first, among the shared views or among your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// True shares a view of your own with everyone who can see the
    /// collection (needs a role that can build collections).
    #[serde(default)]
    pub share: bool,
}

impl Targeted for ViewUpdateInput {
    fn target(&self) -> Target {
        Target::thing("view", self.view.id())
    }
}

impl Operation for ViewUpdate {
    const NAME: &'static str = "view_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change a view";
    const DESCRIPTION: &'static str = "Rename a view, change what it shows, move it, or share \
        one of your own. Changing a shared view needs a role that can build collections.";

    type Input = ViewUpdateInput;
    type Output = ViewEntry;

    async fn run(call: &mut OpCall<'_>, input: ViewUpdateInput) -> Result<ViewEntry, OpError> {
        let name = input.name.as_deref().map(name_of).transpose()?;
        let config = config_of(input.config)?;
        let current = view_of(call, input.view.id()).await?;
        let scope = current.scope();
        let held = collection(
            call,
            current.collection_id,
            true,
            scope == Scope::Shared || input.share,
        )
        .await?;
        if held.archived {
            return Err(archived(&held));
        }
        let tx = call.tx().await?;
        if let Some(config) = &config {
            let definitions = crate::collections::field_definitions(
                tx,
                held.project_id,
                held.collection_id,
                true,
            )
            .await?;
            checked(tx, held.project_id, config, &definitions).await?;
        }
        let view = match views::change(
            tx,
            held.project_id,
            held.collection_id,
            scope,
            current.id,
            ViewChange {
                name,
                config,
                position: input.position,
                expected_version: input.expected_version,
                share: input.share,
            },
        )
        .await?
        {
            ChangeViewOutcome::Changed { view, .. } => {
                announce(call, &held, view.owner_actor_id.is_none(), "updated", &view).await?;
                view
            }
            ChangeViewOutcome::Unchanged(view) => view,
            ChangeViewOutcome::NotFound => return Err(OpError::not_found("That view")),
            ChangeViewOutcome::VersionConflict(view) => {
                return Err(AppError::VersionConflict {
                    resource: "view",
                    current_version: view.version,
                }
                .into());
            }
            ChangeViewOutcome::NameTaken(other) => {
                return Err(conflict(format!(
                    "Another view is already called \"{other}\"."
                )));
            }
            ChangeViewOutcome::Full => {
                return Err(conflict(format!(
                    "{} has {} shared views already. Remove one first.",
                    held.name,
                    views::MAX_VIEWS
                )));
            }
        };
        entry_for(call, &view).await
    }
}

// ---------------------------------------------------------------------------
// view_delete

pub struct ViewDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewDeleteInput {
    /// The view: its id.
    pub view: IdRef,
}

impl Targeted for ViewDeleteInput {
    fn target(&self) -> Target {
        Target::thing("view", self.view.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewDeleted {
    #[schemars(with = "String")]
    pub removed: Uuid,
}

impl Operation for ViewDelete {
    const NAME: &'static str = "view_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Remove a view";
    const DESCRIPTION: &'static str = "Remove a view. The records it showed stay. Removing a \
        shared view needs a role that can build collections, and a collection keeps at least \
        one shared view.";

    type Input = ViewDeleteInput;
    type Output = ViewDeleted;

    async fn run(call: &mut OpCall<'_>, input: ViewDeleteInput) -> Result<ViewDeleted, OpError> {
        let current = view_of(call, input.view.id()).await?;
        let scope = current.scope();
        let held = collection(call, current.collection_id, true, scope == Scope::Shared).await?;
        if held.archived {
            return Err(archived(&held));
        }
        let tx = call.tx().await?;
        let view =
            match views::remove(tx, held.project_id, held.collection_id, scope, current.id)
                .await?
            {
                RemoveViewOutcome::Removed(view) => view,
                RemoveViewOutcome::NotFound => return Err(OpError::not_found("That view")),
                RemoveViewOutcome::LastView => {
                    return Err(conflict(
                        "A collection keeps at least one shared view. Add another before \
                         removing this one.",
                    ));
                }
            };
        announce(call, &held, scope == Scope::Shared, "removed", &view).await?;
        Ok(ViewDeleted { removed: view.id })
    }
}

// ---------------------------------------------------------------------------
// view_pin

pub struct ViewPin;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewPinInput {
    /// The view: its id.
    pub view: IdRef,
    /// True keeps it in your sidebar; false takes it out.
    pub pinned: bool,
    /// Its place among your pinned views, 0 first; the end when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

impl Targeted for ViewPinInput {
    fn target(&self) -> Target {
        Target::thing("view", self.view.id())
    }
}

impl Operation for ViewPin {
    const NAME: &'static str = "view_pin";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Pin a view";
    const DESCRIPTION: &'static str = "Keep a view in your sidebar, or take it out. Only your \
        own sidebar changes.";

    type Input = ViewPinInput;
    type Output = ViewEntry;

    async fn run(call: &mut OpCall<'_>, input: ViewPinInput) -> Result<ViewEntry, OpError> {
        let view = view_of(call, input.view.id()).await?;
        let held = collection(call, view.collection_id, false, false).await?;
        let actor = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        views::set_pin(
            tx,
            held.project_id,
            view.id,
            actor,
            input.pinned,
            input.position,
        )
        .await?;
        call.audit_target(
            "collection",
            held.collection_id,
            json!({ "view": view.id, "pinned": input.pinned }),
        );
        entry_for(call, &view).await
    }
}

// ---------------------------------------------------------------------------
// view_default_set

pub struct ViewDefaultSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewDefaultSetInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// The view it opens on; left out, the default is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<IdRef>,
    /// A role (by id): the default for everyone holding it in this space,
    /// which needs a role that can build collections. Left out, your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<IdRef>,
}

impl Targeted for ViewDefaultSetInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewDefaultSetOutput {
    /// The view the collection now opens on for you.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub default_view_id: Option<Uuid>,
}

impl Operation for ViewDefaultSet {
    const NAME: &'static str = "view_default_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Choose a default view";
    const DESCRIPTION: &'static str = "Choose the view a collection opens on: for you, or for \
        everyone holding a role. Your own choice wins over your role's; with neither it opens \
        on the first shared view.";

    type Input = ViewDefaultSetInput;
    type Output = ViewDefaultSetOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ViewDefaultSetInput,
    ) -> Result<ViewDefaultSetOutput, OpError> {
        let held = collection(call, input.collection.id(), false, input.role.is_some()).await?;
        let actor = call.ctx.caller.account_id;
        let workspace_id = call.ctx.workspace_id;
        let view = match input.view {
            Some(view) => Some(view_of(call, view.id()).await?),
            None => None,
        };
        if let Some(view) = &view {
            if view.collection_id != held.collection_id {
                return Err(invalid("view", "That view is of another collection."));
            }
            if input.role.is_some() && view.owner_actor_id.is_some() {
                return Err(invalid(
                    "view",
                    "A role's default is a shared view. Share this one first.",
                ));
            }
        }
        let tx = call.tx().await?;
        let holder = match input.role {
            Some(role) => {
                let role_id = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM workspace_roles WHERE id = $1 AND organization_id = $2",
                )
                .bind(role.id())
                .bind(workspace_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?
                .ok_or_else(|| OpError::not_found("That role"))?;
                DefaultFor::Role(role_id)
            }
            None => DefaultFor::Account(actor),
        };
        views::set_default(
            tx,
            held.project_id,
            held.collection_id,
            holder,
            view.as_ref().map(|view| view.id),
            actor,
        )
        .await?;
        let default_view_id =
            views::default_view(tx, held.project_id, held.collection_id, actor).await?;
        call.audit_target(
            "collection",
            held.collection_id,
            json!({
                "defaultView": view.as_ref().map(|view| view.id),
                "role": match holder { DefaultFor::Role(role) => Some(role), DefaultFor::Account(_) => None },
            }),
        );
        Ok(ViewDefaultSetOutput { default_view_id })
    }
}
