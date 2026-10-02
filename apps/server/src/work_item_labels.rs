//! Labels on tasks (migration 0044).
//!
//! A label belongs to one project. Names are unique among a project's active
//! labels, ignoring case; archiving a label hides it from every task until
//! it's restored, and nothing is ever deleted. MCP, `task_move`, the batch
//! tools and the web app share these functions, so the rules live in one
//! place.
//!
//! Every function that writes needs the caller to hold the project row lock
//! from `work_item_commands::lock_membership`. That is the order every task
//! write takes (without it, the foreign keys' share locks on `projects`
//! deadlock against that lock), and it makes the count limits and name
//! checks below exact. Names are compared with SQL `lower()` on both sides,
//! never by lowercasing in Rust, so the database and the code agree.

use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

pub const MAX_ACTIVE_LABELS_PER_PROJECT: i64 = 200;
/// Every label row a project keeps, archived ones included.
pub const MAX_LABEL_ROWS_PER_PROJECT: i64 = 1_000;
/// The most active labels one task can carry. Checked only when adding, so
/// removing always works.
pub const MAX_LABELS_PER_TASK: usize = 20;
pub const MAX_NAME_CHARACTERS: usize = 50;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 200;
/// Named colors, and the ones a label gets when none is given.
pub const PALETTE: [(&str, &str); 9] = [
    ("red", "#e5484d"),
    ("orange", "#f76b15"),
    ("yellow", "#ffc53d"),
    ("green", "#30a46c"),
    ("teal", "#12a594"),
    ("blue", "#0090ff"),
    ("purple", "#8e4ec6"),
    ("pink", "#d6409f"),
    ("gray", "#8b8d98"),
];
/// The longest piece of what a caller wrote that a message repeats back.
const ECHO_CHARACTERS: usize = 60;
/// How many label names an error lists before pointing at label_list.
const LISTED_NAMES: i64 = 20;

/// A label name on one line: trimmed, runs of whitespace collapsed to one
/// space, 1 to 50 characters, no control characters, and not shaped like an
/// ID, so "ID or name" is never ambiguous.
pub fn normalize_name(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let name = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = name.chars().count();
    if length == 0 || length > MAX_NAME_CHARACTERS || name.parse::<Uuid>().is_ok() {
        return None;
    }
    Some(name)
}

/// A color as stored: lowercase `#rrggbb`. Takes `#rgb`, `#rrggbb`, either
/// without the `#`, or a palette name ("grey" works for gray).
pub fn normalize_color(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let named = if value == "grey" { "gray" } else { &value };
    if let Some((_, hex)) = PALETTE.iter().find(|(name, _)| *name == named) {
        return Some((*hex).to_owned());
    }
    let digits = value.strip_prefix('#').unwrap_or(&value);
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => Some(
            std::iter::once('#')
                .chain(digits.chars().flat_map(|digit| [digit, digit]))
                .collect(),
        ),
        6 => Some(format!("#{digits}")),
        _ => None,
    }
}

/// What a label means, on one line of at most 200 characters. Empty clears it.
pub fn normalize_label_description(value: &str) -> Option<String> {
    let value = value.trim();
    if value.chars().any(breaks_description) || value.chars().count() > MAX_DESCRIPTION_CHARACTERS {
        return None;
    }
    Some(value.to_owned())
}

/// A control character (Unicode Cc: U+0000 to U+001F and U+007F to U+009F)
/// or a line or paragraph separator (U+2028, U+2029). The database's CHECK
/// on label descriptions names these same code points (migration 0046)
/// rather than `[[:cntrl:]]`, whose meaning follows the database's locale.
fn breaks_description(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// The color a label gets when none is given. It depends only on the name,
/// ignoring case, through SHA-256, so it never shifts between releases.
pub fn default_color(name: &str) -> &'static str {
    let digest = Sha256::digest(name.to_lowercase().as_bytes());
    PALETTE[usize::from(digest[0]) % PALETTE.len()].1
}

