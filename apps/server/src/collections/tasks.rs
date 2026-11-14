//! Tasks as the built-in collection (AWD-110). A project's tasks read as the
//! records of its built-in Tasks collection, through the same query, views,
//! groups and pages as any collection's records:
//!
//! * [`SOURCE`] is a subquery over `work_items` with the columns a record
//!   has (aliased `record` by the caller), each task's built-in fields
//!   (status, assignee, priority, dates, labels) folded into its field
//!   values next to its custom ones.
//! * [`fields`] gives the built-in fields, under fixed IDs, then the
//!   project's task fields, so filters, sorts and views name them as they
//!   name any field.
//!
//! Only the collection's views are stored and changed here. Tasks, their
//! fields and the collection itself change on task screens and with the
//! task tools.
//!
//! Every task also has a record in this collection, with the same ID
//! (migration 20261114010000): database triggers copy each task write onto
//! it and refuse any other write to it. Reads still come from `work_items`
//! until the record owns the shared columns.

use sqlx::{FromRow, PgConnection};
use uuid::{Uuid, uuid};

use crate::{
    error::AppError,
    fields::{
        self, ChoiceOption, FieldConfig, FieldDefinition, FieldType, GroupSql, RecordKind, SortKey,
    },
};

/// `collections.builtin` for the Tasks collection.
pub const BUILTIN: &str = "tasks";
pub const NAME: &str = "Tasks";
pub const TITLE_NAME: &str = "Title";
pub const FIRST_VIEW_NAME: &str = "All tasks";
// NAME, TITLE_NAME and FIRST_VIEW_NAME are also written in
// `ensure_tasks_collection` (migration 20261114010000).

pub const STATUS: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000001");
pub const ASSIGNEE: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000002");
pub const PRIORITY: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000003");
pub const START_DATE: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000004");
pub const DUE_DATE: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000005");
pub const LABELS: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000006");

/// Priorities as choice options, most urgent first; "no_priority" is no value.
const PRIORITIES: [(&str, &str, Uuid, &str); 4] = [
    (
        "urgent",
        "Urgent",
        uuid!("0192f5c0-0000-7000-8000-0000000000a1"),
        "#ef4444",
    ),
    (
        "high",
        "High",
        uuid!("0192f5c0-0000-7000-8000-0000000000a2"),
        "#f97316",
    ),
    (
        "medium",
        "Medium",
        uuid!("0192f5c0-0000-7000-8000-0000000000a3"),
        "#eab308",
    ),
    (
        "low",
        "Low",
        uuid!("0192f5c0-0000-7000-8000-0000000000a4"),
        "#3b82f6",
    ),
];

/// The stored priority a Priority option stands for.
pub fn priority_for_option(option: Uuid) -> Option<&'static str> {
    PRIORITIES
        .iter()
        .find(|(_, _, id, _)| *id == option)
        .map(|(stored, _, _, _)| *stored)
}

/// Whether a name is one of the built-in fields' (compared lowercased).
pub fn is_builtin_name(name: &str) -> bool {
    [
        "status",
        "assignee",
        "priority",
        "start date",
        "due date",
        "labels",
    ]
    .contains(&name.trim().to_lowercase().as_str())
}

/// Whether a field is one of the built-in ones.
pub fn is_builtin_field(id: Uuid) -> bool {
    [STATUS, ASSIGNEE, PRIORITY, START_DATE, DUE_DATE, LABELS].contains(&id)
}

/// A status's color, by what kind of status it is.
fn status_color(category: &str) -> &'static str {
    match category {
        "backlog" => "#8b8d98",
        "unstarted" => "#a1a1aa",
        "started" => "#3b82f6",
        "review" | "verification" => "#a855f7",
        "completed" => "#22c55e",
        _ => "#71717a",
    }
}

fn builtin(
    id: Uuid,
    name: &str,
    field_type: FieldType,
    options: Vec<ChoiceOption>,
    position: i32,
) -> FieldDefinition {
    FieldDefinition {
        id,
        name: name.to_owned(),
        description: String::new(),
        field_type,
        config: FieldConfig {
            options,
            target: None,
            collection: None,
        },
        position,
        archived: false,
        version: 1,
    }
}

#[derive(FromRow)]
struct OptionRow {
    id: Uuid,
    name: String,
    color: String,
    archived: bool,
}

/// The fields of a collection whose records are tasks: the built-in ones,
/// then `kind`'s own (archived ones too when asked for). `kind` is
/// `RecordKind::Task` for the Tasks collection, whose own fields are the
/// project's task fields.
pub async fn fields(
    connection: &mut PgConnection,
    project_id: Uuid,
    kind: RecordKind,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    let statuses = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "SELECT id, name, semantic_category, NOT enabled FROM work_item_statuses \
         WHERE project_id = $1 ORDER BY NOT enabled, position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name, category, archived)| ChoiceOption {
        id,
        name,
        color: status_color(&category).to_owned(),
        archived,
    })
    .collect();
    let labels = sqlx::query_as::<_, OptionRow>(
        "SELECT id, name, color, archived_at IS NOT NULL AS archived FROM project_labels \
         WHERE project_id = $1 ORDER BY archived_at IS NOT NULL, lower(name), id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| ChoiceOption {
        id: row.id,
        name: row.name,
        color: row.color,
        archived: row.archived,
    })
    .collect();
    let priorities = PRIORITIES
        .iter()
        .map(|(_, name, id, color)| ChoiceOption {
            id: *id,
            name: (*name).to_owned(),
            color: (*color).to_owned(),
            archived: false,
        })
        .collect();
    let mut found = vec![
        builtin(STATUS, "Status", FieldType::SingleChoice, statuses, 0),
        builtin(ASSIGNEE, "Assignee", FieldType::Person, Vec::new(), 1),
        builtin(PRIORITY, "Priority", FieldType::SingleChoice, priorities, 2),
        builtin(START_DATE, "Start date", FieldType::Date, Vec::new(), 3),
        builtin(DUE_DATE, "Due date", FieldType::Date, Vec::new(), 4),
        builtin(LABELS, "Labels", FieldType::MultipleChoice, labels, 5),
    ];
    let count = found.len() as i32;
    found.extend(
        fields::list(&mut *connection, project_id, kind, include_archived)
            .await?
            .into_iter()
            // A task field named like a built-in one keeps its own name
            // only where it can't be confused: built-in names win.
            .filter(|field| !is_builtin_name(&field.name))
            .map(|mut field| {
                field.position += count;
                field
            }),
    );
    Ok(found)
}

