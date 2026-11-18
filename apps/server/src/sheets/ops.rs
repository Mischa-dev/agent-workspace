//! Sheet operations (C1.16): `sheet_search` finds cells by what they show
//! or hold, and saved filter views (`sheet_view_*`) keep a way to look at
//! a tab: which rows show, in what order, which columns hide. Views change
//! no data. A shared view is for everyone who sees the sheet and needs
//! sheets.edit to make or change; a personal one is its owner's alone.
//!
//! Replacing what a search found is an ordinary `sheet_write` with the
//! revision the search returned as each cell's `ifRevision`.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{address, text_of};
use crate::{
    events::{self, Audience, NewEvent, catalog},
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, access_shim, refs::IdRef, target::Located,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<SheetSearch>()
        .add::<SheetViewList>()
        .add::<SheetViewCreate>()
        .add::<SheetViewUpdate>()
        .add::<SheetViewDelete>()
        .add::<SheetRangeSet>()
        .add::<super::row_ops::SheetRowAdd>()
        .add::<super::chart_ops::SheetChartSet>()
        .add::<super::chart_ops::SheetChartGet>();
}

const SHEET: &str = "That sheet";
const VIEW: &str = "That view";
const MAX_QUERY_CHARACTERS: usize = 200;
const MAX_FILTERS: usize = 20;
const MAX_VIEWS_PER_TAB: i64 = 50;

/// A tab of the sheet by id or name (ignoring case), or the first.
async fn tab_of(
    connection: &mut sqlx::PgConnection,
    sheet_id: Uuid,
    reference: Option<&str>,
) -> Result<Option<super::Tab>, OpError> {
    let tabs = super::tabs(&mut *connection, sheet_id).await?;
    Ok(
        match reference.map(str::trim).filter(|value| !value.is_empty()) {
            None => tabs.into_iter().next(),
            Some(reference) => {
                let id = reference.parse::<Uuid>().ok();
                tabs.into_iter()
                    .find(|tab| Some(tab.id) == id || tab.name.eq_ignore_ascii_case(reference))
            }
        },
    )
}

// ------------------------------------------------------------------ search

pub struct SheetSearch;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetSearchInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// What to look for (up to 200 characters).
    pub query: String,
    /// Only this tab, by name or id. Leave out to search every tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// Match upper and lower case exactly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub match_case: bool,
    /// The whole cell must match, not just part of it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole_cell: bool,
    /// Look in formulas as typed (=SUM(B2:B9)) as well as in what cells show.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_formulas: bool,
    /// How many matches, in reading order (default 200, at most 1,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for SheetSearchInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetMatch {
    pub tab: String,
    /// Such as "B12".
    pub cell: String,
    /// What's typed in it.
    pub input: String,
    /// What it shows.
    pub shown: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetSearchOutput {
    /// The sheet's revision: pass it as each cell's ifRevision to replace
    /// what was found with sheet_write.
    pub revision: i64,
    pub matches: Vec<SheetMatch>,
    /// More matched than came back.
    pub truncated: bool,
}

/// Whether `text` holds `query` (as prepared by the caller).
fn matches(text: &str, query: &str, match_case: bool, whole_cell: bool) -> bool {
    let folded;
    let text = if match_case {
        text
    } else {
        folded = text.to_lowercase();
        &folded
    };
    if whole_cell {
        text.trim() == query.trim()
    } else {
        text.contains(query)
    }
}

impl Operation for SheetSearch {
    const NAME: &'static str = "sheet_search";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Find in a sheet";
    const DESCRIPTION: &'static str = "Find cells in a sheet by what they show (or, with \
        inFormulas, by their formulas), in reading order, with the sheet's revision. To replace \
        what was found, write the new inputs with sheet_write, giving that revision as each \
        cell's ifRevision so a cell someone changed since isn't overwritten.";

