//! Dashboards (T1.32): a project's pages of tiles. A tile shows one of a
//! collection's views (a chart, one number, the first few records) or a
//! note, or a block of a sheet as a chart (C1.20). A dashboard is shared with everyone in the project, or personal:
//! only its owner sees it. A tile names a view and never holds its data:
//! whoever looks reads the view as themselves, so a tile over records they
//! can't see says so, and the rest of the dashboard still shows.

pub mod ops;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    collections::{self, query, records},
    error::AppError,
    views::{ViewKind, definition as views},
};

/// Dashboards one project holds, shared and personal together.
pub const MAX_DASHBOARDS: i64 = 100;
/// Tiles one dashboard holds.
pub const MAX_TILES: usize = 24;
pub const MAX_NAME_CHARACTERS: usize = 80;
pub const MAX_TITLE_CHARACTERS: usize = 80;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 2_000;
pub const MAX_TEXT_CHARACTERS: usize = 2_000;
/// Records a list tile shows.
pub const LIST_TILE_RECORDS: i64 = 5;

/// What a tile shows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TileKind {
    /// A collection's view: a chart view as its chart, any other as a
    /// count and its first few records.
    View,
    /// A note.
    Text,
    /// A block of a sheet (a named range, or such as Summary!A1:B9) as a
    /// chart: labels from its first column, numbers from its second.
    Sheet,
}

/// A tile as stored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    pub id: Uuid,
    pub kind: TileKind,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Uuid>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A sheet tile's sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<Uuid>,
    /// A sheet tile's block: a named range's name, or such as "A1:B9" or
    /// "Summary!A1:B9".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub range: String,
    /// A sheet tile's look; a bar chart when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<views::ChartStyle>,
    /// Columns of twelve it spans.
    pub width: u8,
    /// Rows it spans, 1 to 4.
    pub height: u8,
}

#[derive(Clone, Debug)]
pub struct Dashboard {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub owner_actor_id: Option<Uuid>,
    pub tiles: Vec<Tile>,
    pub position: i32,
    pub version: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(FromRow)]
struct Row {
    id: Uuid,
    project_id: Uuid,
    name: String,
    description: String,
    owner_actor_id: Option<Uuid>,
    tiles: Value,
    position: i32,
    version: i64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Row> for Dashboard {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            description: row.description,
            owner_actor_id: row.owner_actor_id,
            // A tile the server can't read back is left out rather than
            // failing the whole dashboard.
            tiles: row
                .tiles
                .as_array()
                .map(|tiles| {
                    tiles
                        .iter()
                        .filter_map(|tile| serde_json::from_value(tile.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
            position: row.position,
            version: row.version,
            updated_at: row.updated_at,
        }
    }
}

const COLUMNS: &str = "id, space_id AS project_id, name, description, owner_actor_id, tiles, position, version, updated_at";

/// The dashboards of a project `reader` sees: the shared ones in order,
/// then their own.
pub async fn visible(
    connection: &mut PgConnection,
    project_id: Uuid,
    reader: Uuid,
) -> Result<Vec<Dashboard>, AppError> {
    let rows = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM dashboards \
         WHERE space_id = $1 AND (owner_actor_id IS NULL OR owner_actor_id = $2) \
         ORDER BY owner_actor_id IS NOT NULL, position, lower(name), id"
    ))
    .bind(project_id)
    .bind(reader)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows.into_iter().map(Dashboard::from).collect())
}

/// One dashboard `reader` sees, locked for a write when `lock` is set.
pub async fn one(
    connection: &mut PgConnection,
    project_id: Uuid,
    id: Uuid,
    reader: Uuid,
    lock: bool,
) -> Result<Option<Dashboard>, AppError> {
    let row = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM dashboards \
         WHERE space_id = $1 AND id = $2 AND (owner_actor_id IS NULL OR owner_actor_id = $3){}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(project_id)
    .bind(id)
    .bind(reader)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.map(Dashboard::from))
}

pub struct NewDashboard<'a> {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: &'a str,
    pub description: &'a str,
    pub owner_actor_id: Option<Uuid>,
    pub tiles: &'a [Tile],
    pub created_by: Uuid,
}

