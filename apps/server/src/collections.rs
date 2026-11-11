//! Collections (AWD-90): the record types a project makes for itself, such
//! as Playtest feedback, Builds or Contacts, each with its own fields,
//! records and views (docs/plans/collections.md; migration 20261111081500).
//!
//! * A collection's fields are `crate::fields` definitions with the record
//!   kind `RecordKind::Collection(id)`, so tasks and collections share one
//!   set of field types, checks, filters and sorts.
//! * [`records`]: a collection's records, their values and their history.
//! * [`views`]: the ways to look at a collection (filters, sort, grouping
//!   and columns), checked against its fields on every write.
//! * [`query`]: finding records on the server, a page at a time, with
//!   counts by group. Nothing reads a whole collection.
//! * [`definition`]: a collection's fields and views by name, so one made
//!   in one project can be made again in another.
//! * [`http`]: the web app's routes. Agents use the five tools in
//!   `mcp::collection_tools`.
//!
//! Everyone in a project reads its collections. `collections.edit` adds,
//! changes, archives and restores records; `collections.build` creates
//! collections and changes their fields and views (AWD-74 roles). Archiving
//! hides a collection or a record; nothing is deleted. Every write takes the
//! project row lock first (`work_item_commands::lock_membership`), then the
//! collection's row, so record numbers, limits and names are exact.

pub mod definition;
pub mod http;
pub mod query;
pub mod records;
pub mod views;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    error::AppError,
    fields::{self, FieldDefinition, RecordKind},
};

/// Active collections one project can have.
pub const MAX_ACTIVE_COLLECTIONS: i64 = 50;
/// Every collection a project keeps, archived ones too.
pub const MAX_COLLECTION_ROWS: i64 = 200;
pub const MAX_NAME_CHARACTERS: usize = 80;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 500;
pub const MAX_TITLE_NAME_CHARACTERS: usize = 60;
/// Records one collection keeps, archived ones too.
pub const MAX_RECORDS: i64 = 100_000;
/// The view every new collection starts with.
pub const FIRST_VIEW_NAME: &str = "All records";
pub const DEFAULT_TITLE_NAME: &str = "Name";

/// The kind of record a collection's fields belong to.
pub fn kind(collection_id: Uuid) -> RecordKind {
    RecordKind::Collection(collection_id)
}

/// A collection's fields: active ones in order, then archived ones when
/// asked for.
pub async fn field_definitions(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    fields::list(executor, project_id, kind(collection_id), include_archived).await
}

// ---------------------------------------------------------------------------
// Names

/// An ID, or a link whose last part is one (such as
/// `/projects/…/collections/<id>?view=…`).
pub fn id_in(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value.len() > 2048 {
        return None;
    }
    value.parse::<Uuid>().ok().or_else(|| {
        value
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|segment| segment.split(['?', '#']).next())
            .and_then(|segment| segment.parse::<Uuid>().ok())
    })
}

fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line of 1 to `max` characters, runs of whitespace collapsed, and
/// never shaped like an ID, so "ID or name" is never ambiguous.
fn one_line(value: &str, max: usize) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = collapse(value);
    let length = value.chars().count();
    (length > 0 && length <= max && value.parse::<Uuid>().is_err()).then_some(value)
}

/// A collection's (or a view's) name.
pub fn normalize_name(value: &str) -> Option<String> {
    one_line(value, MAX_NAME_CHARACTERS)
}

/// What the title column is called, such as Name, Version or Tester.
pub fn normalize_title_name(value: &str) -> Option<String> {
    one_line(value, MAX_TITLE_NAME_CHARACTERS)
}

/// What a collection is for, on one line. Empty clears it.
pub fn normalize_description(value: &str) -> Option<String> {
    let value = value.trim();
    (!value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
        && value.chars().count() <= MAX_DESCRIPTION_CHARACTERS)
        .then(|| value.to_owned())
}

// ---------------------------------------------------------------------------
// Collections