    type Input = SheetSearchInput;
    type Output = SheetSearchOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetSearchInput,
    ) -> Result<SheetSearchOutput, OpError> {
        let query = input.query.trim_end_matches(['\n', '\r']);
        if query.trim().is_empty() || query.chars().count() > MAX_QUERY_CHARACTERS {
            return Err(OpError::invalid(
                "query",
                "Say what to look for, in up to 200 characters.",
            ));
        }
        let query = if input.match_case {
            query.to_owned()
        } else {
            query.to_lowercase()
        };
        let limit = usize::from(input.limit.unwrap_or(200).clamp(1, 1_000));
        let sheet_id = input.sheet_id.id();
        let tx = call.tx().await?;
        let revision = sqlx::query_scalar::<_, i64>("SELECT revision FROM sheets WHERE id = $1")
            .bind(sheet_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?
            .ok_or(OpError::not_found(SHEET))?;
        let tabs = super::tabs(&mut **tx, sheet_id).await?;
        let only = match input.tab.as_deref() {
            Some(reference) => Some(
                tab_of(tx, sheet_id, Some(reference))
                    .await?
                    .ok_or(OpError::invalid("tab", "That sheet has no such tab."))?
                    .id,
            ),
            None => None,
        };
        let cells = super::cells(&mut **tx, sheet_id).await?;
        let mut found = Vec::new();
        let mut truncated = false;
        for tab in tabs.iter().filter(|tab| only.is_none_or(|id| id == tab.id)) {
            for cell in cells
                .iter()
                .filter(|cell| cell.tab_id == tab.id && cell.filled())
            {
                let shown = cell
                    .display
                    .clone()
                    .unwrap_or_else(|| text_of(&cell.value()));
                let formula = input.in_formulas && cell.input.starts_with('=');
                if matches(&shown, &query, input.match_case, input.whole_cell)
                    || (formula && matches(&cell.input, &query, input.match_case, input.whole_cell))
                    || (!cell.input.starts_with('=')
                        && shown != cell.input
                        && matches(&cell.input, &query, input.match_case, input.whole_cell))
                {
                    if found.len() == limit {
                        truncated = true;
                        break;
                    }
                    found.push(SheetMatch {
                        tab: tab.name.clone(),
                        cell: format!(
                            "{}{}",
                            address::column_name(cell.column_number),
                            cell.row_number
                        ),
                        input: cell.input.clone(),
                        shown,
                    });
                }
            }
            if truncated {
                break;
            }
        }
        Ok(SheetSearchOutput {
            revision,
            matches: found,
            truncated,
        })
    }
}

// ------------------------------------------------------------------ views

/// How a filter compares a column's values.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    GreaterThan,
    LessThan,
    AtLeast,
    AtMost,
    Empty,
    NotEmpty,
    /// Any of `values`.
    OneOf,
}