pub enum Written {
    Done(Dashboard),
    NameTaken,
    Full,
}

fn tiles_json(tiles: &[Tile]) -> Result<Value, AppError> {
    serde_json::to_value(tiles).map_err(|_| AppError::Internal("encode dashboard tiles"))
}

fn name_taken(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.constraint())
        .is_some_and(|constraint| constraint == "dashboards_name_unique")
}

pub async fn create(
    connection: &mut PgConnection,
    new: NewDashboard<'_>,
) -> Result<Written, AppError> {
    let (count, position) = sqlx::query_as::<_, (i64, i32)>(
        "SELECT count(*), COALESCE(max(position) + 1, 0) FROM dashboards \
         WHERE space_id = $1 AND owner_actor_id IS NOT DISTINCT FROM $2",
    )
    .bind(new.project_id)
    .bind(new.owner_actor_id)
    .fetch_one(&mut *connection)
    .await?;
    let all = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM dashboards WHERE space_id = $1")
        .bind(new.project_id)
        .fetch_one(&mut *connection)
        .await?;
    if all >= MAX_DASHBOARDS || count >= MAX_DASHBOARDS {
        return Ok(Written::Full);
    }
    let inserted = sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO dashboards (id, workspace_id, space_id, name, description, owner_actor_id, \
         tiles, position, created_by_actor_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING {COLUMNS}"
    ))
    .bind(Uuid::now_v7())
    .bind(new.workspace_id)
    .bind(new.project_id)
    .bind(new.name)
    .bind(new.description)
    .bind(new.owner_actor_id)
    .bind(tiles_json(new.tiles)?)
    .bind(position)
    .bind(new.created_by)
    .fetch_one(&mut *connection)
    .await;
    match inserted {
        Ok(row) => Ok(Written::Done(row.into())),
        Err(error) if name_taken(&error) => Ok(Written::NameTaken),
        Err(error) => Err(error.into()),
    }
}

pub struct Change<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub tiles: Option<&'a [Tile]>,
    /// A new owner: `Some(None)` shares it with the project,
    /// `Some(Some(account))` makes it that account's alone.
    pub owner: Option<Option<Uuid>>,
}

/// Changes a dashboard already read under lock; bumps its version.
pub async fn change(
    connection: &mut PgConnection,
    current: &Dashboard,
    change: Change<'_>,
) -> Result<Written, AppError> {
    let tiles = change.tiles.map(tiles_json).transpose()?;
    let updated = sqlx::query_as::<_, Row>(&format!(
        "UPDATE dashboards SET name = COALESCE($3, name), \
         description = COALESCE($4, description), tiles = COALESCE($5, tiles), \
         owner_actor_id = CASE WHEN $6 THEN $7 ELSE owner_actor_id END, \
         version = version + 1, updated_at = now() \
         WHERE space_id = $1 AND id = $2 RETURNING {COLUMNS}"
    ))
    .bind(current.project_id)
    .bind(current.id)
    .bind(change.name)
    .bind(change.description)
    .bind(tiles)
    .bind(change.owner.is_some())
    .bind(change.owner.flatten())
    .fetch_one(&mut *connection)
    .await;
    match updated {
        Ok(row) => Ok(Written::Done(row.into())),
        Err(error) if name_taken(&error) => Ok(Written::NameTaken),
        Err(error) => Err(error.into()),
    }
}

