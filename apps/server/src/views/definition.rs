//! A collection's views (AWD-90, AWD-110): a name, a type (a table, a
//! board, a gallery or a calendar) and what it shows, kept in
//! `collection_views.config` as JSON. The filters, sort, grouping and
//! columns name fields by ID and are checked against the collection's
//! fields on every write. A field archived since is left out when a view is
//! read, and comes back with the field.

use std::collections::HashSet;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::filter::Filter;
use crate::{
    error::AppError,
    fields::{self, DateBucket, FieldDefinition, FieldType, LinkTarget},
};

/// Shared views one collection keeps.
pub const MAX_VIEWS: i64 = 30;
/// Views of their own one person keeps in one collection.
pub const MAX_PERSONAL_VIEWS: i64 = 100;
/// Sort keys one view or query takes.
pub const MAX_SORTS: usize = 3;
pub const MIN_COLUMN_WIDTH: u16 = 48;
pub const MAX_COLUMN_WIDTH: u16 = 1_200;

/// A view's kind: the one list in `crate::views` (T1.2).
pub use crate::views::ViewKind as ViewType;

/// The kinds a collection's view can be (T1.14): a table, a board (a column
/// for each value of the field it groups by), a list (one line for each
/// record), a gallery (a card for each record), a calendar (a month, each
/// record on its date) or a timeline (each record a bar from its date to its
/// end date).
pub const COLLECTION_VIEW_TYPES: [ViewType; 7] = [
    ViewType::Table,
    ViewType::Board,
    ViewType::List,
    ViewType::Gallery,
    ViewType::Calendar,
    ViewType::Timeline,
    ViewType::Chart,
];

/// What a chart counts for each group (T1.31): how many records, or the
/// sum, average, least or most of a number field.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum MeasureOp {
    #[default]
    Count,
    Sum,
    Average,
    Min,
    Max,
}

impl MeasureOp {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "count" => Some(Self::Count),
            "sum" | "total" => Some(Self::Sum),
            "average" | "avg" | "mean" => Some(Self::Average),
            "min" | "least" => Some(Self::Min),
            "max" | "most" => Some(Self::Max),
            _ => None,
        }
    }

    /// Its name, as written.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Average => "average",
            Self::Min => "min",
            Self::Max => "max",
        }
    }

    /// The SQL aggregate it is, over a number.
    pub fn sql(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Average => "avg",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// A chart's measure: an operation, and the number field it reads (none
/// for a count).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Measure {
    #[serde(default)]
    pub op: MeasureOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<Uuid>,
}

/// How a chart draws its groups.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ChartStyle {
    #[default]
    Bar,
    Line,
    Donut,
    /// One number: the measure over every record.
    Number,
}

/// A chart view's settings beside the view's filters and grouping (its
/// groups are the chart's bars, points or slices).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChartConfig {
    #[serde(default)]
    pub style: ChartStyle,
    #[serde(default)]
    pub measure: Measure,
}

/// A view kind a collection's view can be, by name.
pub fn parse_view_type(value: &str) -> Option<ViewType> {
    ViewType::parse(value).filter(|kind| COLLECTION_VIEW_TYPES.contains(kind))
}

/// What a view or a query asks for when the type isn't one there is.
pub const VIEW_TYPE_MESSAGE: &str = "A view is a \"table\", \"board\", \"list\", \"gallery\", \"calendar\", \"timeline\" \
     or \"chart\".";

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Asc,
    Desc,
}

/// What records sort by: their title, their number (the order they were
/// made in), when they last changed, or a field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortBy {
    Title,
    Number,
    Updated,
    /// The order on the board, for records that are tasks (their status,
    /// then their place in its column).
    Rank,
    Field(Uuid),
}

impl SortBy {
    /// "title", "number" (or "created"), "updated", or a field's ID.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Ok(id) = value.parse::<Uuid>() {
            return Some(Self::Field(id));
        }
        match value.to_ascii_lowercase().as_str() {
            "title" => Some(Self::Title),
            "number" | "created" | "#" => Some(Self::Number),
            "updated" | "edited" => Some(Self::Updated),
            "rank" | "manual" => Some(Self::Rank),
            _ => None,
        }
    }

    pub fn key(self) -> String {
        match self {
            Self::Title => "title".to_owned(),
            Self::Number => "number".to_owned(),
            Self::Updated => "updated".to_owned(),
            Self::Rank => "rank".to_owned(),
            Self::Field(id) => id.to_string(),
        }
    }
}

/// One sort key as a view stores it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SortRule {
    /// "title", "number", "updated", or a field's ID.
    pub field: String,
    #[serde(default)]
    pub direction: Direction,
}

/// One column of a table view. Fields a view doesn't list show after the
/// ones it does, in the collection's order, so a new field shows up.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Column {
    pub field: Uuid,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// In pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
}

/// What a view shows, as `collection_views.config` stores it: filters with
/// stored values (option IDs, actor IDs or "me", task and page IDs), the
/// sort, the field it groups by and its columns.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewConfig {
    /// A list of clauses (all must hold), or a tree of "and" and "or"
    /// groups (`super::filter`).
    #[serde(default, skip_serializing_if = "Filter::is_empty")]
    pub filters: Filter,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<Uuid>,
    /// Grouped by a date: by day, week or month (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_bucket: Option<DateBucket>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    /// The title column's width, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_width: Option<u16>,
    /// The date field a calendar puts records on; where a timeline's bars
    /// start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_field: Option<Uuid>,
    /// Where a timeline's bars end; none, and each record is one day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_field: Option<Uuid>,
    /// A chart's style and measure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ChartConfig>,
    /// Words every record the view shows contains: a search kept with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
}