/// Keep the rows whose value in `column` passes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewFilter {
    /// The column, by its letters (such as "C").
    pub column: String,
    pub op: FilterOp,
    /// What to compare with: text, a number or true/false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Json>")]
    pub value: Option<Json>,
    /// With oneOf: the values to keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "Vec<Json>")]
    pub values: Vec<Json>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSort {
    /// The column, by its letters.
    pub column: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub descending: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetViewOut {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub sheet_id: Uuid,
    #[schemars(with = "String")]
    pub tab_id: Uuid,
    pub tab: String,
    pub name: String,
    /// Everyone who sees the sheet sees it; otherwise it's yours alone.
    pub shared: bool,
    pub filters: Vec<ViewFilter>,
    pub sort: Vec<ViewSort>,
    /// Columns it hides, by their letters.
    pub hidden_columns: Vec<String>,
    /// Pass it back as expectedVersion to change the view.
    pub version: i64,
    pub updated_at: String,
}

#[derive(FromRow)]
struct ViewRow {
    id: Uuid,
    sheet_id: Uuid,
    tab_id: Uuid,
    tab: String,
    name: String,
    owner_actor_id: Option<Uuid>,
    filters: Json,
    sort: Json,
    hidden_columns: Json,
    version: i64,
    updated_at: String,
}

impl From<ViewRow> for SheetViewOut {
    fn from(row: ViewRow) -> Self {
        Self {
            id: row.id,
            sheet_id: row.sheet_id,
            tab_id: row.tab_id,
            tab: row.tab,
            name: row.name,
            shared: row.owner_actor_id.is_none(),
            filters: serde_json::from_value(row.filters).unwrap_or_default(),
            sort: serde_json::from_value(row.sort).unwrap_or_default(),
            hidden_columns: serde_json::from_value(row.hidden_columns).unwrap_or_default(),
            version: row.version,
            updated_at: row.updated_at,
        }
    }
}

const VIEW_SELECT: &str = "SELECT view.id, view.sheet_id, view.tab_id, tab.name AS tab, view.name, \
     view.owner_actor_id, view.filters, view.sort, view.hidden_columns, view.version, \
     to_char(view.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS updated_at \
     FROM sheet_views view JOIN sheet_tabs tab ON tab.id = view.tab_id";

async fn view_by_id(
    connection: &mut sqlx::PgConnection,
    id: Uuid,
    viewer: Uuid,
) -> Result<Option<(SheetViewOut, Option<Uuid>)>, OpError> {
    let row = sqlx::query_as::<_, ViewRow>(&format!(
        "{VIEW_SELECT} WHERE view.id = $1 \
           AND (view.owner_actor_id IS NULL OR view.owner_actor_id = $2) \
           AND subject_visible_to('sheet', view.sheet_id, $2)"
    ))
    .bind(id)
    .bind(viewer)
    .fetch_optional(connection)
    .await
    .map_err(db)?;
    Ok(row.map(|row| {
        let owner = row.owner_actor_id;
        (SheetViewOut::from(row), owner)
    }))
}

fn name_of(value: &str) -> Result<String, OpError> {
    let name = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 80 {
        return Err(OpError::invalid(
            "name",
            "A view's name is 1 to 80 characters.",
        ));
    }
    Ok(name)
}

fn column_ok(letters: &str) -> bool {
    address::column_number(letters.trim()).is_some_and(|column| column <= super::MAX_COLUMNS)
}

fn check_filters(filters: &[ViewFilter]) -> Result<(), OpError> {
    if filters.len() > MAX_FILTERS {
        return Err(OpError::invalid("filters", "A view has up to 20 filters."));
    }
    for filter in filters {
        if !column_ok(&filter.column) {
            return Err(OpError::invalid(
                "filters",
                "Name each filter's column by its letters, such as \"C\".",
            ));
        }
        let scalar =
            |value: &Json| matches!(value, Json::String(_) | Json::Number(_) | Json::Bool(_));
        let ok = match filter.op {
            FilterOp::Empty | FilterOp::NotEmpty => true,
            FilterOp::OneOf => {
                !filter.values.is_empty()
                    && filter.values.len() <= 200
                    && filter.values.iter().all(scalar)
            }
            FilterOp::GreaterThan | FilterOp::LessThan | FilterOp::AtLeast | FilterOp::AtMost => {
                matches!(filter.value, Some(Json::Number(_)))
            }
            _ => filter.value.as_ref().is_some_and(scalar),
        };
        if !ok {
            return Err(OpError::invalid(
                "filters",
                "Each filter needs a value that fits it: a number to compare, text or a number to match, values for oneOf, none for empty and notEmpty.",
            ));
        }
    }
    Ok(())
}

fn check_sort(sort: &[ViewSort]) -> Result<(), OpError> {
    if sort.len() > 3 || sort.iter().any(|key| !column_ok(&key.column)) {
        return Err(OpError::invalid(
            "sort",
            "Sort by up to three columns, named by their letters.",
        ));
    }
    Ok(())
}

fn check_hidden(hidden: &[String]) -> Result<Vec<String>, OpError> {
    if hidden.len() > 200 || hidden.iter().any(|letters| !column_ok(letters)) {
        return Err(OpError::invalid(
            "hiddenColumns",
            "Name hidden columns by their letters, such as \"C\".",
        ));
    }
    Ok(hidden
        .iter()
        .map(|letters| letters.trim().to_ascii_uppercase())
        .collect())
}

/// A shared view is made and changed by people who can edit the sheet.
async fn may_share(call: &OpCall<'_>) -> Result<(), OpError> {
    let located = Located {
        workspace_id: call.ctx.workspace_id,
        space_id: call.ctx.space_id,
    };
    let mut connection = crate::db::scoped_tx(&call.state.pool)
        .await
        .map_err(crate::error::AppError::from)?;
    access_shim::check(
        &mut connection,
        &call.ctx.caller,
        &call.ctx.target,
        &located,
        Some(Perm::SheetsEdit),
        true,
        "Change a shared view",
    )
    .await?;
    Ok(())
}

/// Tells the sheet's open screens (or, for a personal view, its owner's).
async fn announce(
    call: &mut OpCall<'_>,
    sheet_id: Uuid,
    owner: Option<Uuid>,
) -> Result<(), OpError> {
    let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let actor = call.account_id();
    let workspace = call.ctx.workspace_id;
    let mut event = NewEvent::thing(&catalog::SHEET_VIEWS_CHANGED, space, "sheet", sheet_id)
        .actor(actor)
        .workspace(workspace);
    if let Some(owner) = owner {
        event = event.audience(Audience::Account(owner));
    }
    let tx = call.tx().await?;
    events::emit(tx, event).await?;
    Ok(())
}

fn taken() -> OpError {
    OpError::Conflict {
        reason: ConflictReason::State { code: "name_taken" },
        message: "A view on that tab already has that name.".into(),
        current: None,
    }
}

fn db(error: sqlx::Error) -> OpError {
    crate::error::AppError::from(error).into()
}

fn unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

// list

pub struct SheetViewList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetViewListInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// Only this tab's views, by name or id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

impl Targeted for SheetViewListInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetViewListOutput {
    /// Shared views and your own, by tab then name.
    pub views: Vec<SheetViewOut>,
}

impl Operation for SheetViewList {
    const NAME: &'static str = "sheet_view_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a sheet's views";
    const DESCRIPTION: &'static str = "List a sheet's saved filter views: the shared ones and \
        your own. A view says which rows of a tab show (filters by column), in what order, and \
        which columns hide; it changes no data.";

    type Input = SheetViewListInput;
    type Output = SheetViewListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetViewListInput,
    ) -> Result<SheetViewListOutput, OpError> {
        let viewer = call.account_id();
        let sheet_id = input.sheet_id.id();
        let tx = call.tx().await?;
        let tab = match input.tab.as_deref() {
            Some(reference) => Some(
                tab_of(tx, sheet_id, Some(reference))
                    .await?
                    .ok_or(OpError::invalid("tab", "That sheet has no such tab."))?
                    .id,
            ),
            None => None,
        };
        let rows = sqlx::query_as::<_, ViewRow>(&format!(
            "{VIEW_SELECT} WHERE view.sheet_id = $1 AND ($2::uuid IS NULL OR view.tab_id = $2) \
               AND (view.owner_actor_id IS NULL OR view.owner_actor_id = $3) \
               AND subject_visible_to('sheet', view.sheet_id, $3) \
             ORDER BY tab.position, lower(view.name), view.id"
        ))
        .bind(sheet_id)
        .bind(tab)
        .bind(viewer)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        Ok(SheetViewListOutput {
            views: rows.into_iter().map(SheetViewOut::from).collect(),
        })
    }
}

