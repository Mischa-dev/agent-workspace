//! Dashboards as operations (T1.32): list, read (with each tile's figures,
//! read as the caller), make, change and remove. Anyone in a project keeps
//! dashboards of their own; a shared one, or sharing one, needs a role that
//! can edit dashboards.

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    Change, Dashboard, Figures, MAX_DASHBOARDS, MAX_DESCRIPTION_CHARACTERS, MAX_NAME_CHARACTERS,
    MAX_TEXT_CHARACTERS, MAX_TILES, MAX_TITLE_CHARACTERS, NewDashboard, Tile, TileKind, Unseen,
    Written,
};
use crate::{
    access::{self, Need, Target as AccessTarget},
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        error::{ConflictReason, DenyReason},
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<DashboardList>()
        .add::<DashboardGet>()
        .add::<DashboardCreate>()
        .add::<DashboardUpdate>()
        .add::<DashboardDelete>();
}

fn invalid(field: &'static str, message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Invalid {
        field: Cow::Borrowed(field),
        message: message.into(),
    }
}

fn conflict(message: impl Into<Cow<'static, str>>) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State { code: "dashboard" },
        message: message.into(),
        current: None,
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

async fn need_share(call: &mut OpCall<'_>) -> Result<(), OpError> {
    if holds(call, Perm::DashboardsEdit).await? {
        return Ok(());
    }
    Err(OpError::Denied {
        reason: DenyReason::Role {
            permission: Perm::DashboardsEdit,
        },
        message: Cow::Borrowed(
            "Shared dashboards need a role that can edit dashboards. Make a dashboard of your \
             own instead.",
        ),
    })
}

fn one_line(field: &'static str, value: &str, most: usize, what: &str) -> Result<String, OpError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > most || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            format!("Give the {what} a name of 1 to {most} characters, on one line."),
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

/// A tile as written.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileInput {
    /// The tile's id, to keep a tile that's already there; left out for a
    /// new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<IdRef>,
    /// "view" (the default when view is given) or "text".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TileKind>,
    /// Shown above the tile; left out, the view's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A view tile's view, by id (view_list gives them). A chart view shows
    /// as its chart; any other as how many records it finds and the first
    /// few. A shared dashboard shows shared views only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<IdRef>,
    /// A text tile's note, up to 2,000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Columns of twelve it spans: 3 to 12 (6 when left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u8>,
    /// Rows it spans: 1 to 4 (2 when left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u8>,
}

/// Checks tiles as written: each view one the dashboard's readers can see
/// (a shared dashboard's, a shared view), each size in range. A tile it
/// already had (`before`) whose view has since gone stays as it was, so the
/// rest of the dashboard can still change; it shows as gone.
async fn tiles_of(
    call: &mut OpCall<'_>,
    tiles: &[TileInput],
    shared: bool,
    before: &[Tile],
) -> Result<Vec<Tile>, OpError> {
    if tiles.len() > MAX_TILES {
        return Err(invalid(
            "tiles",
            format!("A dashboard holds at most {MAX_TILES} tiles."),
        ));
    }
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let writer = call.ctx.caller.account_id;
    let tx = call.tx().await?;
    let mut written = Vec::with_capacity(tiles.len());
    for input in tiles {
        let kind = input.kind.unwrap_or(if input.view.is_some() {
            TileKind::View
        } else {
            TileKind::Text
        });
        let title = input.title.as_deref().unwrap_or("").trim().to_owned();
        if title.chars().count() > MAX_TITLE_CHARACTERS || title.chars().any(char::is_control) {
            return Err(invalid(
                "tiles",
                format!("A tile's title is one line of at most {MAX_TITLE_CHARACTERS} characters."),
            ));
        }
        let width = input.width.unwrap_or(6);
        let height = input.height.unwrap_or(2);
        if !(3..=12).contains(&width) || !(1..=4).contains(&height) {
            return Err(invalid(
                "tiles",
                "A tile is 3 to 12 columns wide and 1 to 4 rows high.",
            ));
        }
        let mut tile = Tile {
            id: input.id.map(IdRef::id).unwrap_or_else(Uuid::now_v7),
            kind,
            title,
            collection: None,
            view: None,
            text: String::new(),
            width,
            height,
        };
        match kind {
            TileKind::View => {
                let Some(view) = input.view.map(IdRef::id) else {
                    return Err(invalid("tiles", "A view tile names its view."));
                };
                let found = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
                    "SELECT collection_id, owner_actor_id FROM collection_views \
                     WHERE project_id = $1 AND id = $2 \
                       AND (owner_actor_id IS NULL OR owner_actor_id = $3)",
                )
                .bind(project_id)
                .bind(view)
                .bind(writer)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?;
                let Some((collection, owner)) = found else {
                    let kept = before
                        .iter()
                        .find(|old| old.id == tile.id && old.view == Some(view));
                    let Some(kept) = kept else {
                        return Err(OpError::not_found("That view"));
                    };
                    tile.collection = kept.collection;
                    tile.view = Some(view);
                    if written.iter().any(|other: &Tile| other.id == tile.id) {
                        return Err(invalid("tiles", "Each tile appears once."));
                    }
                    written.push(tile);
                    continue;
                };
                if shared && owner.is_some() {
                    return Err(invalid(
                        "tiles",
                        "A shared dashboard shows shared views only: share the view first, or \
                         keep the dashboard your own.",
                    ));
                }
                tile.collection = Some(collection);
                tile.view = Some(view);
            }
            TileKind::Text => {
                let text = input.text.as_deref().unwrap_or("").trim().to_owned();
                if text.is_empty() || text.chars().count() > MAX_TEXT_CHARACTERS {
                    return Err(invalid(
                        "tiles",
                        format!("A text tile holds 1 to {MAX_TEXT_CHARACTERS} characters."),
                    ));
                }
                tile.text = text;
            }
        }
        if written.iter().any(|other: &Tile| other.id == tile.id) {
            return Err(invalid("tiles", "Each tile appears once."));
        }
        written.push(tile);
    }
    Ok(written)
}