impl ViewConfig {
    /// The view as it reads now: anything that names a field that's
    /// archived (or gone) is left out until the field comes back.
    pub fn active(&self, definitions: &[FieldDefinition]) -> Self {
        let active = |id: &Uuid| {
            definitions
                .iter()
                .any(|field| field.id == *id && !field.archived)
        };
        Self {
            filters: self.filters.retain(&|clause| active(&clause.field)),
            sort: self
                .sort
                .iter()
                .filter(|rule| match SortBy::parse(&rule.field) {
                    Some(SortBy::Field(id)) => active(&id),
                    Some(_) => true,
                    None => false,
                })
                .cloned()
                .collect(),
            group_by: self.group_by.filter(active),
            group_bucket: self.group_bucket,
            columns: self
                .columns
                .iter()
                .filter(|column| active(&column.field))
                .cloned()
                .collect(),
            title_width: self.title_width,
            date_field: self.date_field.filter(active),
            end_field: self.end_field.filter(active),
            // A measure on a field that's gone counts until it's back.
            chart: self.chart.map(|chart| match chart.measure.field {
                Some(field) if !active(&field) => ChartConfig {
                    measure: Measure::default(),
                    ..chart
                },
                _ => chart,
            }),
            search: self.search.clone(),
        }
    }

    /// The view made ready for its type when it's created: a board groups
    /// by a field and a calendar has a date field, so one written without
    /// them takes the collection's first that fits. The message says what
    /// the collection needs first.
    pub fn for_type(
        mut self,
        view_type: ViewType,
        definitions: &[FieldDefinition],
    ) -> Result<Self, String> {
        let first = |fits: &dyn Fn(&FieldDefinition) -> bool| {
            let mut found = definitions
                .iter()
                .filter(|field| !field.archived && fits(field))
                .collect::<Vec<_>>();
            found.sort_by_key(|field| field.position);
            found.first().map(|field| field.id)
        };
        match view_type {
            ViewType::Board if self.group_by.is_none() => {
                self.group_by = Some(first(&|field| field.field_type.can_group()).ok_or(
                    "A board needs a single choice, person or checkbox field for its columns. Add one first.",
                )?);
            }
            ViewType::Calendar if self.date_field.is_none() => {
                self.date_field = Some(
                    first(&|field| field.field_type == FieldType::Date)
                        .ok_or("A calendar needs a date field. Add one first.")?,
                );
            }
            ViewType::Timeline if self.date_field.is_none() => {
                let mut dates = definitions
                    .iter()
                    .filter(|field| !field.archived && field.field_type == FieldType::Date)
                    .collect::<Vec<_>>();
                dates.sort_by_key(|field| field.position);
                let start = dates
                    .first()
                    .ok_or("A timeline needs a date field. Add one first.")?;
                self.date_field = Some(start.id);
                if self.end_field.is_none() {
                    self.end_field = dates.get(1).map(|field| field.id);
                }
            }
            // A chart's bars are its groups: the first field that can group,
            // or, with none, one number over every record.
            ViewType::Chart => {
                if self.group_by.is_none() {
                    self.group_by = first(&|field| field.field_type.can_group());
                }
                if self.chart.is_none() {
                    self.chart = Some(ChartConfig {
                        style: if self.group_by.is_some() {
                            ChartStyle::Bar
                        } else {
                            ChartStyle::Number
                        },
                        measure: Measure::default(),
                    });
                }
            }
            _ => {}
        }
        Ok(self)
    }
}

/// "me" in a person filter stands for whoever reads the view.
pub const ME: &str = "me";