#[derive(Clone, Debug, FromRow)]
pub struct LabelRow {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub description: String,
    pub archived: bool,
    pub version: i64,
    /// Tasks that carry it, not counting archived tasks. For an archived
    /// label, the tasks a restore brings it back on.
    pub task_count: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelRef {
    pub id: Uuid,
    pub name: String,
}

/// Selects `LabelRow` columns from `project_labels label`.
const LABEL_COLUMNS: &str = r#"
    label.id, label.name, label.color, label.description,
    label.archived_at IS NOT NULL AS archived, label.version,
    (SELECT count(*)
     FROM work_item_labels applied
     JOIN work_items item
       ON item.project_id = applied.project_id AND item.id = applied.work_item_id
     WHERE applied.project_id = label.project_id AND applied.label_id = label.id
       AND item.archived_at IS NULL) AS task_count
"#;

/// A project's labels, active ones first, then by name.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<LabelRow>, AppError> {
    Ok(sqlx::query_as::<_, LabelRow>(&format!(
        r#"
        SELECT {LABEL_COLUMNS}
        FROM project_labels label
        WHERE label.project_id = $1 AND ($2 OR label.archived_at IS NULL)
        ORDER BY label.archived_at IS NOT NULL, lower(label.name), label.id
        LIMIT {MAX_LABEL_ROWS_PER_PROJECT}
        "#
    ))
    .bind(project_id)
    .bind(include_archived)
    .fetch_all(executor)
    .await?)
}

async fn label_row(
    connection: &mut PgConnection,
    project_id: Uuid,
    label_id: Uuid,
) -> Result<Option<LabelRow>, AppError> {
    Ok(sqlx::query_as::<_, LabelRow>(&format!(
        "SELECT {LABEL_COLUMNS} FROM project_labels label WHERE label.project_id = $1 AND label.id = $2"
    ))
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A project's active label names, sorted, at most `limit`.
pub async fn project_names(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    limit: i64,
) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM project_labels
        WHERE project_id = $1 AND archived_at IS NULL
        ORDER BY lower(name), id
        LIMIT $2
        "#,
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

/// A task's active label names, sorted.
pub async fn active_names(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM work_item_active_labels
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY lower(name), label_id
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(executor)
    .await
}

/// A task's active labels with their colors, sorted by name; what
/// `task_move` carries to the new task.
pub async fn active_labels_with_colors(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<(String, String)>, AppError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT name, color FROM work_item_active_labels
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY lower(name), label_id
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(executor)
    .await?)
}

// ---------------------------------------------------------------------------
// Resolving what callers wrote

/// Labels named by ID or name (ignoring case) among one project's labels.
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    /// Active labels, each once, in the order asked for.
    pub found: Vec<LabelRef>,
    /// What was written for entries that match no label.
    pub unknown: Vec<String>,
    /// Names of entries whose only match is archived.
    pub archived: Vec<String>,
}

impl Resolution {
    /// Every entry named an active label.
    pub fn is_complete(&self) -> bool {
        self.unknown.is_empty() && self.archived.is_empty()
    }

    pub fn ids(&self) -> Vec<Uuid> {
        self.found.iter().map(|label| label.id).collect()
    }
}

/// How a reference is compared: an ID in its canonical form, a name
/// trimmed and collapsed. Anything else is kept as written and matches
/// nothing.
fn reference_value(reference: &str) -> String {
    let trimmed = reference.trim();
    if let Ok(id) = trimmed.parse::<Uuid>() {
        return id.to_string();
    }
    normalize_name(trimmed).unwrap_or_else(|| trimmed.to_owned())
}

/// Resolves label IDs or names among one project's labels. Only active
/// labels count as found; an entry whose only match is archived is listed
/// under `archived`.
pub async fn resolve(
    connection: &mut PgConnection,
    project_id: Uuid,
    references: &[String],
) -> Result<Resolution, AppError> {
    let values = references
        .iter()
        .map(|reference| reference_value(reference))
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (String, Option<Uuid>, Option<String>, Option<bool>)>(
        r#"
        SELECT requested.value, label.id, label.name, label.archived
        FROM unnest($2::text[]) WITH ORDINALITY AS requested(value, ordinality)
        LEFT JOIN LATERAL (
            SELECT id, name, archived_at IS NOT NULL AS archived
            FROM project_labels
            WHERE project_id = $1
              AND (id::text = lower(requested.value) OR lower(name) = lower(requested.value))
            ORDER BY archived_at IS NOT NULL, archived_at DESC
            LIMIT 1
        ) label ON true
        ORDER BY requested.ordinality
        "#,
    )
    .bind(project_id)
    .bind(&values)
    .fetch_all(&mut *connection)
    .await?;
    let mut resolution = Resolution::default();
    for (value, id, name, archived) in rows {
        match (id, name, archived) {
            (Some(id), Some(name), Some(false)) => {
                if !resolution.found.iter().any(|label| label.id == id) {
                    resolution.found.push(LabelRef { id, name });
                }
            }
            (Some(_), Some(name), Some(true)) => {
                if !resolution.archived.contains(&name) {
                    resolution.archived.push(name);
                }
            }
            _ => {
                if !resolution.unknown.contains(&value) {
                    resolution.unknown.push(value);
                }
            }
        }
    }
    Ok(resolution)
}