/// A dashboard as a reader sees it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// True for one everyone in the project sees; false for your own.
    pub shared: bool,
    pub tiles: Vec<TileEntry>,
    /// 1 is first, among the shared dashboards or among your own.
    pub position: i32,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TileEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: TileKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub collection: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub view: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub width: u8,
    pub height: u8,
    /// With figures: what the view finds, read as you.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub figures: Option<Figures>,
    /// With figures, when there are none for you: why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unseen: Option<String>,
}

fn entry(dashboard: &Dashboard) -> DashboardEntry {
    DashboardEntry {
        id: dashboard.id,
        name: dashboard.name.clone(),
        description: dashboard.description.clone(),
        shared: dashboard.owner_actor_id.is_none(),
        tiles: dashboard
            .tiles
            .iter()
            .map(|tile| TileEntry {
                id: tile.id,
                kind: tile.kind,
                title: tile.title.clone(),
                collection: tile.collection,
                view: tile.view,
                text: tile.text.clone(),
                width: tile.width,
                height: tile.height,
                figures: None,
                unseen: None,
            })
            .collect(),
        position: dashboard.position + 1,
        version: dashboard.version,
        updated_at: dashboard.updated_at.to_rfc3339(),
    }
}

async fn read(call: &mut OpCall<'_>, id: Uuid, lock: bool) -> Result<Dashboard, OpError> {
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let reader = call.ctx.caller.account_id;
    let tx = call.tx().await?;
    super::one(tx, project_id, id, reader, lock)
        .await?
        .ok_or_else(|| OpError::not_found("That dashboard"))
}

/// Tells open screens a shared dashboard changed, and audits the write.
async fn announce(
    call: &mut OpCall<'_>,
    dashboard: &Dashboard,
    version: i64,
    action: &str,
) -> Result<(), OpError> {
    if dashboard.owner_actor_id.is_none() {
        let tx = call.tx().await?;
        crate::outbox::enqueue_dashboard_updated(tx, dashboard.project_id, dashboard.id, version)
            .await?;
    }
    call.audit_target(
        "dashboard",
        dashboard.id,
        json!({ "action": action, "shared": dashboard.owner_actor_id.is_none() }),
    );
    Ok(())
}

fn written(outcome: Written) -> Result<Dashboard, OpError> {
    match outcome {
        Written::Done(dashboard) => Ok(dashboard),
        Written::NameTaken => Err(conflict(
            "Another dashboard here already has that name. Pick another.",
        )),
        Written::Full => Err(conflict(format!(
            "This project has {MAX_DASHBOARDS} dashboards already. Remove one first."
        ))),
    }
}

// ---------------------------------------------------------------------------
// dashboard_list

pub struct DashboardList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardListInput {
    /// The project: its id or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
}

impl Targeted for DashboardListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardListOutput {
    /// The shared dashboards in order, then your own; dashboard_get reads
    /// one with its figures.
    pub dashboards: Vec<DashboardEntry>,
    /// Whether you can make, change and remove shared dashboards here.
    pub can_share: bool,
}

