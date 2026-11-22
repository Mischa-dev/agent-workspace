//! Initiatives (T1.26): work grouped across spaces. An initiative is a
//! record of the workspace's Initiatives collection, which lives in the
//! workspace's own space and is known by its role (`collections.role`), so
//! it has fields, views, comments and history like any collection. It is
//! not a space. Its parts are milestones: a milestone's
//! `initiative_id` is the initiative record, which makes the record the
//! milestone thing's parent (containment, R11). A space takes part in an
//! initiative through its milestones there.
//!
//! Progress is worked out as it is read, for the reader: a milestone in a
//! space the reader can't see is left out with its tasks, so two people may
//! see different counts for the same initiative.

use std::collections::HashMap;

use chrono::NaiveDate;
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access::{self, Caller},
    collections::{
        self, Collection, CreateOutcome, NewCollection,
        definition::{self, FieldWrite},
    },
    error::AppError,
    fields::{self, OptionInput},
    policy::Permission,
    work_item_fields::Writer,
};

/// What `collections.role` calls the workspace's Initiatives collection.
pub const ROLE: &str = "initiatives";
pub const COLLECTION_NAME: &str = "Initiatives";
/// Initiatives one list shows, at most.
const MAX_LISTED: i64 = 500;

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Initiatives {
    /// The workspace's own space, where the Initiatives collection is.
    pub space_id: Option<String>,
    /// The Initiatives collection: its records are the initiatives, changed
    /// with record_update_many like any record. None until the first is made.
    pub collection_id: Option<String>,
    pub initiatives: Vec<Initiative>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Initiative {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub version: i64,
    pub archived: bool,
    /// Its fields as they read (owner, status, target date, health and any
    /// added since), in order; empty ones left out.
    pub fields: Vec<InitiativeField>,
    /// Its milestones in spaces you can see, by target date.
    pub milestones: Vec<InitiativeMilestone>,
    /// Tasks in those milestones, leaving out canceled ones.
    pub scope: i64,
    pub completed: i64,
    /// The spaces of those milestones, each with its health and latest
    /// update (T1.43).
    pub spaces: Vec<InitiativeSpace>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitiativeSpace {
    pub space_name: String,
    pub space_key: String,
    #[serde(flatten)]
    pub status: super::project_status::SpaceStatus,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitiativeField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub value: Value,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitiativeMilestone {
    pub id: String,
    pub name: String,
    pub space_id: String,
    pub space_name: String,
    pub space_key: String,
    #[schemars(with = "Option<String>")]
    pub target_date: Option<NaiveDate>,
    pub archived: bool,
    pub scope: i64,
    pub completed: i64,
}

/// The workspace's own space and its Initiatives collection, if made.
pub async fn find(
    connection: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Option<(Uuid, Collection)>, AppError> {
    let Some(space) = crate::spaces::workspace::find(connection, workspace_id).await? else {
        return Ok(None);
    };
    let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections WHERE project_id = $1 AND role = $2",
    )
    .bind(space)
    .bind(ROLE)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    Ok(collections::get(&mut *connection, space, id)
        .await?
        .map(|collection| (space, collection)))
}

/// The fields a new Initiatives collection starts with.
fn starting_fields() -> Vec<FieldWrite> {
    let option = |name: &str, color: Option<&str>, category: Option<&str>| OptionInput {
        id: None,
        name: name.to_owned(),
        color: color.map(str::to_owned),
        category: category.map(str::to_owned),
    };
    let field = |name: &str, field_type: &str, options: Option<Vec<OptionInput>>| FieldWrite {
        name: Some(name.to_owned()),
        field_type: Some(field_type.to_owned()),
        options,
        ..FieldWrite::default()
    };
    vec![
        field("Owner", "person", None),
        field(
            "Status",
            "status",
            Some(vec![
                option("Planned", None, Some("unstarted")),
                option("Active", None, Some("started")),
                option("Done", None, Some("completed")),
                option("Canceled", None, Some("canceled")),
            ]),
        ),
        field("Target date", "date", None),
        field(
            "Health",
            "single_choice",
            Some(vec![
                option("On track", Some("green"), None),
                option("At risk", Some("yellow"), None),
                option("Off track", Some("red"), None),
            ]),
        ),
    ]
}

/// The Initiatives collection, made with its starting fields the first
/// time. A collection already called Initiatives in the workspace space
/// becomes it, keeping its fields.
pub async fn ensure(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_id: Uuid,
) -> Result<(Uuid, Collection), AppError> {
    if let Some(found) = find(transaction, workspace_id).await? {
        return Ok(found);
    }
    let space = crate::spaces::workspace::ensure(transaction, workspace_id).await?;
    let (collection, created) = match collections::create(
        transaction,
        space,
        actor_id,
        NewCollection {
            name: COLLECTION_NAME.to_owned(),
            description: "Work that spans spaces, followed through its milestones.".to_owned(),
            title_name: "Name".to_owned(),
            first_view: true,
        },
    )
    .await?
    {
        CreateOutcome::Created(collection) => (collection, true),
        CreateOutcome::NameTaken(collection) => (collection, false),
        CreateOutcome::ProjectFull => {
            return Err(AppError::Refused {
                field: "initiative",
                message: "The workspace's space has as many collections as it can hold.",
            });
        }
    };
    if created {
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space)
                .fetch_one(&mut **transaction)
                .await?;
        definition::apply(
            transaction,
            space,
            &collection,
            Writer {
                actor_id,
                project_key: &project_key,
            },
            &starting_fields(),
            &[],
        )
        .await?
        .map_err(|_| AppError::Internal("the Initiatives fields didn't apply"))?;
    }
    sqlx::query("UPDATE collections SET role = $2 WHERE id = $1")
        .bind(collection.id)
        .bind(ROLE)
        .execute(&mut **transaction)
        .await?;
    let collection = collections::get(&mut **transaction, space, collection.id)
        .await?
        .ok_or(AppError::NotFound("collection"))?;
    Ok((space, collection))
}

/// An initiative by its record ID, #number or title, among the active
/// ones.
pub async fn resolve(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    reference: &str,
) -> Result<Option<Uuid>, AppError> {
    let Some((space, collection)) = find(connection, workspace_id).await? else {
        return Ok(None);
    };
    let reference = reference.trim();
    let id = Uuid::parse_str(reference).ok();
    let number = reference
        .strip_prefix('#')
        .unwrap_or(reference)
        .parse::<i64>()
        .ok();
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_records \
         WHERE project_id = $1 AND collection_id = $2 AND archived_at IS NULL \
           AND (id = $3 OR number = $4 OR lower(title) = lower($5)) \
         ORDER BY id = $3 IS TRUE DESC, number = $4 IS TRUE DESC, number LIMIT 1",
    )
    .bind(space)
    .bind(collection.id)
    .bind(id)
    .bind(number)
    .bind(reference)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The initiative a milestone is part of, as people read it: its title.
pub async fn title_of(
    connection: &mut PgConnection,
    initiative_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT record.title FROM collection_records record \
         JOIN collections collection ON collection.id = record.collection_id \
         WHERE record.id = $1 AND collection.role = $2",
    )
    .bind(initiative_id)
    .bind(ROLE)
    .fetch_optional(&mut *connection)
    .await?)
}