// create

pub struct SheetViewCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetViewCreateInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// The tab, by name or id. Defaults to the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub name: String,
    /// Share it with everyone who sees the sheet (needs sheets.edit).
    /// Otherwise it's yours alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    /// Rows show when they pass every filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<ViewFilter>,
    /// The order rows show in; the data stays as it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<ViewSort>,
    /// Columns to hide, by their letters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_columns: Vec<String>,
}

impl Targeted for SheetViewCreateInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

impl Operation for SheetViewCreate {
    const NAME: &'static str = "sheet_view_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "sheets";
    // Anyone who sees the sheet can keep a view of their own; a shared one
    // asks for sheets.edit in `run`.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Save a sheet view";
    const DESCRIPTION: &'static str = "Save a filter view of a sheet's tab: filters by column \
        (equals, contains, greaterThan, empty, oneOf and the like), a display order and hidden \
        columns. It changes no data. Yours alone unless shared (sharing needs sheets.edit).";
    const EMITS: &'static [&'static str] = &["sheet.views_changed"];

    type Input = SheetViewCreateInput;
    type Output = SheetViewOut;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetViewCreateInput,
    ) -> Result<SheetViewOut, OpError> {
        let name = name_of(&input.name)?;
        check_filters(&input.filters)?;
        check_sort(&input.sort)?;
        let hidden = check_hidden(&input.hidden_columns)?;
        if input.shared {
            may_share(call).await?;
        }
        let viewer = call.account_id();
        let owner = (!input.shared).then_some(viewer);
        let sheet_id = input.sheet_id.id();
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        let tab = tab_of(tx, sheet_id, input.tab.as_deref())
            .await?
            .ok_or(OpError::invalid("tab", "That sheet has no such tab."))?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sheet_views WHERE tab_id = $1 \
             AND (owner_actor_id IS NULL OR owner_actor_id = $2)",
        )
        .bind(tab.id)
        .bind(viewer)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
        if count >= MAX_VIEWS_PER_TAB {
            return Err(OpError::invalid(
                "tab",
                "A tab keeps up to 50 views you can see. Remove one first.",
            ));
        }
        let inserted = sqlx::query(
            "INSERT INTO sheet_views (id, workspace_id, space_id, sheet_id, tab_id, name, \
               owner_actor_id, filters, sort, hidden_columns, created_by_actor_id) \
             SELECT $1, project.organization_id, sheet.project_id, sheet.id, $3, $4, $5, $6, $7, \
                    $8, $9 \
             FROM sheets sheet JOIN projects project ON project.id = sheet.project_id \
             WHERE sheet.id = $2",
        )
        .bind(id)
        .bind(sheet_id)
        .bind(tab.id)
        .bind(&name)
        .bind(owner)
        .bind(json!(input.filters))
        .bind(json!(input.sort))
        .bind(json!(hidden))
        .bind(viewer)
        .execute(&mut **tx)
        .await;
        match inserted {
            Err(error) if unique_violation(&error) => return Err(taken()),
            other => {
                if other.map_err(db)?.rows_affected() == 0 {
                    return Err(OpError::not_found(SHEET));
                }
            }
        }
        let (view, _) = view_by_id(tx, id, viewer)
            .await?
            .ok_or(OpError::Unavailable)?;
        announce(call, sheet_id, owner).await?;
        call.audit_target(
            "sheet_view",
            id,
            json!({"sheet": sheet_id, "shared": input.shared}),
        );
        Ok(view)
    }
}

