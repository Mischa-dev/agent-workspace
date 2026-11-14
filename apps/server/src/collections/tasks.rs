//! Tasks as the built-in collection (AWD-110). A project's tasks read as the
//! records of its built-in Tasks collection, through the same query, views,
//! groups and pages as any collection's records:
//!
//! * [`TasksSource`] reads each record with its task beside it
//!   (`collection_records record JOIN work_items core`, same ID), and the
//!   built-in fields read the task's own columns ([`accessor`]), so
//!   filters, sorts and groups on status, assignee or dates use the task's
//!   indexes (T1.7). [`source`] folds those fields into a record's values
//!   for showing a page of records.
//! * [`fields`] gives the built-in fields, under fixed IDs, then the
//!   project's task fields, so filters, sorts and views name them as they
//!   name any field; [`system_fields`] adds the rest a task has (type,
//!   milestone, parent, blocked, ready, claimed by, estimate, who made it,
//!   when, and its key).
//!
//! Only the collection's views are stored and changed here. Tasks, their
//! fields and the collection itself change on task screens and with the
//! task tools.
//!
//! Every task also has a record in this collection, with the same ID
//! (migration 20261114010000): database triggers copy each task write onto
//! it and refuse any other write to it. Reads still come from `work_items`
//! until the record owns the shared columns.

use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder};
use uuid::{Uuid, uuid};

use crate::{
    error::AppError,
    fields::{
        self, Accessor, ChoiceOption, FieldConfig, FieldDefinition, FieldType, GroupSql,
        LinkTarget, RecordKind, SortKey,
    },
    views::source::RecordSource,
    work_item_planning::HAS_OPEN_BLOCKER,
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
// The built-in fields views over tasks read beyond the six a collection
// lists (T1.7). 00d and 00e are kept for Delegate and Cycle.
pub const TYPE: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000007");
pub const MILESTONE: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000008");
pub const PARENT: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000009");
pub const BLOCKED: Uuid = uuid!("0192f5c0-0000-7000-8000-00000000000a");
pub const READY: Uuid = uuid!("0192f5c0-0000-7000-8000-00000000000b");
pub const CLAIMED_BY: Uuid = uuid!("0192f5c0-0000-7000-8000-00000000000c");
pub const ESTIMATE: Uuid = uuid!("0192f5c0-0000-7000-8000-00000000000f");
pub const CREATED_BY: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000010");
pub const CREATED: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000011");
pub const EDITED: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000012");
pub const KEY: Uuid = uuid!("0192f5c0-0000-7000-8000-000000000013");

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

/// SQL for the color of a status of `category` (an expression).
fn status_color_sql(category: &str) -> String {
    let arms = [
        "backlog",
        "unstarted",
        "started",
        "review",
        "verification",
        "completed",
    ]
    .iter()
    .map(|known| format!("WHEN '{known}' THEN '{}'", status_color(known)))
    .collect::<Vec<_>>()
    .join(" ");
    format!("CASE {category} {arms} ELSE '#71717a' END")
}

/// A task's own column, as a field reads it (record JOIN core, T1.7).
fn column(expr: impl Into<String>, sql_type: &'static str) -> Accessor {
    Accessor::Column {
        expr: expr.into(),
        sql_type,
        category: None,
    }
}

/// SQL for whether something unfinished blocks the task aliased `core`.
fn has_open_blocker() -> String {
    HAS_OPEN_BLOCKER.replace("item.", "core.")
}

/// SQL for the task's status category: its status is joined beside it
/// (`core_status`, in `TasksSource::from_sql`).
const STATUS_CATEGORY: &str = "core_status.semantic_category";

/// Where each built-in field's value is read from, by ID: the task's own
/// columns (`core`) and the record's (`record`), so filters, sorts and groups
/// use their indexes instead of reading a folded value.
pub fn accessor(id: Uuid) -> Option<Accessor> {
    Some(match id {
        STATUS => Accessor::Column {
            expr: "core.status_id".to_owned(),
            sql_type: "uuid",
            category: Some(STATUS_CATEGORY.to_owned()),
        },
        ASSIGNEE => column("core.assignee_actor_id", "uuid"),
        PRIORITY => column(priority_value("core"), "text"),
        START_DATE => column("core.start_date", "date"),
        DUE_DATE => column("core.due_date", "date"),
        LABELS => Accessor::Elements {
            sql: "SELECT label.label_id::text FROM work_item_labels label \
                  WHERE label.project_id = core.project_id AND label.work_item_id = core.id"
                .to_owned(),
        },
        TYPE => column("core.type_id", "uuid"),
        MILESTONE => column("core.milestone_id", "uuid"),
        PARENT => column("core.parent_work_item_id", "uuid"),
        BLOCKED => column(
            format!(
                "CASE WHEN core.blocked_reason IS NOT NULL OR {} THEN true END",
                has_open_blocker()
            ),
            "boolean",
        ),
        READY => column(
            format!(
                "CASE WHEN {STATUS_CATEGORY} NOT IN ('completed', 'canceled') \
                 AND core.blocked_reason IS NULL AND NOT {} THEN true END",
                has_open_blocker()
            ),
            "boolean",
        ),
        CLAIMED_BY => column(
            "(SELECT claim.actor_id FROM work_item_claims claim \
              WHERE claim.work_item_id = core.id AND claim.expires_at > now() \
              ORDER BY claim.expires_at DESC LIMIT 1)",
            "uuid",
        ),
        ESTIMATE => column("core.estimate", "double precision"),
        CREATED_BY => column("core.created_by_actor_id", "uuid"),
        CREATED => column("core.created_at", "timestamptz"),
        EDITED => column("core.updated_at", "timestamptz"),
        KEY => column("core.number", "bigint"),
        _ => return None,
    })
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
        accessor: accessor(id).unwrap_or_default(),
    }
}