/// The plain-words error for a resolution that named unknown or archived
/// labels: what's wrong, the project's labels, and the tool that fixes it.
pub async fn unknown_message(
    connection: &mut PgConnection,
    project_id: Uuid,
    resolution: &Resolution,
) -> Result<String, AppError> {
    let key = sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&mut *connection)
        .await?;
    if !resolution.unknown.is_empty() {
        let names = sqlx::query_as::<_, (String, i64)>(
            r#"
            SELECT name, count(*) OVER ()
            FROM project_labels
            WHERE project_id = $1 AND archived_at IS NULL
            ORDER BY lower(name), id
            LIMIT $2
            "#,
        )
        .bind(project_id)
        .bind(LISTED_NAMES)
        .fetch_all(&mut *connection)
        .await?;
        let Some(total) = names.first().map(|(_, total)| *total) else {
            return Ok(format!(
                "{key} has no labels yet. Create one with label_write first."
            ));
        };
        let mut listed = names
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if total > LISTED_NAMES {
            listed.push_str(&format!(
                ", …and {} more (label_list)",
                total - LISTED_NAMES
            ));
        }
        let called = if resolution.unknown.len() == 1 {
            "No label called"
        } else {
            "No labels called"
        };
        return Ok(format!(
            "{called} {} in {key}. Its labels: {listed}. Create one with label_write first.",
            quoted(&resolution.unknown)
        ));
    }
    Ok(if resolution.archived.len() == 1 {
        format!(
            "{} is archived in {key}. Restore it with label_write (archived: false) to use it.",
            quoted(&resolution.archived)
        )
    } else {
        format!(
            "{} are archived in {key}. Restore them with label_write (archived: false) to use them.",
            quoted(&resolution.archived)
        )
    })
}

/// `"a"`, `"a" and "b"`, or `"a", "b" and "c"`, each cut short if long.
fn quoted(values: &[String]) -> String {
    let quoted = values
        .iter()
        .take(LISTED_NAMES as usize)
        .map(|value| format!("\"{}\"", echo(value)))
        .collect::<Vec<_>>();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// What a caller wrote, short enough to repeat back.
pub fn echo(value: &str) -> String {
    if value.chars().count() <= ECHO_CHARACTERS {
        return value.to_owned();
    }
    let mut cut = value.chars().take(ECHO_CHARACTERS - 1).collect::<String>();
    cut.push('…');
    cut
}

/// How a `label` argument (an ID or a name) matched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabelLookup {
    Found(Uuid),
    NotFound,
    /// Several archived labels have the name, and either none is active or
    /// the call restores one.
    AmbiguousArchived,
}

/// Finds one label of the project by ID or name. A name finds the active
/// label with it, or else the one archived label with it. When `restoring`,
/// a name finds the one archived label with it first, so a restore never
/// lands on an active label that took the name since; only without one
/// does it find the active label.
pub async fn find(
    connection: &mut PgConnection,
    project_id: Uuid,
    reference: &str,
    restoring: bool,
) -> Result<LabelLookup, AppError> {
    let value = reference_value(reference);
    let by_id = value.parse::<Uuid>().is_ok();
    // At most one active label has a name, and it sorts first, so three rows
    // are enough to tell whether two archived ones share it.
    let rows = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT id, archived_at IS NOT NULL
        FROM project_labels
        WHERE project_id = $1 AND (id::text = lower($2) OR lower(name) = lower($2))
        ORDER BY archived_at IS NOT NULL, archived_at DESC
        LIMIT 3
        "#,
    )
    .bind(project_id)
    .bind(&value)
    .fetch_all(&mut *connection)
    .await?;
    if by_id {
        // Names are never shaped like IDs, so an ID matches one label.
        return Ok(rows
            .first()
            .map_or(LabelLookup::NotFound, |(id, _)| LabelLookup::Found(*id)));
    }
    let active = rows
        .iter()
        .find(|(_, archived)| !archived)
        .map(|(id, _)| *id);
    let archived = rows
        .iter()
        .filter(|(_, archived)| *archived)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    Ok(match (archived.as_slice(), active) {
        ([only], _) if restoring => LabelLookup::Found(*only),
        ([_, _, ..], _) if restoring => LabelLookup::AmbiguousArchived,
        (_, Some(id)) => LabelLookup::Found(id),
        ([only], None) => LabelLookup::Found(*only),
        ([], None) => LabelLookup::NotFound,
        ([_, _, ..], None) => LabelLookup::AmbiguousArchived,
    })
}