impl Operation for DashboardList {
    const NAME: &'static str = "dashboard_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "dashboards";
    const PERMISSION: Option<Perm> = Some(Perm::DashboardsView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List dashboards";
    const DESCRIPTION: &'static str = "List a project's dashboards you can see: the shared ones, \
        then your own, each with its tiles. Read one's figures with dashboard_get.";

    type Input = DashboardListInput;
    type Output = DashboardListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: DashboardListInput,
    ) -> Result<DashboardListOutput, OpError> {
        let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let reader = call.ctx.caller.account_id;
        let can_share = holds(call, Perm::DashboardsEdit).await?;
        let tx = call.tx().await?;
        let dashboards = super::visible(tx, project_id, reader).await?;
        Ok(DashboardListOutput {
            dashboards: dashboards.iter().map(entry).collect(),
            can_share,
        })
    }
}

// ---------------------------------------------------------------------------
// dashboard_get

pub struct DashboardGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardGetInput {
    /// The dashboard: its id or a link to it.
    pub dashboard: IdRef,
    /// Read each view tile's figures (the default); false for the layout
    /// alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figures: Option<bool>,
}

impl Targeted for DashboardGetInput {
    fn target(&self) -> Target {
        Target::thing("dashboard", self.dashboard.id())
    }
}

impl Operation for DashboardGet {
    const NAME: &'static str = "dashboard_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "dashboards";
    const PERMISSION: Option<Perm> = Some(Perm::DashboardsView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read a dashboard";
    const DESCRIPTION: &'static str = "Read a dashboard and what each tile shows, as you see it: \
        a chart's groups with their counts (and sums or averages), or how many records a view \
        finds and the first few. A tile over records you can't see says so.";

    type Input = DashboardGetInput;
    type Output = DashboardEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DashboardGetInput,
    ) -> Result<DashboardEntry, OpError> {
        let dashboard = read(call, input.dashboard.id(), false).await?;
        let mut shown = entry(&dashboard);
        if input.figures == Some(false) {
            return Ok(shown);
        }
        let can_view_tasks = holds(call, Perm::TasksView).await?;
        let project_id = dashboard.project_id;
        let reader = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        for (tile, shown) in dashboard.tiles.iter().zip(shown.tiles.iter_mut()) {
            if tile.kind != TileKind::View {
                continue;
            }
            match super::figures(tx, project_id, tile, reader, can_view_tasks).await? {
                Ok(figures) => shown.figures = Some(figures),
                Err(Unseen::Gone) => {
                    shown.unseen =
                        Some("This tile's view is gone, or isn't one you can see.".into());
                }
                Err(Unseen::Tasks) => {
                    shown.unseen =
                        Some("You can't see this data: your role doesn't see tasks.".into());
                }
            }
        }
        Ok(shown)
    }
}

// ---------------------------------------------------------------------------
// dashboard_create

pub struct DashboardCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardCreateInput {
    /// The project: its id or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Shared with everyone in the project (the default; needs a role that
    /// can edit dashboards), or false for your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
    /// Its tiles, in order, at most 24.
    #[serde(default)]
    pub tiles: Vec<TileInput>,
}

impl Targeted for DashboardCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for DashboardCreate {
    const NAME: &'static str = "dashboard_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "dashboards";
    const PERMISSION: Option<Perm> = Some(Perm::DashboardsView);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["dashboard.updated"];
    const TITLE: &'static str = "Make a dashboard";
    const DESCRIPTION: &'static str = "Make a dashboard in a project: tiles that each show one of \
        a collection's views (a chart view as its chart) or a note. Make chart views first with \
        view_create (type chart). Shared dashboards need a role that can edit dashboards.";

    type Input = DashboardCreateInput;
    type Output = DashboardEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DashboardCreateInput,
    ) -> Result<DashboardEntry, OpError> {
        let name = one_line("name", &input.name, MAX_NAME_CHARACTERS, "dashboard")?;
        let description = description_of(input.description.as_deref().unwrap_or(""))?;
        let shared = input.shared.unwrap_or(true);
        if shared {
            need_share(call).await?;
        }
        let tiles = tiles_of(call, &input.tiles, shared, &[]).await?;
        let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace_id = call.ctx.workspace_id;
        let me = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let dashboard = written(
            super::create(
                tx,
                NewDashboard {
                    workspace_id,
                    project_id,
                    name: &name,
                    description: &description,
                    owner_actor_id: (!shared).then_some(me),
                    tiles: &tiles,
                    created_by: me,
                },
            )
            .await?,
        )?;
        announce(call, &dashboard, dashboard.version, "created").await?;
        Ok(entry(&dashboard))
    }
}

// ---------------------------------------------------------------------------
// dashboard_update