#[derive(FromRow)]
struct OptionRow {
    id: Uuid,
    name: String,
    color: String,
    archived: bool,
}

/// The six built-in fields every collection whose records are tasks lists.
async fn listed(
    connection: &mut PgConnection,
    project_id: Uuid,
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
        category: Some(category),
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
        category: None,
    })
    .collect();
    let priorities = PRIORITIES
        .iter()
        .map(|(_, name, id, color)| ChoiceOption {
            id: *id,
            name: (*name).to_owned(),
            color: (*color).to_owned(),
            archived: false,
            category: None,
        })
        .collect();
    Ok(vec![
        builtin(STATUS, "Status", FieldType::SingleChoice, statuses, 0),
        builtin(ASSIGNEE, "Assignee", FieldType::Person, Vec::new(), 1),
        builtin(PRIORITY, "Priority", FieldType::SingleChoice, priorities, 2),
        builtin(START_DATE, "Start date", FieldType::Date, Vec::new(), 3),
        builtin(DUE_DATE, "Due date", FieldType::Date, Vec::new(), 4),
        builtin(LABELS, "Labels", FieldType::MultipleChoice, labels, 5),
    ])
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
    let mut found = listed(&mut *connection, project_id).await?;
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

/// Every built-in field a task has, for views over tasks (T1.7): the six
/// the collection lists, then its type, milestone, parent, whether it's
/// blocked or ready, who claimed it, its estimate, who made it, when it was
/// made and changed, and its key. Views filter, sort and group by these by
/// ID; the collection screens list them once T1.11 opens Tasks for editing.
pub async fn system_fields(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Vec<FieldDefinition>, AppError> {
    let mut found = listed(&mut *connection, project_id).await?;
    let types = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, NOT enabled FROM work_item_types \
         WHERE project_id = $1 ORDER BY NOT enabled, position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name, archived)| ChoiceOption {
        id,
        name,
        color: "#71717a".to_owned(),
        archived,
        category: None,
    })
    .collect();
    let milestones = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, archived_at IS NOT NULL FROM project_milestones \
         WHERE project_id = $1 \
         ORDER BY archived_at IS NOT NULL, target_date NULLS LAST, lower(name), id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name, archived)| ChoiceOption {
        id,
        name,
        color: "#71717a".to_owned(),
        archived,
        category: None,
    })
    .collect();
    let mut parent = builtin(PARENT, "Parent", FieldType::Link, Vec::new(), 8);
    parent.config.target = Some(LinkTarget::Task);
    found.extend([
        builtin(TYPE, "Type", FieldType::SingleChoice, types, 6),
        builtin(
            MILESTONE,
            "Milestone",
            FieldType::SingleChoice,
            milestones,
            7,
        ),
        parent,
        builtin(BLOCKED, "Blocked", FieldType::Checkbox, Vec::new(), 9),
        builtin(READY, "Ready", FieldType::Checkbox, Vec::new(), 10),
        builtin(CLAIMED_BY, "Claimed by", FieldType::Person, Vec::new(), 11),
        builtin(ESTIMATE, "Estimate", FieldType::Number, Vec::new(), 12),
        builtin(CREATED_BY, "Created by", FieldType::Person, Vec::new(), 13),
        builtin(CREATED, "Created", FieldType::Date, Vec::new(), 14),
        builtin(EDITED, "Edited", FieldType::Date, Vec::new(), 15),
        builtin(KEY, "Key", FieldType::Number, Vec::new(), 16),
    ]);
    Ok(found)
}