// update

pub struct SheetViewUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetViewUpdateInput {
    /// The view: its id, from sheet_view_list.
    pub view_id: IdRef,
    /// The version you read; a view changed since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replaces the filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<ViewFilter>>,
    /// Replaces the order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<ViewSort>>,
    /// Replaces the hidden columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_columns: Option<Vec<String>>,
}

impl Targeted for SheetViewUpdateInput {
    fn target(&self) -> Target {
        Target::Thing {
            kind: "sheet_view",
            id: self.view_id.id(),
            expected_version: self.expected_version,
        }
    }
}

impl Operation for SheetViewUpdate {
    const NAME: &'static str = "sheet_view_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change a sheet view";
    const DESCRIPTION: &'static str = "Rename a saved sheet view or replace its filters, order \
        or hidden columns. A shared view needs sheets.edit; your own needs nothing more.";
    const EMITS: &'static [&'static str] = &["sheet.views_changed"];

    type Input = SheetViewUpdateInput;
    type Output = SheetViewOut;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetViewUpdateInput,
    ) -> Result<SheetViewOut, OpError> {
        let name = input.name.as_deref().map(name_of).transpose()?;
        if let Some(filters) = &input.filters {
            check_filters(filters)?;
        }
        if let Some(sort) = &input.sort {
            check_sort(sort)?;
        }
        let hidden = input
            .hidden_columns
            .as_deref()
            .map(check_hidden)
            .transpose()?;
        let viewer = call.account_id();
        let id = input.view_id.id();
        let (current, owner) = {
            let tx = call.tx().await?;
            view_by_id(tx, id, viewer)
                .await?
                .ok_or(OpError::not_found(VIEW))?
        };
        if owner.is_none() {
            may_share(call).await?;
        }
        if let Some(expected) = input.expected_version
            && expected != current.version
        {
            return Err(OpError::Conflict {
                reason: ConflictReason::Version {
                    current_version: current.version,
                },
                message: "That view changed since you read it. Read it again and retry.".into(),
                current: None,
            });
        }
        let tx = call.tx().await?;
        let updated = sqlx::query(
            "UPDATE sheet_views SET name = COALESCE($2, name), \
               filters = COALESCE($3, filters), sort = COALESCE($4, sort), \
               hidden_columns = COALESCE($5, hidden_columns), version = version + 1, \
               updated_at = now() \
             WHERE id = $1 AND ($6::bigint IS NULL OR version = $6)",
        )
        .bind(id)
        .bind(name)
        .bind(input.filters.map(|filters| json!(filters)))
        .bind(input.sort.map(|sort| json!(sort)))
        .bind(hidden.map(|hidden| json!(hidden)))
        .bind(input.expected_version)
        .execute(&mut **tx)
        .await;
        // The version is checked again as the row is written, so two
        // updates from one read can't both land.
        match updated {
            Err(error) if unique_violation(&error) => return Err(taken()),
            Ok(done) if done.rows_affected() == 0 => {
                let (now, _) = view_by_id(tx, id, viewer)
                    .await?
                    .ok_or(OpError::not_found(VIEW))?;
                return Err(OpError::Conflict {
                    reason: ConflictReason::Version {
                        current_version: now.version,
                    },
                    message: "That view changed since you read it. Read it again and retry.".into(),
                    current: None,
                });
            }
            other => {
                other.map_err(db)?;
            }
        }
        let (view, _) = view_by_id(tx, id, viewer)
            .await?
            .ok_or(OpError::Unavailable)?;
        announce(call, view.sheet_id, owner).await?;
        Ok(view)
    }
}