pub struct DashboardUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardUpdateInput {
    /// The dashboard: its id or a link to it.
    pub dashboard: IdRef,
    /// The version you last read; a change made since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Every tile, in order: replaces the ones there (pass a tile's id to
    /// keep it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiles: Option<Vec<TileInput>>,
    /// true shares one of your own with everyone in the project; false
    /// makes a shared one yours alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<bool>,
}

impl Targeted for DashboardUpdateInput {
    fn target(&self) -> Target {
        Target::thing("dashboard", self.dashboard.id())
    }
}

impl Operation for DashboardUpdate {
    const NAME: &'static str = "dashboard_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "dashboards";
    const PERMISSION: Option<Perm> = Some(Perm::DashboardsView);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["dashboard.updated"];
    const TITLE: &'static str = "Change a dashboard";
    const DESCRIPTION: &'static str = "Rename a dashboard, change its tiles, share one of your own, \
        or make a shared one yours alone. Changing a shared dashboard needs a role that can edit \
        dashboards.";

    type Input = DashboardUpdateInput;
    type Output = DashboardEntry;

    async fn run(
        call: &mut OpCall<'_>,
        input: DashboardUpdateInput,
    ) -> Result<DashboardEntry, OpError> {
        let name = input
            .name
            .as_deref()
            .map(|name| one_line("name", name, MAX_NAME_CHARACTERS, "dashboard"))
            .transpose()?;
        let description = input
            .description
            .as_deref()
            .map(description_of)
            .transpose()?;
        let current = read(call, input.dashboard.id(), true).await?;
        let was_shared = current.owner_actor_id.is_none();
        let shared = input.share.unwrap_or(was_shared);
        if shared || was_shared {
            need_share(call).await?;
        }
        let owner = match (was_shared, shared) {
            (false, true) => Some(None),
            (true, false) => Some(Some(call.ctx.caller.account_id)),
            _ => None,
        };
        if let Some(expected) = input.expected_version
            && expected != current.version
        {
            return Err(AppError::VersionConflict {
                resource: "dashboard",
                current_version: current.version,
            }
            .into());
        }
        // Sharing checks the tiles there too: a shared dashboard shows
        // shared views only.
        let tiles = match &input.tiles {
            Some(tiles) => Some(tiles_of(call, tiles, shared, &current.tiles).await?),
            None if shared && !was_shared => {
                let kept = current
                    .tiles
                    .iter()
                    .map(|tile| TileInput {
                        id: Some(IdRef(tile.id)),
                        kind: Some(tile.kind),
                        title: Some(tile.title.clone()),
                        view: tile.view.map(IdRef),
                        text: (!tile.text.is_empty()).then(|| tile.text.clone()),
                        width: Some(tile.width),
                        height: Some(tile.height),
                    })
                    .collect::<Vec<_>>();
                Some(tiles_of(call, &kept, true, &current.tiles).await?)
            }
            None => None,
        };
        let tx = call.tx().await?;
        let dashboard = written(
            super::change(
                tx,
                &current,
                Change {
                    name: name.as_deref(),
                    description: description.as_deref(),
                    tiles: tiles.as_deref(),
                    owner,
                },
            )
            .await?,
        )?;
        announce(call, &dashboard, dashboard.version, "updated").await?;
        Ok(entry(&dashboard))
    }
}

// ---------------------------------------------------------------------------
// dashboard_delete

pub struct DashboardDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardDeleteInput {
    /// The dashboard: its id or a link to it.
    pub dashboard: IdRef,
}

impl Targeted for DashboardDeleteInput {
    fn target(&self) -> Target {
        Target::thing("dashboard", self.dashboard.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDeleteOutput {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub removed: bool,
}

impl Operation for DashboardDelete {
    const NAME: &'static str = "dashboard_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "dashboards";
    const PERMISSION: Option<Perm> = Some(Perm::DashboardsView);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["dashboard.updated"];
    const TITLE: &'static str = "Remove a dashboard";
    const DESCRIPTION: &'static str = "Remove a dashboard. The views its tiles showed stay. \
        Removing a shared one needs a role that can edit dashboards.";

    type Input = DashboardDeleteInput;
    type Output = DashboardDeleteOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: DashboardDeleteInput,
    ) -> Result<DashboardDeleteOutput, OpError> {
        let current = read(call, input.dashboard.id(), true).await?;
        if current.owner_actor_id.is_none() {
            need_share(call).await?;
        }
        let tx = call.tx().await?;
        super::remove(tx, &current).await?;
        announce(call, &current, current.version + 1, "removed").await?;
        Ok(DashboardDeleteOutput {
            id: current.id,
            removed: true,
        })
    }
}