fn priority_value() -> String {
    let arms = PRIORITIES
        .iter()
        .map(|(stored, _, id, _)| format!("WHEN '{stored}' THEN '{id}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("CASE task.priority {arms} END")
}

/// The project's tasks as records, aliased by the caller: the columns
/// `collection_records` has, built-in fields folded into `field_values`.
/// Each is in its own collection: the Tasks collection, or the one that
/// acts like a task it was made in (`work_items.collection_id`).
pub fn source() -> String {
    format!(
        "(SELECT task.id, task.project_id, COALESCE(task.collection_id, tasks.id) AS collection_id, task.number, task.title, \
           task.field_values || jsonb_strip_nulls(jsonb_build_object( \
             '{STATUS}', task.status_id::text, \
             '{ASSIGNEE}', task.assignee_actor_id::text, \
             '{PRIORITY}', {priority}, \
             '{START_DATE}', task.start_date::text, \
             '{DUE_DATE}', task.due_date::text, \
             '{LABELS}', (SELECT jsonb_agg(label.label_id::text ORDER BY label.label_id) \
                          FROM work_item_labels label \
                          WHERE label.project_id = task.project_id AND label.work_item_id = task.id) \
           )) AS field_values, \
           task.created_by_actor_id, task.created_by_actor_id AS updated_by_actor_id, \
           task.archived_at, task.version, task.created_at, task.updated_at \
         FROM work_items task \
         JOIN collections tasks ON tasks.project_id = task.project_id AND tasks.builtin = '{BUILTIN}')",
        priority = priority_value(),
    )
}

/// Grouping by Status or Priority, whose options aren't stored in
/// `field_definitions`: their names come from the statuses and the fixed
/// priorities. None for any other field.
pub fn group_sql(field: &FieldDefinition, column: &str, project: &str) -> Option<GroupSql> {
    let text = format!("({column} ->> '{}')", field.id);
    let join = if field.id == STATUS {
        format!(
            " LEFT JOIN LATERAL (SELECT status.name, \
               CASE status.semantic_category {colors} ELSE '#71717a' END AS color, \
               status.position::bigint AS position \
               FROM work_item_statuses status \
               WHERE status.project_id = {project} AND status.id::text = {text}) group_field ON true",
            colors = [
                "backlog",
                "unstarted",
                "started",
                "review",
                "verification",
                "completed"
            ]
            .iter()
            .map(|category| format!("WHEN '{category}' THEN '{}'", status_color(category)))
            .collect::<Vec<_>>()
            .join(" "),
        )
    } else if field.id == PRIORITY {
        let values = PRIORITIES
            .iter()
            .enumerate()
            .map(|(index, (_, name, id, color))| format!("('{id}', '{name}', '{color}', {index})"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            " LEFT JOIN LATERAL (SELECT choice.name, choice.color, choice.position::bigint AS position \
               FROM (VALUES {values}) AS choice(id, name, color, position) \
               WHERE choice.id = {text}) group_field ON true"
        )
    } else {
        return None;
    };
    Some(GroupSql {
        join,
        key: format!("CASE WHEN group_field.name IS NULL THEN 'none' ELSE {text} END"),
        label: "COALESCE(group_field.name, 'No value')".to_owned(),
        color: "group_field.color".to_owned(),
        order: vec![
            SortKey {
                expr: "(group_field.name IS NULL)::integer".to_owned(),
                ty: "integer",
                follows_direction: true,
            },
            SortKey {
                expr: "COALESCE(group_field.position, 0)".to_owned(),
                ty: "bigint",
                follows_direction: true,
            },
        ],
    })
}

/// Makes the project's Tasks collection, with its first view, if it has
/// none yet, as `actor_id` when they're in the project. Idempotent. A
/// project's first task makes it too, through the same SQL function
/// (`ensure_tasks_collection`, migration 20261114010000), which gives it the
/// first free name of "Tasks", "Tasks (built in)", "Tasks (built in 2)" and
/// so on, so a collection already called Tasks keeps its name.
pub async fn ensure(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        SELECT ensure_tasks_collection(project.id, member.actor_id)
        FROM projects project
        JOIN project_memberships member
          ON member.project_id = project.id AND member.actor_id = $2
        WHERE project.id = $1 AND project.kind <> 'direct'
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// What to say when a write asks to change the Tasks collection's records,
/// fields, or the collection itself.
pub const READ_ONLY_MESSAGE: &str = "Tasks is the built-in collection of this project's tasks: \
     its name and settings don't change. Change its records with the task tools (task_create, \
     task_update), and its fields with collection_write.";

/// What to say when `record_write` names a collection whose records are
/// tasks.
pub fn task_like_message(name: &str) -> String {
    format!(
        "{name}'s records act like tasks: add them with task_create (collection: \"{name}\") and \
         change them with task_update, by key or ID."
    )
}