#[derive(Clone, Debug, FromRow)]
pub struct Collection {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub title_name: String,
    /// The number the next record gets.
    pub next_number: i64,
    /// 0-based order among the project's collections.
    pub position: i32,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "collection.id, collection.project_id, collection.name, \
     collection.description, collection.title_name, collection.next_number, \
     collection.position, collection.archived_at IS NOT NULL AS archived, collection.version, \
     collection.created_at::text AS created_at, collection.updated_at::text AS updated_at";

/// A collection as the web app and agents see it.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSummary {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What the title column is called, such as Name or Version.
    pub title_name: String,
    /// Records that aren't archived.
    pub records: i64,
    /// 1 is first.
    pub position: i32,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl CollectionSummary {
    pub fn of(collection: &Collection, records: i64) -> Self {
        Self {
            id: collection.id.to_string(),
            name: collection.name.clone(),
            description: collection.description.clone(),
            title_name: collection.title_name.clone(),
            records,
            position: collection.position + 1,
            archived: collection.archived,
            version: collection.version,
            created_at: collection.created_at.clone(),
            updated_at: collection.updated_at.clone(),
        }
    }
}

#[derive(FromRow)]
struct CountedRow {
    #[sqlx(flatten)]
    collection: Collection,
    records: i64,
}

/// A project's collections: active ones in order, then archived ones when
/// asked for, each with how many records it has.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<CollectionSummary>, AppError> {
    Ok(sqlx::query_as::<_, CountedRow>(&format!(
        "SELECT {COLUMNS}, \
           (SELECT count(*) FROM collection_records record \
            WHERE record.collection_id = collection.id AND record.archived_at IS NULL) AS records \
         FROM collections collection \
         WHERE collection.project_id = $1 AND ($2 OR collection.archived_at IS NULL) \
         ORDER BY collection.archived_at IS NOT NULL, collection.position, collection.id \
         LIMIT {MAX_COLLECTION_ROWS}"
    ))
    .bind(project_id)
    .bind(include_archived)
    .fetch_all(executor)
    .await?
    .iter()
    .map(|row| CollectionSummary::of(&row.collection, row.records))
    .collect())
}

/// Records in a collection that aren't archived.
pub async fn record_count(
    executor: impl sqlx::PgExecutor<'_>,
    collection_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records WHERE collection_id = $1 AND archived_at IS NULL",
    )
    .bind(collection_id)
    .fetch_one(executor)
    .await?)
}

