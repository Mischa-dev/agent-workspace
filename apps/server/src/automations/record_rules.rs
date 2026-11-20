//! Record conditions for automation rules (T1.38): `record_matches` and
//! `field_changed` name a collection, and narrow its records by one of its
//! views and field filters, read through the views engine as the rule's
//! maker reads them. A rule stores its fields by ID and its values as
//! stored, so renaming a field or an option doesn't change what it means.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::Trigger;
use crate::{
    collections::{self, Collection, definition::FilterWrite},
    error::AppError,
    events::Event,
    fields::{self, FieldClause, FieldOp},
    views::{
        self,
        definition::ViewConfig,
        filter::Filter,
        query::{self, Spec},
        source::CollectionSource,
    },
    work_item_fields::Writer,
};

/// What a record trigger watches, borrowed from it.
struct Watch<'a> {
    collection_id: Uuid,
    view_id: Option<Uuid>,
    filters: &'a [FilterWrite],
    /// field_changed's field (stored: its ID) and the values it changes to.
    field: Option<(&'a str, &'a [String])>,
}

fn watch(trigger: &Trigger) -> Option<Watch<'_>> {
    match trigger {
        Trigger::RecordMatches {
            collection_id,
            view_id,
            filters,
        } => Some(Watch {
            collection_id: *collection_id,
            view_id: *view_id,
            filters,
            field: None,
        }),
        Trigger::FieldChanged {
            collection_id,
            field,
            to,
            view_id,
            filters,
        } => Some(Watch {
            collection_id: *collection_id,
            view_id: *view_id,
            filters,
            field: Some((field, to)),
        }),
        _ => None,
    }
}