// ---------------------------------------------------------------------------
// Labels on a task

/// Labels to put on a task and take off it, as IDs already resolved among the
/// task's project's active labels in the same transaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct LabelDelta<'a> {
    pub add: &'a [Uuid],
    pub remove: &'a [Uuid],
}

impl LabelDelta<'_> {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// What a label change would do to one task. Adding a label the task has,
/// or removing one it doesn't, changes nothing.
#[derive(Clone, Debug, Default)]
pub struct LabelPlan {
    pub added: Vec<LabelRef>,
    pub removed: Vec<LabelRef>,
    /// The task's active labels after the change.
    pub resulting_count: usize,
}

impl LabelPlan {
    pub fn changes(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

/// Works out a label change without writing anything. A label in both lists
/// is removed, not added; callers refuse that before getting here.
pub async fn plan(
    connection: &mut PgConnection,
    project_id: Uuid,
    work_item_id: Uuid,
    delta: &LabelDelta<'_>,
) -> Result<LabelPlan, AppError> {
    let requested = delta
        .add
        .iter()
        .chain(delta.remove)
        .copied()
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, String, bool, bool)>(
        r#"
        SELECT label.id, label.name, label.archived_at IS NULL AS active,
               EXISTS (
                   SELECT 1 FROM work_item_labels applied
                   WHERE applied.project_id = $1 AND applied.work_item_id = $2
                     AND applied.label_id = label.id
               ) AS present
        FROM project_labels label
        WHERE label.project_id = $1 AND label.id = ANY($3)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(&requested)
    .fetch_all(&mut *connection)
    .await?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_active_labels WHERE project_id = $1 AND work_item_id = $2",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_one(&mut *connection)
    .await?;
    let row = |id: &Uuid| rows.iter().find(|row| row.0 == *id);
    let mut plan = LabelPlan::default();
    let mut removed_active = 0;
    for id in delta.remove {
        if let Some((id, name, active, true)) = row(id)
            && !plan.removed.iter().any(|label| label.id == *id)
        {
            plan.removed.push(LabelRef {
                id: *id,
                name: name.clone(),
            });
            if *active {
                removed_active += 1;
            }
        }
    }
    for id in delta.add {
        if let Some((id, name, true, false)) = row(id)
            && !delta.remove.contains(id)
            && !plan.added.iter().any(|label| label.id == *id)
        {
            plan.added.push(LabelRef {
                id: *id,
                name: name.clone(),
            });
        }
    }
    plan.resulting_count = usize::try_from(current)
        .unwrap_or(0)
        .saturating_add(plan.added.len())
        .saturating_sub(removed_active);
    Ok(plan)
}

/// Writes a planned label change. The caller bumps the task's version and
/// records the change once, with the task's other changes.
pub async fn apply(
    connection: &mut PgConnection,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    plan: &LabelPlan,
) -> Result<(), AppError> {
    if !plan.added.is_empty() {
        let ids = plan.added.iter().map(|label| label.id).collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO work_item_labels (project_id, work_item_id, label_id, added_by_actor_id)
            SELECT $1, $2, label_id, $4 FROM unnest($3::uuid[]) AS label_id
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(&ids)
        .bind(actor_id)
        .execute(&mut *connection)
        .await?;
    }
    if !plan.removed.is_empty() {
        let ids = plan
            .removed
            .iter()
            .map(|label| label.id)
            .collect::<Vec<_>>();
        sqlx::query(
            "DELETE FROM work_item_labels WHERE project_id = $1 AND work_item_id = $2 AND label_id = ANY($3)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(&ids)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Puts active labels on a task being created, under its created activity.
/// Returns the IDs put on it, each once.
pub async fn attach_to_new(
    connection: &mut PgConnection,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    label_ids: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    let mut ids = Vec::with_capacity(label_ids.len());
    for id in label_ids {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    if ids.len() > MAX_LABELS_PER_TASK {
        return Err(AppError::Validation("labels"));
    }
    if ids.is_empty() {
        return Ok(ids);
    }
    // The trigger refuses archived labels and the foreign key refuses another
    // project's, so a caller that resolved them elsewhere fails loudly.
    sqlx::query(
        r#"
        INSERT INTO work_item_labels (project_id, work_item_id, label_id, added_by_actor_id)
        SELECT $1, $2, label_id, $4 FROM unnest($3::uuid[]) AS label_id
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(&ids)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Creating and changing labels

/// Which limit a project reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabelLimit {
    /// MAX_ACTIVE_LABELS_PER_PROJECT active labels.
    Active,
    /// MAX_LABEL_ROWS_PER_PROJECT labels, archived ones included.
    Total,
}

/// A label to create, already normalized.
#[derive(Clone, Copy, Debug)]
pub struct NewLabel<'a> {
    pub name: &'a str,
    /// None picks one from the name.
    pub color: Option<&'a str>,
    pub description: &'a str,
}

#[derive(Clone, Debug)]
pub enum CreateLabelOutcome {
    Created(LabelRow),
    /// An active label already has the name (ignoring case); nothing changed.
    Exists(LabelRow),
    /// Only an archived label has the name; restore it instead.
    ArchivedNameTaken(LabelRow),
    ProjectFull(LabelLimit),
}

async fn active_and_total(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<(i64, i64), AppError> {
    Ok(sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE archived_at IS NULL), count(*) FROM project_labels WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// Creates a label, or returns the active one that already has the name.
/// The actor must be a member of the project.
pub async fn create_label(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    new: NewLabel<'_>,
) -> Result<CreateLabelOutcome, AppError> {
    if normalize_name(new.name).as_deref() != Some(new.name) {
        return Err(AppError::Validation("name"));
    }
    if new
        .color
        .is_some_and(|color| normalize_color(color).as_deref() != Some(color))
    {
        return Err(AppError::Validation("color"));
    }
    if normalize_label_description(new.description).as_deref() != Some(new.description) {
        return Err(AppError::Validation("description"));
    }
    let existing = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT id, archived_at IS NOT NULL FROM project_labels
        WHERE project_id = $1 AND lower(name) = lower($2)
        ORDER BY archived_at IS NOT NULL, archived_at DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(new.name)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some((id, archived)) = existing {
        let row = label_row(connection, project_id, id)
            .await?
            .ok_or(AppError::Internal("label vanished under the project lock"))?;
        return Ok(if archived {
            CreateLabelOutcome::ArchivedNameTaken(row)
        } else {
            CreateLabelOutcome::Exists(row)
        });
    }
    let (active, total) = active_and_total(connection, project_id).await?;
    if active >= MAX_ACTIVE_LABELS_PER_PROJECT {
        return Ok(CreateLabelOutcome::ProjectFull(LabelLimit::Active));
    }
    if total >= MAX_LABEL_ROWS_PER_PROJECT {
        return Ok(CreateLabelOutcome::ProjectFull(LabelLimit::Total));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_labels (id, project_id, name, color, description, created_by_actor_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(new.name)
    .bind(new.color.unwrap_or_else(|| default_color(new.name)))
    .bind(new.description)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    let row = label_row(connection, project_id, id)
        .await?
        .ok_or(AppError::Internal("created label is missing"))?;
    Ok(CreateLabelOutcome::Created(row))
}

/// Changes to one label, already normalized. Fields left as None stay.
#[derive(Clone, Copy, Debug, Default)]
pub struct LabelChange<'a> {
    pub name: Option<&'a str>,
    pub color: Option<&'a str>,
    pub description: Option<&'a str>,
    pub archived: Option<bool>,
    /// Checked only when something would change.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum ChangeLabelOutcome {
    Changed {
        label: LabelRow,
        previous: LabelRow,
        /// Any of name, color, description, archived.
        changed_fields: Vec<&'static str>,
    },
    /// The label already looks like that; nothing was written and the
    /// version wasn't checked.
    Unchanged(LabelRow),
    NotFound,
    VersionConflict(LabelRow),
    /// Another active label has the name, ignoring case.
    NameTaken {
        other: String,
    },
    /// Restoring would pass the active-label limit.
    ProjectFull(LabelLimit),
}

/// Renames, recolors, describes, archives or restores a label. Archiving
/// never touches the tasks that carry it: they show it again once it's
/// restored, and their versions don't change.
pub async fn change_label(
    connection: &mut PgConnection,
    project_id: Uuid,
    label_id: Uuid,
    actor_id: Uuid,
    change: LabelChange<'_>,
) -> Result<ChangeLabelOutcome, AppError> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM project_labels WHERE project_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *connection)
    .await?;
    if locked.is_none() {
        return Ok(ChangeLabelOutcome::NotFound);
    }
    let current = label_row(connection, project_id, label_id)
        .await?
        .ok_or(AppError::Internal("locked label is missing"))?;
    let name = change.name.unwrap_or(&current.name);
    let color = change.color.unwrap_or(&current.color);
    let description = change.description.unwrap_or(&current.description);
    let archived = change.archived.unwrap_or(current.archived);
    if normalize_name(name).as_deref() != Some(name)
        || normalize_color(color).as_deref() != Some(color)
        || normalize_label_description(description).as_deref() != Some(description)
    {
        return Err(AppError::Validation("label"));
    }
    let mut changed_fields = Vec::new();
    if name != current.name {
        changed_fields.push("name");
    }
    if color != current.color {
        changed_fields.push("color");
    }
    if description != current.description {
        changed_fields.push("description");
    }
    if archived != current.archived {
        changed_fields.push("archived");
    }
    if changed_fields.is_empty() {
        return Ok(ChangeLabelOutcome::Unchanged(current));
    }
    if change
        .expected_version
        .is_some_and(|expected| expected != current.version)
    {
        return Ok(ChangeLabelOutcome::VersionConflict(current));
    }
    let restoring = current.archived && !archived;
    if !archived && (restoring || name != current.name) {
        let clash = sqlx::query_scalar::<_, String>(
            r#"
            SELECT name FROM project_labels
            WHERE project_id = $1 AND id <> $2 AND archived_at IS NULL
              AND lower(name) = lower($3)
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(label_id)
        .bind(name)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(other) = clash {
            return Ok(ChangeLabelOutcome::NameTaken { other });
        }
    }
    if restoring
        && active_and_total(connection, project_id).await?.0 >= MAX_ACTIVE_LABELS_PER_PROJECT
    {
        return Ok(ChangeLabelOutcome::ProjectFull(LabelLimit::Active));
    }
    let updated = sqlx::query(
        r#"
        UPDATE project_labels
        SET name = $3, color = $4, description = $5,
            archived_at = CASE WHEN $6 THEN COALESCE(archived_at, now()) END,
            archived_by_actor_id = CASE WHEN $6 THEN COALESCE(archived_by_actor_id, $7) END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(project_id)
    .bind(label_id)
    .bind(name)
    .bind(color)
    .bind(description)
    .bind(archived)
    .bind(actor_id)
    .execute(&mut *connection)
    .await;
    match updated {
        Ok(_) => {}
        // The project lock rules this out; the unique index is the backstop.
        Err(sqlx::Error::Database(error))
            if error.constraint() == Some("project_labels_active_name_unique") =>
        {
            return Ok(ChangeLabelOutcome::NameTaken {
                other: name.to_owned(),
            });
        }
        Err(error) => return Err(error.into()),
    }
    let label = label_row(connection, project_id, label_id)
        .await?
        .ok_or(AppError::Internal("changed label is missing"))?;
    Ok(ChangeLabelOutcome::Changed {
        label,
        previous: current,
        changed_fields,
    })
}

/// Why `ensure_by_name` left a label out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    /// Only an archived label in the project has the name.
    Archived,
    /// The project has no room for another label.
    ProjectFull(LabelLimit),
}

#[derive(Clone, Debug, Default)]
pub struct Ensured {
    /// Active labels for every name that wasn't skipped, each once.
    pub ids: Vec<Uuid>,
    /// Labels this call created.
    pub created: Vec<LabelRef>,
    pub skipped: Vec<(String, SkipReason)>,
}

/// Finds or creates labels by name, for `task_move` and imports that opt in
/// to creating labels. An existing active label keeps its color; a missing
/// one is created with the color given. It skips, and never fails on, a name
/// whose only match is archived or that doesn't fit under the limits.
pub async fn ensure_by_name(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    wanted: &[(String, String)],
) -> Result<Ensured, AppError> {
    let mut ensured = Ensured::default();
    for (name, color) in wanted {
        let Some(name) = normalize_name(name) else {
            continue;
        };
        let color = normalize_color(color);
        let outcome = create_label(
            connection,
            project_id,
            actor_id,
            NewLabel {
                name: &name,
                color: color.as_deref(),
                description: "",
            },
        )
        .await?;
        let id = match outcome {
            CreateLabelOutcome::Created(row) => {
                ensured.created.push(LabelRef {
                    id: row.id,
                    name: row.name,
                });
                row.id
            }
            CreateLabelOutcome::Exists(row) => row.id,
            CreateLabelOutcome::ArchivedNameTaken(row) => {
                ensured.skipped.push((row.name, SkipReason::Archived));
                continue;
            }
            CreateLabelOutcome::ProjectFull(limit) => {
                ensured.skipped.push((name, SkipReason::ProjectFull(limit)));
                continue;
            }
        };
        if !ensured.ids.contains(&id) {
            ensured.ids.push(id);
        }
    }
    Ok(ensured)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        PALETTE, default_color, echo, normalize_color, normalize_label_description, normalize_name,
        quoted,
    };

    #[test]
    fn names_are_one_trimmed_line_that_isnt_an_id() {
        assert_eq!(
            normalize_name("  needs   design ").as_deref(),
            Some("needs design")
        );
        assert_eq!(normalize_name("🐛 bug").as_deref(), Some("🐛 bug"));
        assert_eq!(normalize_name(&"x".repeat(50)), Some("x".repeat(50)));
        assert!(normalize_name("").is_none());
        assert!(normalize_name("   ").is_none());
        assert!(normalize_name(&"x".repeat(51)).is_none());
        assert!(normalize_name("a\nb").is_none());
        assert!(normalize_name("bell\u{7}").is_none());
        assert!(normalize_name(&Uuid::now_v7().to_string()).is_none());
        assert!(normalize_name(&Uuid::now_v7().simple().to_string()).is_none());
    }

    #[test]
    fn colors_are_stored_as_lowercase_hex() {
        assert_eq!(normalize_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize_color("3B82F6").as_deref(), Some("#3b82f6"));
        assert_eq!(normalize_color(" #3b82f6 ").as_deref(), Some("#3b82f6"));
        assert_eq!(normalize_color("Blue").as_deref(), Some("#0090ff"));
        assert_eq!(normalize_color("grey").as_deref(), Some("#8b8d98"));
        assert!(normalize_color("#12345").is_none());
        assert!(normalize_color("chartreuse").is_none());
        assert!(normalize_color("").is_none());
        assert!(normalize_color("#").is_none());
    }

    #[test]
    fn descriptions_are_one_short_line() {
        assert_eq!(
            normalize_label_description("  For crashes ").as_deref(),
            Some("For crashes")
        );
        assert_eq!(normalize_label_description("").as_deref(), Some(""));
        assert!(normalize_label_description("two\nlines").is_none());
        assert!(normalize_label_description(&"x".repeat(201)).is_none());
    }

    #[test]
    fn default_colors_never_shift_between_releases() {
        // Pinned: sha256("bug")[0] = 236, and 236 % 9 = 2.
        assert_eq!(default_color("bug"), "#ffc53d");
        assert_eq!(default_color("Bug"), default_color("bug"));
        assert!(
            PALETTE
                .iter()
                .any(|(_, hex)| *hex == default_color("Mobile"))
        );
    }

    #[test]
    fn messages_quote_what_was_written_briefly() {
        assert_eq!(quoted(&["a".to_owned()]), "\"a\"");
        assert_eq!(
            quoted(&["a".to_owned(), "b".to_owned(), "c".to_owned()]),
            "\"a\", \"b\" and \"c\""
        );
        assert_eq!(echo(&"y".repeat(100)).chars().count(), 60);
    }
}