/// Removes a dashboard (its tiles go with it; the views they named stay).
pub async fn remove(connection: &mut PgConnection, current: &Dashboard) -> Result<(), AppError> {
    sqlx::query("DELETE FROM dashboards WHERE space_id = $1 AND id = $2")
        .bind(current.project_id)
        .bind(current.id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// A view tile's figures, read as `reader`.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Figures {
    /// The view's kind: table, board, list, gallery, calendar, timeline or
    /// chart.
    pub view_type: String,
    pub view_name: String,
    pub collection_name: String,
    /// Records the view finds.
    pub total: i64,
    /// A chart's measure over every record, when it isn't a count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// A chart's look: bar, line, donut or number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// What the chart shows: "count", or "sum", "average", "min" or "max"
    /// of a field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure: Option<String>,
    /// A chart's groups, in order, with their counts (and values).
    pub groups: Vec<FigureGroup>,
    /// Any other view: its first few records' titles.
    pub records: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FigureGroup {
    /// The group's value, stable across reads (an option's id, a date).
    pub key: String,
    pub name: String,
    /// An option's colour, when the group is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

/// Why a tile shows no figures.
pub enum Unseen {
    /// The view (or its collection) is gone, or isn't one `reader` sees.
    Gone,
    /// The records are tasks, which `reader`'s role can't see.
    Tasks,
    /// A sheet tile's range is gone: its name was removed, or its cells.
    Range,
}

/// A view tile's figures as `reader` sees them. `can_view_tasks` is whether
/// their role sees task records.
pub async fn figures(
    connection: &mut PgConnection,
    project_id: Uuid,
    tile: &Tile,
    reader: Uuid,
    can_view_tasks: bool,
) -> Result<Result<Figures, Unseen>, AppError> {
    let (Some(collection_id), Some(view_id)) = (tile.collection, tile.view) else {
        return Ok(Err(Unseen::Gone));
    };
    let Some(collection) = collections::get(&mut *connection, project_id, collection_id).await?
    else {
        return Ok(Err(Unseen::Gone));
    };
    if collection.acts_like_task && !can_view_tasks {
        return Ok(Err(Unseen::Tasks));
    }
    let Some(view) = views::one(&mut *connection, project_id, collection_id, view_id).await? else {
        return Ok(Err(Unseen::Gone));
    };
    if view.owner_actor_id.is_some_and(|owner| owner != reader) {
        return Ok(Err(Unseen::Gone));
    }
    let fields =
        collections::view_fields(&mut *connection, project_id, collection_id, true).await?;
    let config = view.config.active(&fields);
    let chart = view.view_type == ViewKind::Chart;
    let mut spec = query::Spec::from_config(&config, &fields, reader);
    let source = collections::source(&collection);
    let style_number = config
        .chart
        .is_some_and(|chart| chart.style == views::ChartStyle::Number);
    if !chart || style_number {
        spec.group = None;
    }
    let total = query::total(&mut *connection, source.as_ref(), &spec).await?;
    let mut figures = Figures {
        view_type: view.view_type.as_str().to_owned(),
        view_name: view.name.clone(),
        collection_name: collection.name.clone(),
        total,
        value: None,
        style: None,
        measure: None,
        groups: Vec::new(),
        records: Vec::new(),
    };
    if chart {
        let measure = config.chart.map(|chart| chart.measure).unwrap_or_default();
        figures.style = Some(
            match config.chart.map(|chart| chart.style).unwrap_or_default() {
                views::ChartStyle::Bar => "bar",
                views::ChartStyle::Line => "line",
                views::ChartStyle::Donut => "donut",
                views::ChartStyle::Number => "number",
            }
            .to_owned(),
        );
        figures.measure = Some(
            match measure
                .field
                .and_then(|id| fields.iter().find(|field| field.id == id))
            {
                Some(field) if measure.op != views::MeasureOp::Count => {
                    format!("{} of {}", measure.op.as_str(), field.name)
                }
                _ => "count".to_owned(),
            },
        );
        figures.value = query::measure_total(&mut *connection, source.as_ref(), &spec).await?;
        figures.groups = query::group_counts(&mut *connection, source.as_ref(), &spec)
            .await?
            .into_iter()
            .map(|group| FigureGroup {
                key: group.key,
                name: group.label,
                color: group.color,
                count: group.count,
                value: group.value,
            })
            .collect();
    } else {
        let Ok(page) = query::page(
            &mut *connection,
            source.as_ref(),
            &spec,
            None,
            LIST_TILE_RECORDS,
        )
        .await?
        else {
            return Ok(Ok(figures));
        };
        let ids = page.rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let found = records::by_ids(
            &mut *connection,
            project_id,
            collection_id,
            &ids,
            collection.acts_like_task,
        )
        .await?;
        figures.records = ids
            .iter()
            .filter_map(|id| found.iter().find(|record| record.id == *id))
            .map(|record| {
                if record.title.is_empty() {
                    format!("#{}", record.number)
                } else {
                    record.title.clone()
                }
            })
            .collect();
    }
    Ok(Ok(figures))
}

/// The most bars, points or slices a sheet tile shows.
pub const SHEET_TILE_GROUPS: usize = 50;

fn style_name(style: views::ChartStyle) -> &'static str {
    match style {
        views::ChartStyle::Bar => "bar",
        views::ChartStyle::Line => "line",
        views::ChartStyle::Donut => "donut",
        views::ChartStyle::Number => "number",
    }
}

/// A sheet tile's figures as `reader` sees them (C1.20): each row of the
/// block is a label (its first column) and a number (its second; the first
/// when there's one column). A first row whose number is text names the
/// columns. Rows with the same label add up; rows without a number are
/// left out.
pub async fn sheet_figures(
    connection: &mut PgConnection,
    project_id: Uuid,
    tile: &Tile,
    reader: Uuid,
) -> Result<Result<Figures, Unseen>, AppError> {
    let Some(sheet_id) = tile.sheet else {
        return Ok(Err(Unseen::Gone));
    };
    let name = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sheets WHERE id = $1 AND project_id = $2 AND archived_at IS NULL \
           AND subject_visible_to('sheet', id, $3)",
    )
    .bind(sheet_id)
    .bind(project_id)
    .bind(reader)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(sheet_name) = name else {
        return Ok(Err(Unseen::Gone));
    };
    let style = style_name(tile.style.unwrap_or_default());
    Ok(
        range_figures(&mut *connection, sheet_id, sheet_name, &tile.range, style)
            .await?
            .ok_or(Unseen::Range),
    )
}

/// The figures of a block of a sheet (`reference`: a named range or a
/// block such as Summary!A1:B12), drawn as `style`: labels from its first
/// column, numbers from its second, a header row naming them. `None` when
/// the reference names nothing. The caller has checked the sheet is seen.
pub async fn range_figures(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    sheet_name: String,
    reference: &str,
    style: &str,
) -> Result<Option<Figures>, AppError> {
    use crate::sheets::{engine::Value as Cell, ranges, text_of};
    let Some(located) = ranges::resolve(&mut *connection, sheet_id, reference).await? else {
        return Ok(None);
    };
    let block = ranges::read_range(&mut *connection, located.tab_id, located.range).await?;
    let value_column = usize::from(block.rows.first().is_some_and(|row| row.len() > 1));
    let number = |row: &[Cell]| match row.get(value_column) {
        Some(Cell::Number(number)) => Some(*number),
        _ => None,
    };
    let headed = block.rows.first().is_some_and(|row| {
        matches!(row.get(value_column), Some(Cell::Text(_)))
            && block.rows.iter().skip(1).any(|row| number(row).is_some())
    });
    let header = headed
        .then(|| block.rows[0].get(value_column).map(text_of))
        .flatten()
        .filter(|header| !header.trim().is_empty());
    let mut groups: Vec<FigureGroup> = Vec::new();
    let mut total = 0;
    let mut sum = 0.0;
    for (index, row) in block.rows.iter().enumerate().skip(usize::from(headed)) {
        let Some(value) = number(row) else {
            continue;
        };
        let label = if value_column == 0 {
            format!(
                "Row {}",
                located.range.rows.0 + i32::try_from(index).unwrap_or(0)
            )
        } else {
            row.first()
                .map(text_of)
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        total += 1;
        sum += value;
        if let Some(group) = groups.iter_mut().find(|group| group.name == label) {
            group.count += 1;
            group.value = Some(group.value.unwrap_or(0.0) + value);
        } else if groups.len() < SHEET_TILE_GROUPS {
            groups.push(FigureGroup {
                key: label.clone(),
                name: label,
                color: None,
                count: 1,
                value: Some(value),
            });
        }
    }
    Ok(Some(Figures {
        view_type: "chart".to_owned(),
        view_name: located
            .name
            .unwrap_or_else(|| format!("{}!{}", located.tab, located.range.name())),
        collection_name: sheet_name,
        total,
        value: Some(sum),
        style: Some(style.to_owned()),
        measure: Some(format!("sum of {}", header.as_deref().unwrap_or("values"))),
        groups,
        records: Vec::new(),
    }))
}