/// The collection a record rule may watch: in the space, not archived, and
/// not one whose records change as tasks (task triggers follow those).
async fn watched(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<Option<Collection>, AppError> {
    Ok(
        collections::get(&mut *connection, project_id, collection_id)
            .await?
            .filter(|collection| !collection.archived && !collection.acts_like_task),
    )
}

/// A view of the collection the maker may use: shared, or their own.
async fn usable_view(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    view_id: Uuid,
    maker: Uuid,
) -> Result<Option<views::definition::View>, AppError> {
    Ok(
        views::definition::one(connection, project_id, collection_id, view_id)
            .await?
            .filter(|view| view.owner_actor_id.is_none_or(|owner| owner == maker)),
    )
}

async fn project_key(connection: &mut PgConnection, project_id: Uuid) -> Result<String, AppError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// Clauses as a rule stores them: the field by ID, the values as stored.
fn stored(clauses: Vec<FieldClause>) -> Vec<FilterWrite> {
    clauses
        .into_iter()
        .map(|clause| FilterWrite {
            field: clause.field.to_string(),
            op: clause.op,
            values: clause.values,
        })
        .collect()
}

/// Checks a record trigger against the space and stores it by IDs: the
/// collection, the view, the field and the values. Other triggers pass.
pub(super) async fn resolve(
    connection: &mut PgConnection,
    project_id: Uuid,
    maker: Uuid,
    trigger: &mut Trigger,
    fresh_collections: &[Uuid],
) -> Result<(), AppError> {
    let Some(found) = watch(trigger) else {
        return Ok(());
    };
    let collection_id = found.collection_id;
    let view_id = found.view_id;
    if fresh_collections.contains(&collection_id) {
        // Made in the caller's transaction (a package): nothing of it can
        // be named yet, which `prepare_in` refuses.
        return Ok(());
    }
    let Some(collection) = watched(&mut *connection, project_id, collection_id).await? else {
        return Err(AppError::NotFound("collection"));
    };
    let definitions =
        collections::view_fields(&mut *connection, project_id, collection_id, false).await?;
    if let Some(view_id) = view_id
        && usable_view(&mut *connection, project_id, collection_id, view_id, maker)
            .await?
            .is_none()
    {
        return Err(AppError::NotFound("view"));
    }
    let key = project_key(&mut *connection, project_id).await?;
    let writer = Writer {
        actor_id: maker,
        project_key: &key,
    };
    let (filters, field, to) = match trigger {
        Trigger::RecordMatches { filters, .. } => (filters, None, None),
        Trigger::FieldChanged {
            filters, field, to, ..
        } => (filters, Some(field), Some(to)),
        _ => return Ok(()),
    };
    if !filters.is_empty() {
        let clauses = collections::definition::clauses_from_written(
            &mut *connection,
            project_id,
            writer,
            &collection,
            &definitions,
            filters,
        )
        .await?
        .map_err(|_| AppError::Validation("trigger.filters"))?;
        *filters = stored(clauses);
    }
    if let (Some(field), Some(to)) = (field, to) {
        let Some(definition) = fields::find(&definitions, field).filter(|found| !found.archived)
        else {
            return Err(AppError::Validation("trigger.field"));
        };
        *field = definition.id.to_string();
        if !to.is_empty() {
            let written = [FilterWrite {
                field: field.clone(),
                op: FieldOp::Is,
                values: std::mem::take(to),
            }];
            let clauses = collections::definition::clauses_from_written(
                &mut *connection,
                project_id,
                writer,
                &collection,
                &definitions,
                &written,
            )
            .await?
            .map_err(|_| AppError::Validation("trigger.to"))?;
            *to = clauses
                .into_iter()
                .next()
                .map(|clause| clause.values)
                .unwrap_or_default();
        }
    }
    Ok(())
}

/// Whether the event's record is one the rule fires for, as its maker reads
/// the collection now. Err says why the rule can't be read any more.
pub(super) async fn fires(
    pool: &PgPool,
    project_id: Uuid,
    maker: Uuid,
    trigger: &Trigger,
    event: &Event,
) -> Result<Result<bool, &'static str>, AppError> {
    let Some(found) = watch(trigger) else {
        return Ok(Ok(false));
    };
    let Some(record_id) = event.subject.record_id else {
        return Ok(Ok(false));
    };
    if event.subject.collection_id != Some(found.collection_id) {
        return Ok(Ok(false));
    }
    let mut connection = crate::db::scoped_tx(pool).await?;
    let Some(collection) = watched(&mut connection, project_id, found.collection_id).await? else {
        return Ok(Err(
            "Turned off because the collection it watches was archived, deleted or now acts like tasks.",
        ));
    };
    let definitions =
        collections::view_fields(&mut connection, project_id, found.collection_id, false).await?;
    let mut items = Vec::new();
    if let Some((field, to)) = found.field {
        let Some(definition) = fields::find(&definitions, field).filter(|found| !found.archived)
        else {
            return Ok(Err(
                "Turned off because the field it watches was archived or deleted.",
            ));
        };
        // The event names the fields that changed as they were called then.
        if event.event_type != "record.updated"
            || !event
                .subject
                .changed_fields
                .iter()
                .any(|name| name == &definition.name)
        {
            return Ok(Ok(false));
        }
        if !to.is_empty() {
            match fields::clause_from_written(definition, FieldOp::Is, to) {
                Ok(clause) => items.push(Filter::Clause(clause)),
                Err(_) => {
                    return Ok(Err(
                        "Turned off because the values it watches for no longer fit its field.",
                    ));
                }
            }
        }
    }
    if let Some(view_id) = found.view_id {
        let Some(view) = usable_view(
            &mut connection,
            project_id,
            found.collection_id,
            view_id,
            maker,
        )
        .await?
        else {
            return Ok(Err(
                "Turned off because the view its conditions use was deleted.",
            ));
        };
        items.push(view.config.active(&definitions).filters);
    }
    if !found.filters.is_empty() {
        let key = project_key(&mut connection, project_id).await?;
        let writer = Writer {
            actor_id: maker,
            project_key: &key,
        };
        match collections::definition::clauses_from_written(
            &mut connection,
            project_id,
            writer,
            &collection,
            &definitions,
            found.filters,
        )
        .await?
        {
            Ok(clauses) => items.push(Filter::all(clauses)),
            Err(_) => {
                return Ok(Err(
                    "Turned off because its filters no longer fit the collection's fields.",
                ));
            }
        }
    }
    connection.commit().await?;
    if items.is_empty() {
        return Ok(Ok(true));
    }
    let config = ViewConfig {
        filters: Filter::And(items),
        ..ViewConfig::default()
    };
    let mut spec = Spec::from_config(&config, &definitions, maker);
    spec.only = Some(vec![record_id]);
    let source = CollectionSource {
        project_id,
        collection_id: found.collection_id,
    };
    let mut connection = crate::db::scoped_tx(pool).await?;
    let total = query::total(&mut connection, &source, &spec).await?;
    connection.commit().await?;
    Ok(Ok(total > 0))
}