/// A task's priority as its Priority option's ID, for the task aliased
/// `task`; NULL for no priority.
fn priority_value(task: &str) -> String {
    let arms = PRIORITIES
        .iter()
        .map(|(stored, _, id, _)| format!("WHEN '{stored}' THEN '{id}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("CASE {task}.priority {arms} END")
}

/// A collection's tasks as records, aliased by the caller: the columns
/// `collection_records` has, the task's built-in fields folded into
/// `field_values` beside its own, so a record reads the same whatever
/// collection it's in. Used to show records a page at a time (the query
/// itself reads columns through `TasksSource`). The version is the task's,
/// which task writes check.
pub fn source() -> String {
    format!(
        "(SELECT record.id, record.project_id, record.collection_id, record.number, record.title, \
           record.field_values || jsonb_strip_nulls(jsonb_build_object( \
             '{STATUS}', core.status_id::text, \
             '{ASSIGNEE}', core.assignee_actor_id::text, \
             '{PRIORITY}', {priority}, \
             '{START_DATE}', core.start_date::text, \
             '{DUE_DATE}', core.due_date::text, \
             '{LABELS}', (SELECT jsonb_agg(label.label_id::text ORDER BY label.label_id) \
                          FROM work_item_labels label \
                          WHERE label.project_id = core.project_id AND label.work_item_id = core.id) \
           )) AS field_values, \
           core.created_by_actor_id, core.created_by_actor_id AS updated_by_actor_id, \
           record.archived_at, core.version, record.created_at, record.updated_at \
         FROM collection_records record JOIN work_items core ON core.id = record.id)",
        priority = priority_value("core"),
    )
}

/// The records of a collection whose records are tasks, each joined to its
/// task (`record JOIN core`, same ID), with the built-in fields read from
/// the task's columns (`accessor`).
#[derive(Clone, Copy, Debug)]
pub struct TasksSource {
    pub project_id: Uuid,
    pub collection_id: Uuid,
}

impl RecordSource for TasksSource {
    fn from_sql(&self) -> String {
        "collection_records record JOIN work_items core ON core.id = record.id \
         JOIN work_item_statuses core_status \
           ON core_status.project_id = core.project_id AND core_status.id = core.status_id"
            .to_owned()
    }

    fn scope(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        builder.push(" WHERE record.project_id = ");
        builder.push_bind(self.project_id);
        builder.push(" AND record.collection_id = ");
        builder.push_bind(self.collection_id);
        // The task is in the same project; saying so lets the planner use
        // the task's own project indexes.
        builder.push(" AND core.project_id = ");
        builder.push_bind(self.project_id);
    }

    fn group_sql(&self, field: &FieldDefinition) -> Option<GroupSql> {
        group_sql(field.id)
    }

    fn extra_sort(&self, key: &str) -> Option<Vec<SortKey>> {
        (key == RANK).then(|| {
            vec![
                SortKey {
                    expr: "core_status.position".to_owned(),
                    ty: "integer",
                    follows_direction: true,
                },
                SortKey {
                    expr: "core.board_rank".to_owned(),
                    ty: "bigint",
                    follows_direction: true,
                },
            ]
        })
    }

    fn identity(&self) -> String {
        format!("tasks:{}:{}", self.project_id, self.collection_id)
    }
}

/// The sort key only tasks have: their order on the board (status, then
/// the place in its column).
pub const RANK: &str = "rank";

fn order(expr: &str, ty: &'static str) -> SortKey {
    SortKey {
        expr: expr.to_owned(),
        ty,
        follows_direction: true,
    }
}

/// Grouping by a built-in field whose groups are the task's own things:
/// statuses, priorities, types, milestones, parents and labels. None for
/// any other field (people and checkboxes group the field layer's way).
pub fn group_sql(field: Uuid) -> Option<GroupSql> {
    let none_last = |id: &str| order(&format!("({id} IS NULL)::integer"), "integer");
    Some(match field {
        STATUS => GroupSql {
            join: " LEFT JOIN work_item_statuses group_field \
                   ON group_field.project_id = core.project_id AND group_field.id = core.status_id"
                .to_owned(),
            key: "COALESCE(group_field.id::text, 'none')".to_owned(),
            label: "COALESCE(group_field.name, 'No value')".to_owned(),
            color: status_color_sql("group_field.semantic_category"),
            order: vec![
                none_last("group_field.id"),
                order("COALESCE(group_field.position, 0)", "integer"),
                order("COALESCE(group_field.id::text, '')", "text"),
            ],
        },
        PRIORITY => {
            let values = PRIORITIES
                .iter()
                .enumerate()
                .map(|(index, (stored, name, id, color))| {
                    format!("('{stored}', '{id}', '{name}', '{color}', {index})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            GroupSql {
                join: format!(
                    " LEFT JOIN (VALUES {values}) AS group_field(stored, id, name, color, position) \
                     ON group_field.stored = core.priority"
                ),
                key: "COALESCE(group_field.id, 'none')".to_owned(),
                label: "COALESCE(group_field.name, 'No value')".to_owned(),
                color: "group_field.color".to_owned(),
                order: vec![
                    none_last("group_field.id"),
                    order("COALESCE(group_field.position, 0)", "integer"),
                ],
            }
        }
        TYPE => GroupSql {
            join: " LEFT JOIN work_item_types group_field \
                   ON group_field.project_id = core.project_id AND group_field.id = core.type_id"
                .to_owned(),
            key: "COALESCE(group_field.id::text, 'none')".to_owned(),
            label: "COALESCE(group_field.name, 'No value')".to_owned(),
            color: "NULL::text".to_owned(),
            order: vec![
                none_last("group_field.id"),
                order("COALESCE(group_field.position, 0)", "integer"),
                order("COALESCE(group_field.id::text, '')", "text"),
            ],
        },
        MILESTONE => GroupSql {
            join: " LEFT JOIN project_milestones group_field \
                   ON group_field.project_id = core.project_id AND group_field.id = core.milestone_id"
                .to_owned(),
            key: "COALESCE(group_field.id::text, 'none')".to_owned(),
            label: "COALESCE(group_field.name, 'No milestone')".to_owned(),
            color: "NULL::text".to_owned(),
            order: vec![
                none_last("group_field.id"),
                order("COALESCE(group_field.target_date, 'infinity'::date)", "date"),
                order(
                    &format!(
                        "left(lower(COALESCE(group_field.name, '')), {})",
                        fields::SORT_PREFIX_CHARACTERS
                    ),
                    "text",
                ),
                order("COALESCE(group_field.id::text, '')", "text"),
            ],
        },
        PARENT => GroupSql {
            join: " LEFT JOIN work_items group_field \
                   ON group_field.project_id = core.project_id \
                  AND group_field.id = core.parent_work_item_id \
                 LEFT JOIN projects group_project ON group_project.id = core.project_id"
                .to_owned(),
            key: "COALESCE(group_field.id::text, 'none')".to_owned(),
            label: "COALESCE(group_project.project_key || '-' || group_field.number || ' ' || group_field.title, 'No parent')"
                .to_owned(),
            color: "NULL::text".to_owned(),
            order: vec![
                none_last("group_field.id"),
                order("COALESCE(group_field.number, 0)", "bigint"),
            ],
        },
        // One group for each of a task's labels: a task with two is in
        // both.
        LABELS => GroupSql {
            join: " LEFT JOIN work_item_active_labels group_field \
                   ON group_field.project_id = core.project_id AND group_field.work_item_id = core.id"
                .to_owned(),
            key: "COALESCE(group_field.label_id::text, 'none')".to_owned(),
            label: "COALESCE(group_field.name, 'No label')".to_owned(),
            color: "group_field.color".to_owned(),
            order: vec![
                none_last("group_field.label_id"),
                order(
                    &format!(
                        "left(lower(COALESCE(group_field.name, '')), {})",
                        fields::SORT_PREFIX_CHARACTERS
                    ),
                    "text",
                ),
                order("COALESCE(group_field.label_id::text, '')", "text"),
            ],
        },
        _ => return None,
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