#[derive(sqlx::FromRow)]
struct RecordRow {
    id: Uuid,
    number: i64,
    title: String,
    version: i64,
    archived: bool,
    field_values: Value,
}

#[derive(sqlx::FromRow)]
struct MilestoneRow {
    id: Uuid,
    initiative_id: Uuid,
    name: String,
    space_id: Uuid,
    space_name: String,
    space_key: String,
    target_date: Option<NaiveDate>,
    archived: bool,
    scope: i64,
    completed: i64,
}

/// The workspace's initiatives with their progress, as `caller` sees it.
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    caller: &Caller,
    include_archived: bool,
) -> Result<Initiatives, AppError> {
    let Some((space, collection)) = find(connection, workspace_id).await? else {
        return Ok(Initiatives {
            space_id: crate::spaces::workspace::find(connection, workspace_id)
                .await?
                .map(|id| id.to_string()),
            collection_id: None,
            initiatives: Vec::new(),
        });
    };
    let records = sqlx::query_as::<_, RecordRow>(
        "SELECT id, number, title, version, archived_at IS NOT NULL AS archived, field_values \
         FROM collection_records \
         WHERE project_id = $1 AND collection_id = $2 AND ($3 OR archived_at IS NULL) \
         ORDER BY archived_at IS NOT NULL, number LIMIT $4",
    )
    .bind(space)
    .bind(collection.id)
    .bind(include_archived)
    .bind(MAX_LISTED)
    .fetch_all(&mut *connection)
    .await?;
    let definitions = fields::list(&mut *connection, space, collection.field_kind(), false).await?;
    let ids: Vec<Uuid> = records.iter().map(|record| record.id).collect();
    // Only milestones in spaces the caller may see tasks in, and only
    // their tasks there.
    let filter = access::sql_filter(caller, Permission::TasksView, "milestone.project_id", 2);
    let sql = format!(
        r#"
        SELECT milestone.id, milestone.initiative_id, milestone.name,
               milestone.project_id AS space_id, project.name AS space_name,
               project.project_key AS space_key, milestone.target_date,
               milestone.archived_at IS NOT NULL AS archived,
               count(task.id) FILTER (WHERE status.semantic_category <> 'canceled') AS scope,
               count(task.id) FILTER (WHERE status.semantic_category = 'completed') AS completed
        FROM project_milestones milestone
        JOIN projects project ON project.id = milestone.project_id
        LEFT JOIN task_rows task
          ON task.project_id = milestone.project_id AND task.milestone_id = milestone.id
         AND task.archived_at IS NULL
        LEFT JOIN work_item_statuses status
          ON status.project_id = task.project_id AND status.id = task.status_id
        WHERE milestone.initiative_id = ANY($1) AND project.archived_at IS NULL
          AND {}
        GROUP BY milestone.id, project.name, project.project_key
        ORDER BY milestone.target_date NULLS LAST, lower(milestone.name), milestone.id
        "#,
        filter.sql
    );
    let mut query = sqlx::query_as::<_, MilestoneRow>(&sql)
        .bind(&ids)
        .bind(filter.account_id)
        .bind(filter.connection_id)
        .bind(filter.permission);
    if let Some(limited_to) = &filter.limited_to {
        query = query.bind(limited_to);
    }
    let milestones = query.fetch_all(&mut *connection).await?;
    let mut by_initiative: HashMap<Uuid, Vec<InitiativeMilestone>> = HashMap::new();
    for row in milestones {
        by_initiative
            .entry(row.initiative_id)
            .or_default()
            .push(InitiativeMilestone {
                id: row.id.to_string(),
                name: row.name,
                space_id: row.space_id.to_string(),
                space_name: row.space_name,
                space_key: row.space_key,
                target_date: row.target_date,
                archived: row.archived,
                scope: row.scope,
                completed: row.completed,
            });
    }
    let mut spaces: Vec<Uuid> = by_initiative
        .values()
        .flatten()
        .filter_map(|milestone| Uuid::parse_str(&milestone.space_id).ok())
        .collect();
    spaces.sort_unstable();
    spaces.dedup();
    let statuses =
        super::project_status::statuses_of(&mut *connection, caller.account_id, &spaces).await?;
    let names = person_names(connection, space, &definitions, &records).await?;
    let name_of = |id: &str| names.get(id).cloned();
    let initiatives = records
        .into_iter()
        .map(|record| {
            let milestones = by_initiative.remove(&record.id).unwrap_or_default();
            let fields = definitions
                .iter()
                .filter_map(|field| {
                    let stored = record.field_values.get(field.id.to_string())?;
                    let value = fields::display_value(field, stored, &name_of);
                    (!value.is_null() && value != Value::String(String::new())).then(|| {
                        InitiativeField {
                            name: field.name.clone(),
                            field_type: field.field_type.as_str().to_owned(),
                            value,
                        }
                    })
                })
                .collect();
            let mut spaces: Vec<InitiativeSpace> = Vec::new();
            for milestone in &milestones {
                if spaces
                    .iter()
                    .any(|space| space.status.space_id == milestone.space_id)
                {
                    continue;
                }
                let Some(status) = Uuid::parse_str(&milestone.space_id)
                    .ok()
                    .and_then(|id| statuses.get(&id))
                else {
                    continue;
                };
                spaces.push(InitiativeSpace {
                    space_name: milestone.space_name.clone(),
                    space_key: milestone.space_key.clone(),
                    status: status.clone(),
                });
            }
            Initiative {
                id: record.id.to_string(),
                spaces,
                number: record.number,
                title: record.title,
                version: record.version,
                archived: record.archived,
                fields,
                scope: milestones.iter().map(|milestone| milestone.scope).sum(),
                completed: milestones.iter().map(|milestone| milestone.completed).sum(),
                milestones,
            }
        })
        .collect();
    Ok(Initiatives {
        space_id: Some(space.to_string()),
        collection_id: Some(collection.id.to_string()),
        initiatives,
    })
}

/// Display names of the people the records' person fields name.
async fn person_names(
    connection: &mut PgConnection,
    space: Uuid,
    definitions: &[fields::FieldDefinition],
    records: &[RecordRow],
) -> Result<HashMap<String, String>, AppError> {
    let people: Vec<Uuid> = definitions
        .iter()
        .filter(|field| {
            matches!(
                field.field_type,
                fields::FieldType::Person
                    | fields::FieldType::CreatedBy
                    | fields::FieldType::EditedBy
            )
        })
        .flat_map(|field| {
            records.iter().filter_map(move |record| {
                record
                    .field_values
                    .get(field.id.to_string())
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
            })
        })
        .collect();
    if people.is_empty() {
        return Ok(HashMap::new());
    }
    // Everyone in the workspace is in its space, under the name they use.
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        "SELECT actor_id, display_name FROM project_memberships \
         WHERE project_id = $1 AND actor_id = ANY($2)",
    )
    .bind(space)
    .bind(&people)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, name)| (id.to_string(), name))
    .collect())
}