// delete

pub struct SheetViewDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetViewDeleteInput {
    /// The view: its id, from sheet_view_list.
    pub view_id: IdRef,
}

impl Targeted for SheetViewDeleteInput {
    fn target(&self) -> Target {
        Target::thing("sheet_view", self.view_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetViewDeleteOutput {
    #[schemars(with = "String")]
    pub deleted: Uuid,
}

impl Operation for SheetViewDelete {
    const NAME: &'static str = "sheet_view_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove a sheet view";
    const DESCRIPTION: &'static str = "Remove a saved sheet view; the sheet's data stays as it \
        is. A shared view needs sheets.edit; your own needs nothing more.";
    const EMITS: &'static [&'static str] = &["sheet.views_changed"];

    type Input = SheetViewDeleteInput;
    type Output = SheetViewDeleteOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetViewDeleteInput,
    ) -> Result<SheetViewDeleteOutput, OpError> {
        let viewer = call.account_id();
        let id = input.view_id.id();
        let (view, owner) = {
            let tx = call.tx().await?;
            view_by_id(tx, id, viewer)
                .await?
                .ok_or(OpError::not_found(VIEW))?
        };
        if owner.is_none() {
            may_share(call).await?;
        }
        let tx = call.tx().await?;
        sqlx::query("DELETE FROM sheet_views WHERE id = $1")
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        announce(call, view.sheet_id, owner).await?;
        Ok(SheetViewDeleteOutput { deleted: id })
    }
}

// ------------------------------------------------------------ named ranges

pub struct SheetRangeSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetRangeSetInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// The range's name, such as "Budget": a letter first, then letters,
    /// digits, _ and dots. Setting a name the sheet has moves it.
    pub name: String,
    /// The block it names: "A1:B12" on the first tab, or "Summary!A1:B12".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// Remove the named range instead (the cells stay as they are).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

impl Targeted for SheetRangeSetInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetRangesOutput {
    /// The sheet's named ranges now, by name.
    pub ranges: Vec<super::ranges::NamedRange>,
}

impl Operation for SheetRangeSet {
    const NAME: &'static str = "sheet_range_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = Some(Perm::SheetsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Name a sheet range";
    const DESCRIPTION: &'static str = "Give a block of a sheet a name, such as Budget for \
        Summary!A1:B12, or move or remove one. Dashboards, page embeds and sheet_read read a \
        named range by its name; it moves with rows and columns inserted around it.";
    const EMITS: &'static [&'static str] = &["sheet.ranges_changed"];