/// Checks a view against the collection's fields: each filter suits its
/// field (an active one), each sort names an active field or title, number
/// or updated, the grouping is a field that can group, and each column is
/// one of the collection's fields, once. The message says what would work.
pub fn check(config: &ViewConfig, definitions: &[FieldDefinition]) -> Result<(), String> {
    let active = |id: Uuid| {
        definitions
            .iter()
            .find(|field| field.id == id && !field.archived)
    };
    if let Some(problem) = config.filters.size_problem() {
        return Err(problem);
    }
    if let Some(search) = &config.search
        && super::query::search_words(search).is_none()
    {
        return Err(format!(
            "A view searches for at most {} words in {} characters.",
            super::query::MAX_SEARCH_WORDS,
            super::query::MAX_SEARCH_CHARACTERS
        ));
    }
    for clause in config.filters.clauses() {
        let Some(field) = active(clause.field) else {
            return Err("A filter names a field this collection doesn't have.".to_owned());
        };
        if !clause.shape_is_valid() {
            return Err(format!(
                "\"{}\" {} takes {}.",
                field.name,
                clause.op.as_str(),
                match clause.op {
                    fields::FieldOp::Empty | fields::FieldOp::NotEmpty => "no values".to_owned(),
                    fields::FieldOp::Is
                    | fields::FieldOp::IsNot
                    | fields::FieldOp::CategoryIs
                    | fields::FieldOp::CategoryIsNot =>
                        format!("1 to {} values", fields::MAX_CLAUSE_VALUES),
                    _ => "one value".to_owned(),
                }
            ));
        }
        // "me" stands for the reader; any member's ID checks the same way.
        let mut checked = clause.clone();
        if field.behaves_as() == FieldType::Person {
            for value in &mut checked.values {
                if value == ME {
                    *value = Uuid::nil().to_string();
                }
            }
        }
        fields::check_clause(field, &checked)?;
    }
    if config.sort.len() > MAX_SORTS {
        return Err(format!("A view sorts by at most {MAX_SORTS} things."));
    }
    let mut sorted = HashSet::new();
    for rule in &config.sort {
        let Some(by) = SortBy::parse(&rule.field) else {
            return Err(
                "Sort by title, number, updated, or one of the collection's fields.".to_owned(),
            );
        };
        if let SortBy::Field(id) = by
            && active(id).is_none()
        {
            return Err("A sort names a field this collection doesn't have.".to_owned());
        }
        // Only records that are tasks have a place on the board, and only
        // their fields read a task's columns.
        if by == SortBy::Rank
            && !definitions.iter().any(|field| {
                field.accessor != fields::Accessor::Stored && !field.field_type.is_system()
            })
        {
            return Err(
                "Sort by title, number, updated, or one of the collection's fields.".to_owned(),
            );
        }
        if !sorted.insert(by.key()) {
            return Err("Sort by each thing once.".to_owned());
        }
    }
    if let Some(group) = config.group_by {
        match active(group) {
            Some(field) if field.can_group() => {}
            Some(field) => {
                return Err(format!(
                    "\"{}\" can't group a view. Group by a choice, status, person, checkbox or date field.",
                    field.name
                ));
            }
            None => {
                return Err("The grouping names a field this collection doesn't have.".to_owned());
            }
        }
    }
    if let Some(date) = config.date_field {
        match active(date) {
            Some(field) if field.field_type == FieldType::Date => {}
            Some(field) => {
                return Err(format!(
                    "\"{}\" isn't a date field. A calendar shows records by a date field.",
                    field.name
                ));
            }
            None => {
                return Err("The date field names a field this collection doesn't have.".to_owned());
            }
        }
    }
    if let Some(end) = config.end_field {
        match active(end) {
            Some(field) if field.field_type != FieldType::Date => {
                return Err(format!(
                    "\"{}\" isn't a date field. A timeline's bars end on a date field.",
                    field.name
                ));
            }
            Some(_) if config.date_field == Some(end) => {
                return Err(
                    "A timeline's bars start and end on two different date fields.".to_owned(),
                );
            }
            Some(_) if config.date_field.is_none() => {
                return Err("A timeline's end date goes with a start date field.".to_owned());
            }
            Some(_) => {}
            None => {
                return Err("The end date names a field this collection doesn't have.".to_owned());
            }
        }
    }
    if let Some(chart) = &config.chart {
        match (chart.measure.op, chart.measure.field) {
            (MeasureOp::Count, None) => {}
            (MeasureOp::Count, Some(_)) => {
                return Err("A count counts records; it reads no field.".to_owned());
            }
            (_, None) => {
                return Err(
                    "A sum, average, least or most reads a number field: name one.".to_owned(),
                );
            }
            (_, Some(id)) => match active(id) {
                Some(field) if field.behaves_as() == FieldType::Number => {}
                Some(field) => {
                    return Err(format!(
                        "\"{}\" isn't a number field. A chart adds up number fields.",
                        field.name
                    ));
                }
                None => {
                    return Err(
                        "The measure names a field this collection doesn't have.".to_owned()
                    );
                }
            },
        }
    }
    let mut columns = HashSet::new();
    for column in &config.columns {
        if !definitions.iter().any(|field| field.id == column.field) {
            return Err("A column names a field this collection doesn't have.".to_owned());
        }
        if !columns.insert(column.field) {
            return Err("Each field is one column.".to_owned());
        }
    }
    for width in config
        .columns
        .iter()
        .filter_map(|column| column.width)
        .chain(config.title_width)
    {
        if !(MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(&width) {
            return Err(format!(
                "Column widths are {MIN_COLUMN_WIDTH} to {MAX_COLUMN_WIDTH} pixels."
            ));
        }
    }
    Ok(())
}

/// Checks that the people and records a view's filters name are the
/// project's own: a person in it (now or before), a task or page in it,
/// or a record of the link field's collection.
/// An ID from anywhere else is refused, never kept.
pub async fn check_references(
    connection: &mut PgConnection,
    project_id: Uuid,
    config: &ViewConfig,
    definitions: &[FieldDefinition],
) -> Result<Result<(), String>, AppError> {
    for clause in config.filters.clauses() {
        let Some(field) = definitions.iter().find(|field| field.id == clause.field) else {
            continue;
        };
        let ids = clause
            .values
            .iter()
            .filter(|value| *value != ME)
            .filter_map(|value| value.parse::<Uuid>().ok())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let found = match (field.behaves_as(), field.config.target) {
            (FieldType::Person, _) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)",
                )
                .bind(project_id)
                .bind(&ids)
                .fetch_one(&mut *connection)
                .await?
            }
            (FieldType::Link, Some(LinkTarget::Page)) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM project_pages WHERE project_id = $1 AND id = ANY($2)",
                )
                .bind(project_id)
                .bind(&ids)
                .fetch_one(&mut *connection)
                .await?
            }
            (FieldType::Link, Some(LinkTarget::Record)) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM collection_records \
                     WHERE project_id = $1 AND id = ANY($2) AND collection_id = $3",
                )
                .bind(project_id)
                .bind(&ids)
                .bind(field.config.collection)
                .fetch_one(&mut *connection)
                .await?
            }
            (FieldType::Link, _) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM work_items WHERE project_id = $1 AND id = ANY($2)",
                )
                .bind(project_id)
                .bind(&ids)
                .fetch_one(&mut *connection)
                .await?
            }
            _ => continue,
        };
        let distinct = ids.iter().collect::<HashSet<_>>().len() as i64;
        if found != distinct {
            return Ok(Err(format!(
                "A filter on \"{}\" names {} that isn't in this project.",
                field.name,
                if field.behaves_as() == FieldType::Person {
                    "someone"
                } else {
                    "something"
                }
            )));
        }
    }
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// Stored views

