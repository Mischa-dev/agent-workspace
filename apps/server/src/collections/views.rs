//! A collection's views (AWD-90): a name, a type (tables first; boards,
//! galleries and calendars come later) and what it shows, kept in
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

use crate::{
    error::AppError,
    fields::{self, FieldClause, FieldDefinition, FieldType, LinkTarget},
};

/// Views one collection keeps.
pub const MAX_VIEWS: i64 = 30;
/// Sort keys one view or query takes.
pub const MAX_SORTS: usize = 3;
pub const MIN_COLUMN_WIDTH: u16 = 48;
pub const MAX_COLUMN_WIDTH: u16 = 1_200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewType {
    Table,
}

impl ViewType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "table" => Some(Self::Table),
            _ => None,
        }
    }
}

/// What a view or a query asks for when the type isn't one there is yet.
pub const VIEW_TYPE_MESSAGE: &str =
    "Views are tables for now (\"table\"); boards, galleries and calendars come later.";

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
            _ => None,
        }
    }

    pub fn key(self) -> String {
        match self {
            Self::Title => "title".to_owned(),
            Self::Number => "number".to_owned(),
            Self::Updated => "updated".to_owned(),
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FieldClause>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    /// The title column's width, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_width: Option<u16>,
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
            filters: self
                .filters
                .iter()
                .filter(|clause| active(&clause.field))
                .cloned()
                .collect(),
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
            columns: self
                .columns
                .iter()
                .filter(|column| active(&column.field))
                .cloned()
                .collect(),
            title_width: self.title_width,
        }
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
    if config.filters.len() > fields::MAX_CLAUSES {
        return Err(format!(
            "A view has at most {} filters.",
            fields::MAX_CLAUSES
        ));
    }
    for clause in &config.filters {
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
                    fields::FieldOp::Is | fields::FieldOp::IsNot =>
                        format!("1 to {} values", fields::MAX_CLAUSE_VALUES),
                    _ => "one value".to_owned(),
                }
            ));
        }
        // "me" stands for the reader; any member's ID checks the same way.
        let mut checked = clause.clone();
        if field.field_type == FieldType::Person {
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
        if !sorted.insert(by.key()) {
            return Err("Sort by each thing once.".to_owned());
        }
    }
    if let Some(group) = config.group_by {
        match active(group) {
            Some(field) if field.field_type.can_group() => {}
            Some(field) => {
                return Err(format!(
                    "\"{}\" can't group a view. Group by a single choice, person or checkbox field.",
                    field.name
                ));
            }
            None => {
                return Err("The grouping names a field this collection doesn't have.".to_owned());
            }
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
/// project's own: a person in it (now or before), a task or page in it.
/// An ID from anywhere else is refused, never kept.
pub async fn check_references(
    connection: &mut PgConnection,
    project_id: Uuid,
    config: &ViewConfig,
    definitions: &[FieldDefinition],
) -> Result<Result<(), String>, AppError> {
    for clause in &config.filters {
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
        let found = match (field.field_type, field.config.target) {
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
                if field.field_type == FieldType::Person {
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
    /// 0-based order among the collection's views.
    pub position: i32,
    pub version: i64,
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
}

impl TryFrom<ViewRow> for View {
    type Error = AppError;

    fn try_from(row: ViewRow) -> Result<Self, AppError> {
        Ok(Self {
            id: row.id,
            collection_id: row.collection_id,
            name: row.name,
            view_type: ViewType::parse(&row.view_type)
                .ok_or(AppError::Internal("stored view type"))?,
            config: serde_json::from_value(row.config)
                .map_err(|_| AppError::Internal("stored view config"))?,
            position: row.position,
            version: row.version,
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
    /// 1 is first.
    pub position: i32,
    pub version: i64,
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
        }
    }
}

const VIEW_COLUMNS: &str = "id, collection_id, name, view_type, config, position, version";

/// A collection's views in order.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<Vec<View>, AppError> {
    sqlx::query_as::<_, ViewRow>(&format!(
        "SELECT {VIEW_COLUMNS} FROM collection_views \
         WHERE project_id = $1 AND collection_id = $2 ORDER BY position, id LIMIT {MAX_VIEWS}"
    ))
    .bind(project_id)
    .bind(collection_id)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(View::try_from)
    .collect()
}

/// Finds one of `views` by ID or name, ignoring case.
pub fn find<'a>(views: &'a [View], reference: &str) -> Option<&'a View> {
    let reference = reference.trim();
    if let Some(id) = super::id_in(reference) {
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
pub(super) async fn insert_first(
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
    .bind(super::FIRST_VIEW_NAME)
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
}

#[derive(Clone, Debug)]
pub enum CreateViewOutcome {
    Created(View),
    /// A view of the collection already has the name, ignoring case.
    NameTaken(View),
    /// `MAX_VIEWS` already.
    Full,
}

/// Adds a view. The caller holds the project lock and the collection's
/// row, and has checked the actor may build collections.
pub async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    actor_id: Uuid,
    new: NewView,
) -> Result<CreateViewOutcome, AppError> {
    if super::normalize_name(&new.name).as_deref() != Some(new.name.as_str()) {
        return Err(AppError::Validation("name"));
    }
    let existing = list(&mut *connection, project_id, collection_id).await?;
    if let Some(taken) = existing
        .iter()
        .find(|view| view.name.to_lowercase() == new.name.to_lowercase())
    {
        return Ok(CreateViewOutcome::NameTaken(taken.clone()));
    }
    if existing.len() as i64 >= MAX_VIEWS {
        return Ok(CreateViewOutcome::Full);
    }
    let id = Uuid::now_v7();
    let config =
        serde_json::to_value(&new.config).map_err(|_| AppError::Internal("encode view config"))?;
    sqlx::query(
        r#"
        INSERT INTO collection_views (
            id, project_id, collection_id, name, view_type, config, position, created_by_actor_id
        )
        VALUES ($1, $2, $3, $4, $5, $6,
                (SELECT COALESCE(max(position) + 1, 0) FROM collection_views
                 WHERE collection_id = $3), $7)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(collection_id)
    .bind(&new.name)
    .bind(new.view_type.as_str())
    .bind(config)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if let Some(position) = new.position {
        move_to(connection, project_id, collection_id, id, position).await?;
    }
    let view = one(connection, project_id, collection_id, id)
        .await?
        .ok_or(AppError::Internal("created view is missing"))?;
    Ok(CreateViewOutcome::Created(view))
}

async fn one(
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
}

/// Renames a view, changes what it shows, or moves it. The caller holds
/// the project lock and the collection's row.
pub async fn change(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    view_id: Uuid,
    change: ViewChange,
) -> Result<ChangeViewOutcome, AppError> {
    let views = list(&mut *connection, project_id, collection_id).await?;
    let Some(current) = views.iter().find(|view| view.id == view_id).cloned() else {
        return Ok(ChangeViewOutcome::NotFound);
    };
    let name = change.name.clone().unwrap_or_else(|| current.name.clone());
    if super::normalize_name(&name).as_deref() != Some(name.as_str()) {
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
    let config_value =
        serde_json::to_value(&config).map_err(|_| AppError::Internal("encode view config"))?;
    sqlx::query(
        r#"
        UPDATE collection_views
        SET name = $4, config = $5, version = version + 1, updated_at = now()
        WHERE project_id = $1 AND collection_id = $2 AND id = $3
        "#,
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(view_id)
    .bind(&name)
    .bind(config_value)
    .execute(&mut *connection)
    .await?;
    if moves && let Some(position) = change.position {
        move_to(connection, project_id, collection_id, view_id, position).await?;
    }
    let view = one(connection, project_id, collection_id, view_id)
        .await?
        .ok_or(AppError::Internal("changed view is missing"))?;
    Ok(ChangeViewOutcome::Changed { view, changed })
}

#[derive(Clone, Debug)]
pub enum RemoveViewOutcome {
    Removed(View),
    NotFound,
    /// A collection keeps at least one view.
    LastView,
}

/// Removes a view. What it showed stays: views hold no records. The caller
/// holds the project lock and the collection's row.
pub async fn remove(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    view_id: Uuid,
) -> Result<RemoveViewOutcome, AppError> {
    let views = list(&mut *connection, project_id, collection_id).await?;
    let Some(view) = views.iter().find(|view| view.id == view_id).cloned() else {
        return Ok(RemoveViewOutcome::NotFound);
    };
    if views.len() <= 1 {
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

/// Puts a view at a 1-based place among the collection's views and
/// renumbers the rest.
async fn move_to(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    view_id: Uuid,
    position: i32,
) -> Result<(), AppError> {
    let mut order = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_views WHERE project_id = $1 AND collection_id = $2 \
         ORDER BY position, id",
    )
    .bind(project_id)
    .bind(collection_id)
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

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{Column, SortBy, SortRule, ViewConfig, check};
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
            }],
            sort: vec![SortRule {
                field: "updated".to_owned(),
                direction: super::Direction::Desc,
            }],
            group_by: Some(person.id),
            columns: vec![Column {
                field: text.id,
                hidden: true,
                width: Some(200),
            }],
            title_width: None,
        };
        assert_eq!(check(&mine, &fields), Ok(()));
        let elsewhere = ViewConfig {
            filters: vec![FieldClause {
                field: Uuid::now_v7(),
                op: FieldOp::NotEmpty,
                values: Vec::new(),
            }],
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
}