/// One of the project's collections by ID.
pub async fn get(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<Option<Collection>, AppError> {
    Ok(sqlx::query_as::<_, Collection>(&format!(
        "SELECT {COLUMNS} FROM collections collection \
         WHERE collection.project_id = $1 AND collection.id = $2"
    ))
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(executor)
    .await?)
}

/// One of the project's collections by ID or name (ignoring case). A name
/// finds the active collection first, then the archived one made last.
pub async fn find(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    reference: &str,
) -> Result<Option<Collection>, AppError> {
    let reference = reference.trim();
    let id = id_in(reference);
    Ok(sqlx::query_as::<_, Collection>(&format!(
        "SELECT {COLUMNS} FROM collections collection \
         WHERE collection.project_id = $1 \
           AND (collection.id = $2 OR ($2 IS NULL AND lower(collection.name) = lower($3))) \
         ORDER BY collection.archived_at IS NOT NULL, collection.archived_at DESC, collection.id \
         LIMIT 1"
    ))
    .bind(project_id)
    .bind(id)
    .bind(collapse(reference))
    .fetch_optional(executor)
    .await?)
}

/// Locks a collection's row for a write. The caller holds the project lock.
pub async fn lock(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<Option<Collection>, AppError> {
    Ok(sqlx::query_as::<_, Collection>(&format!(
        "SELECT {COLUMNS} FROM collections collection \
         WHERE collection.project_id = $1 AND collection.id = $2 FOR UPDATE"
    ))
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The names of the project's active collections, for messages.
pub async fn active_names(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<String, AppError> {
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM collections WHERE project_id = $1 AND archived_at IS NULL \
         ORDER BY position, id LIMIT 50",
    )
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    Ok(if names.is_empty() {
        "none yet".to_owned()
    } else {
        names.join(", ")
    })
}

/// A collection to create, its parts already normalized.
#[derive(Clone, Debug)]
pub struct NewCollection {
    pub name: String,
    pub description: String,
    pub title_name: String,
    /// Whether to start it with the "All records" table view. A definition
    /// that brings views of its own leaves it out.
    pub first_view: bool,
}

#[derive(Clone, Debug)]
pub enum CreateOutcome {
    Created(Collection),
    /// An active collection already has the name, ignoring case.
    NameTaken(Collection),
    /// `MAX_ACTIVE_COLLECTIONS` active ones, or `MAX_COLLECTION_ROWS` in all.
    ProjectFull,
}

/// Creates a collection. The caller holds the project lock and has checked
/// the actor may build collections.
pub async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    new: NewCollection,
) -> Result<CreateOutcome, AppError> {
    if normalize_name(&new.name).as_deref() != Some(new.name.as_str())
        || normalize_title_name(&new.title_name).as_deref() != Some(new.title_name.as_str())
        || normalize_description(&new.description).as_deref() != Some(new.description.as_str())
    {
        return Err(AppError::Validation("collection"));
    }
    let taken = sqlx::query_as::<_, Collection>(&format!(
        "SELECT {COLUMNS} FROM collections collection \
         WHERE collection.project_id = $1 AND collection.archived_at IS NULL \
           AND lower(collection.name) = lower($2)"
    ))
    .bind(project_id)
    .bind(&new.name)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(taken) = taken {
        return Ok(CreateOutcome::NameTaken(taken));
    }
    let (active, total) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE archived_at IS NULL), count(*) \
         FROM collections WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *connection)
    .await?;
    if active >= MAX_ACTIVE_COLLECTIONS || total >= MAX_COLLECTION_ROWS {
        return Ok(CreateOutcome::ProjectFull);
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO collections (
            id, project_id, name, description, title_name, position, created_by_actor_id
        )
        VALUES ($1, $2, $3, $4, $5,
                (SELECT COALESCE(max(position) + 1, 0) FROM collections WHERE project_id = $2),
                $6)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(&new.name)
    .bind(&new.description)
    .bind(&new.title_name)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if new.first_view {
        views::insert_first(connection, project_id, id, actor_id).await?;
    }
    let created = get(&mut *connection, project_id, id)
        .await?
        .ok_or(AppError::Internal("created collection is missing"))?;
    Ok(CreateOutcome::Created(created))
}

/// Changes to a collection, already normalized. None leaves a part as it is.
#[derive(Clone, Debug, Default)]
pub struct CollectionChange {
    pub name: Option<String>,
    pub description: Option<String>,
    pub title_name: Option<String>,
    /// 1-based place among the project's active collections.
    pub position: Option<i32>,
    pub archived: Option<bool>,
    /// Checked only when something would change.
    pub expected_version: Option<i64>,
}

impl CollectionChange {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && self.title_name.is_none()
            && self.position.is_none()
            && self.archived.is_none()
    }
}

#[derive(Clone, Debug)]
pub enum ChangeOutcome {
    Changed {
        collection: Collection,
        previous: Collection,
        /// Any of name, description, titleName, position, archived.
        changed: Vec<&'static str>,
    },
    Unchanged(Collection),
    NotFound,
    VersionConflict(Collection),
    /// Another active collection has the name.
    NameTaken(String),
    /// Restoring would pass `MAX_ACTIVE_COLLECTIONS`.
    ProjectFull,
}

/// Renames, describes, reorders, archives or restores a collection. The
/// caller holds the project lock and has checked the actor may build
/// collections.
pub async fn change(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    actor_id: Uuid,
    change: CollectionChange,
) -> Result<ChangeOutcome, AppError> {
    let Some(current) = lock(connection, project_id, collection_id).await? else {
        return Ok(ChangeOutcome::NotFound);
    };
    let name = change.name.clone().unwrap_or_else(|| current.name.clone());
    let description = change
        .description
        .clone()
        .unwrap_or_else(|| current.description.clone());
    let title_name = change
        .title_name
        .clone()
        .unwrap_or_else(|| current.title_name.clone());
    let archived = change.archived.unwrap_or(current.archived);
    if normalize_name(&name).as_deref() != Some(name.as_str())
        || normalize_description(&description).as_deref() != Some(description.as_str())
        || normalize_title_name(&title_name).as_deref() != Some(title_name.as_str())
    {
        return Err(AppError::Validation("collection"));
    }
    let active = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM collections WHERE project_id = $1 AND archived_at IS NULL \
         ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let current_place = active
        .iter()
        .position(|(id, _)| *id == collection_id)
        .map(|index| index as i32 + 1);
    let moves = change
        .position
        .filter(|_| !archived)
        .is_some_and(|position| {
            Some(position.clamp(1, active.len().max(1) as i32)) != current_place
        });
    let mut changed = Vec::new();
    if name != current.name {
        changed.push("name");
    }
    if description != current.description {
        changed.push("description");
    }
    if title_name != current.title_name {
        changed.push("titleName");
    }
    if moves {
        changed.push("position");
    }
    if archived != current.archived {
        changed.push("archived");
    }
    if changed.is_empty() {
        return Ok(ChangeOutcome::Unchanged(current));
    }
    if change
        .expected_version
        .is_some_and(|expected| expected != current.version)
    {
        return Ok(ChangeOutcome::VersionConflict(current));
    }
    let restoring = current.archived && !archived;
    if !archived
        && (restoring || name != current.name)
        && let Some((_, other)) = active
            .iter()
            .find(|(id, other)| *id != collection_id && other.to_lowercase() == name.to_lowercase())
    {
        return Ok(ChangeOutcome::NameTaken(other.clone()));
    }
    if restoring && active.len() as i64 >= MAX_ACTIVE_COLLECTIONS {
        return Ok(ChangeOutcome::ProjectFull);
    }
    sqlx::query(
        r#"
        UPDATE collections
        SET name = $3, description = $4, title_name = $5,
            archived_at = CASE WHEN $6 THEN COALESCE(archived_at, now()) END,
            archived_by_actor_id = CASE WHEN $6 THEN COALESCE(archived_by_actor_id, $7) END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(&name)
    .bind(&description)
    .bind(&title_name)
    .bind(archived)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if moves && let Some(position) = change.position {
        move_to(connection, project_id, collection_id, position).await?;
    } else if restoring {
        move_to(connection, project_id, collection_id, i32::MAX).await?;
    }
    let collection = get(&mut *connection, project_id, collection_id)
        .await?
        .ok_or(AppError::Internal("changed collection is missing"))?;
    Ok(ChangeOutcome::Changed {
        collection,
        previous: current,
        changed,
    })
}

/// Bumps a collection's version when its fields or views change, so a
/// screen that holds it knows to read it again.
pub async fn touch(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE collections SET version = version + 1, updated_at = now() \
         WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(collection_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Puts a collection at a 1-based place among the active ones (archived
/// ones keep their places after them) and renumbers the rest.
async fn move_to(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    position: i32,
) -> Result<(), AppError> {
    let mut order = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT id, archived_at IS NOT NULL FROM collections WHERE project_id = $1 \
         ORDER BY archived_at IS NOT NULL, position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let active = order
        .iter()
        .filter(|(id, archived)| !archived && *id != collection_id)
        .count();
    order.retain(|(id, _)| *id != collection_id);
    let index = usize::try_from(position.saturating_sub(1))
        .unwrap_or(0)
        .min(active);
    order.insert(index, (collection_id, false));
    let ids = order.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let positions = (0..ids.len() as i32).collect::<Vec<_>>();
    sqlx::query(
        r#"
        UPDATE collections collection
        SET position = placed.position
        FROM unnest($2::uuid[], $3::int[]) AS placed(id, position)
        WHERE collection.project_id = $1 AND collection.id = placed.id
          AND collection.position <> placed.position
        "#,
    )
    .bind(project_id)
    .bind(&ids)
    .bind(&positions)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{normalize_description, normalize_name, normalize_title_name};

    #[test]
    fn names_are_one_line_and_never_an_id() {
        assert_eq!(
            normalize_name("  Playtest   feedback ").as_deref(),
            Some("Playtest feedback")
        );
        assert!(normalize_name("").is_none());
        assert!(normalize_name(&"x".repeat(81)).is_none());
        assert!(normalize_name(&Uuid::now_v7().to_string()).is_none());
        assert!(normalize_name("two\nlines").is_none());
        assert!(normalize_title_name(&"x".repeat(61)).is_none());
        assert_eq!(normalize_title_name("Tester").as_deref(), Some("Tester"));
        assert_eq!(normalize_description("  ").as_deref(), Some(""));
        assert!(normalize_description("two\nlines").is_none());
        assert!(normalize_description(&"x".repeat(501)).is_none());
    }
}