#[derive(Clone, Debug)]
pub struct View {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub name: String,
    pub view_type: ViewType,
    pub config: ViewConfig,
    /// 0-based order among the collection's shared views, or among its
    /// owner's own.
    pub position: i32,
    pub version: i64,
    /// None for a view shared with everyone who can see the collection.
    pub owner_actor_id: Option<Uuid>,
    /// One of the views every Tasks collection starts with.
    pub builtin: Option<String>,
}

impl View {
    pub fn scope(&self) -> Scope {
        match self.owner_actor_id {
            Some(owner) => Scope::Personal(owner),
            None => Scope::Shared,
        }
    }
}

/// Whose views: the collection's shared ones, or one account's own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Shared,
    Personal(Uuid),
}

impl Scope {
    fn owner(self) -> Option<Uuid> {
        match self {
            Self::Shared => None,
            Self::Personal(owner) => Some(owner),
        }
    }

    fn limit(self) -> i64 {
        match self {
            Self::Shared => MAX_VIEWS,
            Self::Personal(_) => MAX_PERSONAL_VIEWS,
        }
    }
}

#[derive(FromRow)]
struct ViewRow {
    id: Uuid,
    collection_id: Uuid,
    name: String,
    view_type: String,
    config: Value,
    position: i32,
    version: i64,
    owner_actor_id: Option<Uuid>,
    builtin: Option<String>,
}

impl TryFrom<ViewRow> for View {
    type Error = AppError;

    fn try_from(row: ViewRow) -> Result<Self, AppError> {
        Ok(Self {
            id: row.id,
            collection_id: row.collection_id,
            name: row.name,
            view_type: parse_view_type(&row.view_type)
                .ok_or(AppError::Internal("stored view type"))?,
            config: serde_json::from_value(row.config)
                .map_err(|_| AppError::Internal("stored view config"))?,
            position: row.position,
            version: row.version,
            owner_actor_id: row.owner_actor_id,
            builtin: row.builtin,
        })
    }
}

/// A view as the web app sees it: its filters, sort and columns by field
/// ID, as stored.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSummary {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub view_type: &'static str,
    #[serde(flatten)]
    pub config: ViewConfig,
    /// 1 is first, among the shared views or among the reader's own.
    pub position: i32,
    pub version: i64,
    /// Everyone's; false for one of the reader's own.
    pub shared: bool,
}

impl ViewSummary {
    /// The view as it reads with the collection's fields as they are now.
    pub fn of(view: &View, definitions: &[FieldDefinition]) -> Self {
        Self {
            id: view.id.to_string(),
            name: view.name.clone(),
            view_type: view.view_type.as_str(),
            config: view.config.active(definitions),
            position: view.position + 1,
            version: view.version,
            shared: view.owner_actor_id.is_none(),
        }
    }
}

const VIEW_COLUMNS: &str =
    "id, collection_id, name, view_type, config, position, version, owner_actor_id, builtin";

/// A collection's shared views in order.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<Vec<View>, AppError> {
    in_scope(executor, project_id, collection_id, Scope::Shared).await
}

/// The views in one scope, in order.
pub async fn in_scope(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    scope: Scope,
) -> Result<Vec<View>, AppError> {
    sqlx::query_as::<_, ViewRow>(&format!(
        "SELECT {VIEW_COLUMNS} FROM collection_views \
         WHERE project_id = $1 AND collection_id = $2 \
           AND owner_actor_id IS NOT DISTINCT FROM $3 \
         ORDER BY position, id LIMIT {limit}",
        limit = scope.limit()
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(scope.owner())
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(View::try_from)
    .collect()
}

/// The views `reader` sees in a collection: the shared ones in order, then
/// their own.
pub async fn visible(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    reader: Uuid,
) -> Result<Vec<View>, AppError> {
    let mut views = list(&mut *connection, project_id, collection_id).await?;
    views.extend(
        in_scope(
            &mut *connection,
            project_id,
            collection_id,
            Scope::Personal(reader),
        )
        .await?,
    );
    Ok(views)
}

/// Finds one of `views` by ID or name, ignoring case.
pub fn find<'a>(views: &'a [View], reference: &str) -> Option<&'a View> {
    let reference = reference.trim();
    if let Some(id) = crate::collections::id_in(reference) {
        return views.iter().find(|view| view.id == id);
    }
    let name = reference
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    views.iter().find(|view| view.name.to_lowercase() == name)
}