    type Input = SheetRangeSetInput;
    type Output = SheetRangesOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetRangeSetInput,
    ) -> Result<SheetRangesOutput, OpError> {
        let name = super::ranges::name_of(&input.name).ok_or(OpError::invalid(
            "name",
            "A range's name starts with a letter and holds letters, digits, _ and dots (up to \
             60), and isn't a cell's name such as Q1.",
        ))?;
        let sheet_id = input.sheet_id.id();
        let actor = call.account_id();
        let tx = call.tx().await?;
        // The sheet's row orders writes to its names.
        let found = sqlx::query_scalar::<_, Uuid>("SELECT id FROM sheets WHERE id = $1 FOR UPDATE")
            .bind(sheet_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
        if found.is_none() {
            return Err(OpError::not_found(SHEET));
        }
        let id = if input.remove {
            sqlx::query_scalar::<_, Uuid>(
                "DELETE FROM sheet_named_ranges WHERE sheet_id = $1 AND lower(name) = lower($2) \
                 RETURNING id",
            )
            .bind(sheet_id)
            .bind(&name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?
            .ok_or(OpError::not_found("That named range"))?
        } else {
            let reference = input.range.as_deref().unwrap_or("").trim();
            let located = super::ranges::locate_block(tx, sheet_id, None, reference)
                .await?
                .ok_or(OpError::invalid(
                    "range",
                    "Name a block of the sheet, such as A1:B12 or Summary!A1:B12.",
                ))?;
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sheet_named_ranges \
                 WHERE sheet_id = $1 AND lower(name) <> lower($2)",
            )
            .bind(sheet_id)
            .bind(&name)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
            if count >= super::ranges::MAX_NAMED_RANGES {
                return Err(OpError::invalid(
                    "name",
                    "A sheet keeps up to 100 named ranges. Remove one first.",
                ));
            }
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO sheet_named_ranges (id, workspace_id, space_id, sheet_id, tab_id, \
                   name, first_row, last_row, first_column, last_column, created_by_actor_id) \
                 SELECT $1, project.organization_id, sheet.project_id, sheet.id, $3, $4, $5, $6, \
                        $7, $8, $9 \
                 FROM sheets sheet JOIN projects project ON project.id = sheet.project_id \
                 WHERE sheet.id = $2 \
                 ON CONFLICT (sheet_id, lower(name)) DO UPDATE SET name = EXCLUDED.name, \
                   tab_id = EXCLUDED.tab_id, first_row = EXCLUDED.first_row, \
                   last_row = EXCLUDED.last_row, first_column = EXCLUDED.first_column, \
                   last_column = EXCLUDED.last_column, updated_at = now() \
                 RETURNING id",
            )
            .bind(Uuid::now_v7())
            .bind(sheet_id)
            .bind(located.tab_id)
            .bind(&name)
            .bind(located.range.rows.0)
            .bind(located.range.rows.1)
            .bind(located.range.columns.0)
            .bind(located.range.columns.1)
            .bind(actor)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?
        };
        let ranges = super::ranges::named(&mut **tx, sheet_id).await?;
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let event = NewEvent::thing(&catalog::SHEET_RANGES_CHANGED, space, "sheet", sheet_id)
            .payload(json!({"sheetId": sheet_id}))
            .actor(actor)
            .workspace(workspace);
        let tx = call.tx().await?;
        events::emit(tx, event).await?;
        call.audit_target(
            "sheet_named_range",
            id,
            json!({"sheet": sheet_id, "removed": input.remove}),
        );
        Ok(SheetRangesOutput { ranges })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_part_or_whole_and_case() {
        assert!(matches("Rent due", "rent", false, false));
        assert!(!matches("Rent due", "rent", true, false));
        assert!(matches(" Rent ", "rent", false, true));
        assert!(!matches("Rent due", "rent", false, true));
    }

    #[test]
    fn filters_need_values_that_fit() {
        let filter = |op, value: Option<Json>| ViewFilter {
            column: "C".to_owned(),
            op,
            value,
            values: Vec::new(),
        };
        assert!(check_filters(&[filter(FilterOp::GreaterThan, Some(json!(3)))]).is_ok());
        assert!(check_filters(&[filter(FilterOp::GreaterThan, Some(json!("3")))]).is_err());
        assert!(check_filters(&[filter(FilterOp::Empty, None)]).is_ok());
        assert!(check_filters(&[filter(FilterOp::Contains, None)]).is_err());
        assert!(check_filters(&[filter(FilterOp::OneOf, None)]).is_err());
        let mut bad = filter(FilterOp::Equals, Some(json!("x")));
        bad.column = "C1".to_owned();
        assert!(check_filters(&[bad]).is_err());
    }
}
