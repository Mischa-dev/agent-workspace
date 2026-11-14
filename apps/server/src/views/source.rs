//! Where a view's records come from (T1.5): a `RecordSource` gives the
//! query compiler (`super::query`) the table its records are read from,
//! what narrows it to them, and the few things a source does its own way.
//! A field's value is found through the field's `Accessor`, so a source
//! whose records are tasks names the task's columns as fields and the
//! compiler needs nothing else from it.
//!
//! * [`CollectionSource`]: one collection's records.
//! * `collections::tasks::TasksSource`: the records of a collection whose
//!   records are tasks (the built-in Tasks collection, or one that acts
//!   like a task), each with its task beside it.

use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::fields::{FieldDefinition, GroupSql, SortKey};

pub trait RecordSource: Send + Sync {
    /// The FROM clause: the records aliased `record` (and their tasks
    /// `core`, for a source whose records are tasks).
    fn from_sql(&self) -> String;

    /// Narrows to the source's records: appends ` WHERE ...`.
    fn scope(&self, builder: &mut QueryBuilder<'_, Postgres>);

    /// What the reader may see beyond the source's scope, as ` AND ...`.
    /// Everyone who can open a source reads all its records today (its
    /// project is checked before the query); K1's access filter goes here.
    fn access(&self, _builder: &mut QueryBuilder<'_, Postgres>, _reader: Uuid) {}

    /// Grouping by a field the source names its own way (a task's status
    /// and priority). None: the field layer's (`fields::group_sql`).
    fn group_sql(&self, _field: &FieldDefinition) -> Option<GroupSql> {
        None
    }

    /// Sort keys only this source has, by name (a task's board `rank`).
    fn extra_sort(&self, _key: &str) -> Option<Vec<SortKey>> {
        None
    }

    /// What makes the source's cursors its own: a cursor made for another
    /// source is refused.
    fn identity(&self) -> String;
}

/// One collection's records.
#[derive(Clone, Copy, Debug)]
pub struct CollectionSource {
    pub project_id: Uuid,
    pub collection_id: Uuid,
}

impl RecordSource for CollectionSource {
    fn from_sql(&self) -> String {
        "collection_records record".to_owned()
    }

    fn scope(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        builder.push(" WHERE record.project_id = ");
        builder.push_bind(self.project_id);
        builder.push(" AND record.collection_id = ");
        builder.push_bind(self.collection_id);
    }

    fn identity(&self) -> String {
        format!("collection:{}:{}", self.project_id, self.collection_id)
    }
}