/// The names of `views`, for messages.
pub fn names(views: &[View]) -> String {
    if views.is_empty() {
        return "none".to_owned();
    }
    views
        .iter()
        .map(|view| view.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Starts a new collection with its "All records" table.
pub(crate) async fn insert_first(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO collection_views (
            id, project_id, collection_id, name, view_type, config, position, created_by_actor_id
        )
        VALUES ($1, $2, $3, $4, 'table', '{}'::jsonb, 0, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(collection_id)
    .bind(crate::collections::FIRST_VIEW_NAME)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// A view to add, already checked against the collection's fields.
#[derive(Clone, Debug)]
pub struct NewView {
    pub name: String,
    pub view_type: ViewType,
    pub config: ViewConfig,
    /// 1-based place among the views; the end when None.
    pub position: Option<i32>,
    /// Shared, or one account's own.
    pub scope: Scope,
}

#[derive(Clone, Debug)]
pub enum CreateViewOutcome {
    Created(View),
    /// A view of the collection already has the name, ignoring case.
    NameTaken(View),
    /// `MAX_VIEWS` (or `MAX_PERSONAL_VIEWS`) already.
    Full,
}

/// Adds a view. The caller holds the collection's row, and has checked the
/// actor may build collections when the view is shared.
pub async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    actor_id: Uuid,
    new: NewView,
) -> Result<CreateViewOutcome, AppError> {
    if crate::collections::normalize_name(&new.name).as_deref() != Some(new.name.as_str()) {
        return Err(AppError::Validation("name"));
    }
    let existing = in_scope(&mut *connection, project_id, collection_id, new.scope).await?;
    if let Some(taken) = existing
        .iter()
        .find(|view| view.name.to_lowercase() == new.name.to_lowercase())
    {
        return Ok(CreateViewOutcome::NameTaken(taken.clone()));
    }
    if existing.len() as i64 >= new.scope.limit() {
        return Ok(CreateViewOutcome::Full);
    }
    let id = Uuid::now_v7();
    let config =
        serde_json::to_value(&new.config).map_err(|_| AppError::Internal("encode view config"))?;
    sqlx::query(
        r#"
        INSERT INTO collection_views (
            id, project_id, collection_id, name, view_type, config, position,
            created_by_actor_id, owner_actor_id
        )
        VALUES ($1, $2, $3, $4, $5, $6,
                (SELECT COALESCE(max(position) + 1, 0) FROM collection_views
                 WHERE collection_id = $3 AND owner_actor_id IS NOT DISTINCT FROM $8), $7, $8)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(collection_id)
    .bind(&new.name)
    .bind(new.view_type.as_str())
    .bind(config)
    .bind(actor_id)
    .bind(new.scope.owner())
    .execute(&mut *connection)
    .await?;
    if let Some(position) = new.position {
        move_to(
            connection,
            project_id,
            collection_id,
            new.scope,
            id,
            position,
        )
        .await?;
    }
    let view = one(connection, project_id, collection_id, id)
        .await?
        .ok_or(AppError::Internal("created view is missing"))?;
    Ok(CreateViewOutcome::Created(view))
}

/// One view by ID, in whichever scope.
pub async fn one(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    view_id: Uuid,
) -> Result<Option<View>, AppError> {
    sqlx::query_as::<_, ViewRow>(&format!(
        "SELECT {VIEW_COLUMNS} FROM collection_views \
         WHERE project_id = $1 AND collection_id = $2 AND id = $3"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(view_id)
    .fetch_optional(&mut *connection)
    .await?
    .map(View::try_from)
    .transpose()
}

/// Changes to a view, already checked. None leaves a part as it is.
#[derive(Clone, Debug, Default)]
pub struct ViewChange {
    pub name: Option<String>,
    pub config: Option<ViewConfig>,
    /// 1-based place among the collection's views.
    pub position: Option<i32>,
    /// Checked only when something would change.
    pub expected_version: Option<i64>,
    /// A personal view becomes shared, at the end of the shared views.
    pub share: bool,
}

#[derive(Clone, Debug)]
pub enum ChangeViewOutcome {
    Changed {
        view: View,
        /// Any of name, config, position.
        changed: Vec<&'static str>,
    },
    Unchanged(View),
    NotFound,
    VersionConflict(View),
    NameTaken(String),
    /// Sharing it would pass `MAX_VIEWS`.
    Full,
}

/// Renames a view, changes what it shows, moves it among the views in its
/// scope, or shares a personal view. The caller holds the collection's row
/// and has checked who may change a view in `scope`.
pub async fn change(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    scope: Scope,
    view_id: Uuid,
    change: ViewChange,
) -> Result<ChangeViewOutcome, AppError> {
    let views = in_scope(&mut *connection, project_id, collection_id, scope).await?;
    let Some(current) = views.iter().find(|view| view.id == view_id).cloned() else {
        return Ok(ChangeViewOutcome::NotFound);
    };
    let share = change.share && scope != Scope::Shared;
    let name = change.name.clone().unwrap_or_else(|| current.name.clone());
    if crate::collections::normalize_name(&name).as_deref() != Some(name.as_str()) {
        return Err(AppError::Validation("name"));
    }
    let config = change
        .config
        .clone()
        .unwrap_or_else(|| current.config.clone());
    let current_place = views
        .iter()
        .position(|view| view.id == view_id)
        .map(|index| index as i32 + 1);
    let moves = change.position.is_some_and(|position| {
        Some(position.clamp(1, views.len().max(1) as i32)) != current_place
    });
    let mut changed = Vec::new();
    if name != current.name {
        changed.push("name");
    }
    if config != current.config {
        changed.push("config");
    }
    if moves {
        changed.push("position");
    }
    if share {
        changed.push("shared");
    }
    if changed.is_empty() {
        return Ok(ChangeViewOutcome::Unchanged(current));
    }
    if change
        .expected_version
        .is_some_and(|expected| expected != current.version)
    {
        return Ok(ChangeViewOutcome::VersionConflict(current));
    }
    if name != current.name
        && let Some(other) = views
            .iter()
            .find(|view| view.id != view_id && view.name.to_lowercase() == name.to_lowercase())
    {
        return Ok(ChangeViewOutcome::NameTaken(other.name.clone()));
    }
    if share {
        let shared = list(&mut *connection, project_id, collection_id).await?;
        if let Some(other) = shared
            .iter()
            .find(|view| view.name.to_lowercase() == name.to_lowercase())
        {
            return Ok(ChangeViewOutcome::NameTaken(other.name.clone()));
        }
        if shared.len() as i64 >= MAX_VIEWS {
            return Ok(ChangeViewOutcome::Full);
        }
    }
    let config_value =
        serde_json::to_value(&config).map_err(|_| AppError::Internal("encode view config"))?;
    sqlx::query(
        r#"
        UPDATE collection_views
        SET name = $4, config = $5, version = version + 1, updated_at = now(),
            owner_actor_id = CASE WHEN $6 THEN NULL ELSE owner_actor_id END,
            position = CASE WHEN $6 THEN
                (SELECT COALESCE(max(shared.position) + 1, 0) FROM collection_views shared
                 WHERE shared.collection_id = $2 AND shared.owner_actor_id IS NULL)
                ELSE position END
        WHERE project_id = $1 AND collection_id = $2 AND id = $3
        "#,
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(view_id)
    .bind(&name)
    .bind(config_value)
    .bind(share)
    .execute(&mut *connection)
    .await?;
    let scope = if share { Scope::Shared } else { scope };
    if moves && let Some(position) = change.position {
        move_to(
            connection,
            project_id,
            collection_id,
            scope,
            view_id,
            position,
        )
        .await?;
    }
    let view = one(connection, project_id, collection_id, view_id)
        .await?
        .ok_or(AppError::Internal("changed view is missing"))?;
    Ok(ChangeViewOutcome::Changed { view, changed })
}

#[derive(Clone, Debug)]
// Made once per call and matched at once: boxing the large variant
// buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum RemoveViewOutcome {
    Removed(View),
    NotFound,
    /// A collection keeps at least one shared view.
    LastView,
}

/// Removes a view. What it showed stays: views hold no records. The caller
/// holds the collection's row and has checked who may remove a view in
/// `scope`.
pub async fn remove(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    scope: Scope,
    view_id: Uuid,
) -> Result<RemoveViewOutcome, AppError> {
    let views = in_scope(&mut *connection, project_id, collection_id, scope).await?;
    let Some(view) = views.iter().find(|view| view.id == view_id).cloned() else {
        return Ok(RemoveViewOutcome::NotFound);
    };
    if scope == Scope::Shared && views.len() <= 1 {
        return Ok(RemoveViewOutcome::LastView);
    }
    sqlx::query(
        "DELETE FROM collection_views WHERE project_id = $1 AND collection_id = $2 AND id = $3",
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(view_id)
    .execute(&mut *connection)
    .await?;
    Ok(RemoveViewOutcome::Removed(view))
}

/// Puts a view at a 1-based place among the views in its scope and
/// renumbers the rest.
async fn move_to(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    scope: Scope,
    view_id: Uuid,
    position: i32,
) -> Result<(), AppError> {
    let mut order = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_views WHERE project_id = $1 AND collection_id = $2 \
           AND owner_actor_id IS NOT DISTINCT FROM $3 \
         ORDER BY position, id",
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(scope.owner())
    .fetch_all(&mut *connection)
    .await?;
    order.retain(|id| *id != view_id);
    let index = usize::try_from(position.saturating_sub(1))
        .unwrap_or(0)
        .min(order.len());
    order.insert(index, view_id);
    let positions = (0..order.len() as i32).collect::<Vec<_>>();
    sqlx::query(
        r#"
        UPDATE collection_views view
        SET position = placed.position
        FROM unnest($3::uuid[], $4::int[]) AS placed(id, position)
        WHERE view.project_id = $1 AND view.collection_id = $2 AND view.id = placed.id
          AND view.position <> placed.position
        "#,
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(&order)
    .bind(&positions)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pins and defaults (T1.6)

/// Which views `reader` keeps in their sidebar: the ones they pinned or
/// unpinned themselves, and which built-in views start pinned for them.
#[derive(Clone, Debug, Default)]
pub struct Pins {
    /// By view: pinned or not, and the place among their pinned views.
    pub own: std::collections::HashMap<Uuid, (bool, i32)>,
    /// Their role's defaults (built-in view keys); without any, My tasks
    /// and Ready for agents start pinned.
    pub role_defaults: Option<Vec<String>>,
}

impl Pins {
    /// Whether the view is in their sidebar, and where.
    pub fn of(&self, view: &View) -> (bool, Option<i32>) {
        if let Some((pinned, position)) = self.own.get(&view.id) {
            return (*pinned, pinned.then_some(*position));
        }
        let Some(builtin) = view.builtin.as_deref() else {
            return (false, None);
        };
        let pinned = match &self.role_defaults {
            Some(keys) => keys.iter().any(|key| key == builtin),
            None => matches!(builtin, "my_tasks" | "ready_for_agents"),
        };
        (pinned, None)
    }
}

/// The views every Tasks collection starts with (`collection_views.builtin`).
pub const BUILTIN_VIEW_KEYS: [&str; 4] = [
    "my_tasks",
    "ready_for_agents",
    "blocked",
    "recently_changed",
];

/// `reader`'s pins in a collection.
pub async fn pins(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    reader: Uuid,
) -> Result<Pins, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, bool, i32)>(
        "SELECT pin.view_id, pin.pinned, pin.position FROM collection_view_pins pin \
         JOIN collection_views view ON view.project_id = pin.space_id AND view.id = pin.view_id \
         WHERE pin.space_id = $1 AND view.collection_id = $2 AND pin.actor_id = $3",
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(reader)
    .fetch_all(&mut *connection)
    .await?;
    let role_defaults =
        sqlx::query_scalar::<_, Option<Vec<String>>>("SELECT role_default_pinned_views($1, $2)")
            .bind(project_id)
            .bind(reader)
            .fetch_one(&mut *connection)
            .await?;
    Ok(Pins {
        own: rows
            .into_iter()
            .map(|(view, pinned, position)| (view, (pinned, position)))
            .collect(),
        role_defaults,
    })
}

/// Pins or unpins a view for one account. A pinned view goes to the end
/// of their pinned views unless `position` (0-based) says where.
pub async fn set_pin(
    connection: &mut PgConnection,
    project_id: Uuid,
    view_id: Uuid,
    actor_id: Uuid,
    pinned: bool,
    position: Option<i32>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO collection_view_pins (workspace_id, space_id, view_id, actor_id, pinned, position)
        SELECT project.organization_id, $1, $2, $3, $4,
               COALESCE($5, (SELECT COALESCE(max(other.position) + 1, 0)
                             FROM collection_view_pins other
                             WHERE other.space_id = $1 AND other.actor_id = $3 AND other.pinned))
        FROM projects project WHERE project.id = $1
        ON CONFLICT (view_id, actor_id) DO UPDATE
        SET pinned = EXCLUDED.pinned, position = EXCLUDED.position, updated_at = now()
        "#,
    )
    .bind(project_id)
    .bind(view_id)
    .bind(actor_id)
    .bind(pinned)
    .bind(position.map(|position| position.max(0)))
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Whose default a view is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefaultFor {
    /// One account's own; it wins over their role's.
    Account(Uuid),
    /// Everyone holding a role in the space.
    Role(Uuid),
}

/// The view a collection opens on for `reader`: their own default, else
/// their role's in the space, else the first shared view. None only when
/// the collection has no shared view.
pub async fn default_view(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    reader: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let chosen = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT preference.view_id FROM collection_view_defaults preference
        JOIN collection_views view
          ON view.project_id = preference.space_id AND view.id = preference.view_id
        WHERE preference.space_id = $1 AND preference.collection_id = $2
          AND (view.owner_actor_id IS NULL OR view.owner_actor_id = $3)
          AND (preference.actor_id = $3
               OR preference.role_id IN (
                   SELECT held.id FROM project_memberships membership
                   JOIN projects project ON project.id = membership.project_id
                   JOIN workspace_roles held ON held.organization_id = project.organization_id
                    AND (held.id = membership.role_id
                         OR (membership.role_id IS NULL AND held.key = membership.role))
                   WHERE membership.project_id = $1 AND membership.actor_id = $3))
        ORDER BY (preference.actor_id IS NULL), preference.updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(reader)
    .fetch_optional(&mut *connection)
    .await?;
    if chosen.is_some() {
        return Ok(chosen);
    }
    Ok(list(&mut *connection, project_id, collection_id)
        .await?
        .first()
        .map(|view| view.id))
}

/// Sets (or, with no view, clears) a default. The caller has checked the
/// view is one the default's holder can see.
pub async fn set_default(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    holder: DefaultFor,
    view_id: Option<Uuid>,
    set_by: Uuid,
) -> Result<(), AppError> {
    let (role_id, actor_id) = match holder {
        DefaultFor::Account(actor) => (None, Some(actor)),
        DefaultFor::Role(role) => (Some(role), None),
    };
    sqlx::query(
        "DELETE FROM collection_view_defaults \
         WHERE space_id = $1 AND collection_id = $2 \
           AND role_id IS NOT DISTINCT FROM $3 AND actor_id IS NOT DISTINCT FROM $4",
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(role_id)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if let Some(view_id) = view_id {
        sqlx::query(
            r#"
            INSERT INTO collection_view_defaults
                (id, workspace_id, space_id, collection_id, role_id, actor_id, view_id, set_by_actor_id)
            SELECT $1, project.organization_id, $2, $3, $4, $5, $6, $7
            FROM projects project WHERE project.id = $2
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(collection_id)
        .bind(role_id)
        .bind(actor_id)
        .bind(view_id)
        .bind(set_by)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{Column, SortBy, SortRule, ViewConfig, ViewType, check, parse_view_type};
    use crate::fields::{FieldClause, FieldConfig, FieldDefinition, FieldOp, FieldType};

    fn field(name: &str, field_type: FieldType, archived: bool) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived,
            version: 1,
            accessor: crate::fields::Accessor::Stored,
        }
    }

    #[test]
    fn views_name_only_the_collections_own_active_fields() {
        let person = field("Tester", FieldType::Person, false);
        let text = field("Notes", FieldType::Text, false);
        let gone = field("Old", FieldType::Text, true);
        let fields = vec![person.clone(), text.clone(), gone.clone()];
        let mine = ViewConfig {
            filters: vec![FieldClause {
                field: person.id,
                op: FieldOp::Is,
                values: vec!["me".to_owned()],
            }]
            .into(),
            sort: vec![SortRule {
                field: "updated".to_owned(),
                direction: super::Direction::Desc,
            }],
            group_by: Some(person.id),
            group_bucket: None,
            columns: vec![Column {
                field: text.id,
                hidden: true,
                width: Some(200),
            }],
            title_width: None,
            date_field: None,
            end_field: None,
            chart: None,
            search: Some("login crash".to_owned()),
        };
        assert_eq!(check(&mine, &fields), Ok(()));
        // A kept search is read like a typed one: every word, a few at most.
        let spec = crate::views::query::Spec::from_config(&mine, &fields, Uuid::now_v7())
            .searching(vec!["Crash".to_owned(), "parser".to_owned()], &fields);
        assert_eq!(spec.words, ["login", "crash", "parser"]);
        let wordy = ViewConfig {
            search: Some("one two three four five six seven eight nine".to_owned()),
            ..ViewConfig::default()
        };
        assert!(check(&wordy, &fields).is_err());
        let elsewhere = ViewConfig {
            filters: vec![FieldClause {
                field: Uuid::now_v7(),
                op: FieldOp::NotEmpty,
                values: Vec::new(),
            }]
            .into(),
            ..ViewConfig::default()
        };
        assert!(check(&elsewhere, &fields).is_err());
        let archived = ViewConfig {
            sort: vec![SortRule {
                field: gone.id.to_string(),
                direction: super::Direction::Asc,
            }],
            ..ViewConfig::default()
        };
        assert!(check(&archived, &fields).is_err());
        let text_group = ViewConfig {
            group_by: Some(text.id),
            ..ViewConfig::default()
        };
        assert!(check(&text_group, &fields).is_err());
        let narrow = ViewConfig {
            title_width: Some(2),
            ..ViewConfig::default()
        };
        assert!(check(&narrow, &fields).is_err());
        // A field archived after the view was saved drops out of reads.
        let stored = ViewConfig {
            group_by: Some(gone.id),
            columns: vec![Column {
                field: gone.id,
                hidden: false,
                width: None,
            }],
            ..mine.clone()
        };
        let read = stored.active(&fields);
        assert_eq!(read.group_by, None);
        assert!(read.columns.is_empty());
        assert_eq!(read.filters, mine.filters);
    }

    #[test]
    fn sorts_read_the_way_people_write_them() {
        assert_eq!(SortBy::parse("Title"), Some(SortBy::Title));
        assert_eq!(SortBy::parse("created"), Some(SortBy::Number));
        assert_eq!(SortBy::parse("edited"), Some(SortBy::Updated));
        let id = Uuid::now_v7();
        assert_eq!(SortBy::parse(&id.to_string()), Some(SortBy::Field(id)));
        assert_eq!(SortBy::parse("size"), None);
    }

    #[test]
    fn timelines_run_from_one_date_field_to_another() {
        let start = field("Start", FieldType::Date, false);
        let mut end = field("End", FieldType::Date, false);
        end.position = start.position + 1;
        let notes = field("Notes", FieldType::Text, false);
        let fields = vec![start.clone(), end.clone(), notes.clone()];
        let timeline = ViewConfig::default()
            .for_type(ViewType::Timeline, &fields)
            .expect("a timeline");
        assert_eq!(timeline.date_field, Some(start.id));
        assert_eq!(timeline.end_field, Some(end.id));
        assert!(check(&timeline, &fields).is_ok());
        let same = ViewConfig {
            date_field: Some(start.id),
            end_field: Some(start.id),
            ..ViewConfig::default()
        };
        assert!(check(&same, &fields).is_err());
        let not_a_date = ViewConfig {
            date_field: Some(start.id),
            end_field: Some(notes.id),
            ..ViewConfig::default()
        };
        assert!(check(&not_a_date, &fields).is_err());
        let one_date = ViewConfig::default()
            .for_type(ViewType::Timeline, std::slice::from_ref(&start))
            .expect("one date is enough");
        assert_eq!(one_date.end_field, None);
        assert!(
            ViewConfig::default()
                .for_type(ViewType::Timeline, std::slice::from_ref(&notes))
                .is_err()
        );
        assert!(parse_view_type("list").is_some() && parse_view_type("timeline").is_some());
    }

    #[test]
    fn boards_and_calendars_start_on_a_field_that_fits() {
        let notes = field("Notes", FieldType::Text, false);
        let old_date = field("Was due", FieldType::Date, true);
        let due = field("Due", FieldType::Date, false);
        let status = field("Status", FieldType::SingleChoice, false);
        let fields = vec![notes.clone(), old_date.clone(), due.clone(), status.clone()];
        let board = ViewConfig::default()
            .for_type(ViewType::Board, &fields)
            .expect("a board");
        assert_eq!(board.group_by, Some(status.id));
        let calendar = ViewConfig::default()
            .for_type(ViewType::Calendar, &fields)
            .expect("a calendar");
        assert_eq!(calendar.date_field, Some(due.id));
        assert_eq!(check(&calendar, &fields), Ok(()));
        // What the writer chose stays.
        let chosen = ViewConfig {
            group_by: Some(status.id),
            ..ViewConfig::default()
        };
        assert_eq!(
            chosen.clone().for_type(ViewType::Board, &fields),
            Ok(chosen)
        );
        // Nothing that fits: the message says what to add.
        assert!(
            ViewConfig::default()
                .for_type(ViewType::Board, std::slice::from_ref(&notes))
                .is_err()
        );
        assert!(
            ViewConfig::default()
                .for_type(ViewType::Calendar, &[notes.clone(), old_date.clone()])
                .is_err()
        );
        // A calendar's dates come from a date field, an active one.
        for wrong in [notes.id, old_date.id, Uuid::now_v7()] {
            let config = ViewConfig {
                date_field: Some(wrong),
                ..ViewConfig::default()
            };
            assert!(check(&config, &fields).is_err());
        }
        // Archived since, it drops out of reads until it's back.
        let stored = ViewConfig {
            date_field: Some(old_date.id),
            ..ViewConfig::default()
        };
        assert_eq!(stored.active(&fields).date_field, None);
        assert_eq!(parse_view_type(" Calendar "), Some(ViewType::Calendar));
        assert_eq!(parse_view_type("chart"), Some(ViewType::Chart));
        assert_eq!(parse_view_type("kanban"), None);
    }
}
