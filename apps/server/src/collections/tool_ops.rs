//! Collections and their records as operations (K2.11): listing, opening,
//! making and changing a collection, and reading and writing its records in
//! batches. Each runs the command its hand-written tool ran
//! (`ops::wrapped`) as the caller the dispatcher checked. `collection_describe`
//! became `collection_get`, `collection_write` became `collection_create` and
//! `collection_update`, `record_query` became `record_list` and
//! `record_write` became `record_create_many` and `record_update_many`; the
//! old names are retired (`ops::retired`).

use crate::{
    mcp::collection_tools,
    ops::{
        OpError, Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{keyed, names_existing, run_tool, tool_op},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<CollectionList>()
        .add::<CollectionGet>()
        .add::<CollectionCreate>()
        .add::<CollectionUpdate>()
        .add::<RecordList>()
        .add::<RecordCreateMany>()
        .add::<RecordUpdateMany>();
}

/// A collection is named by its name or id inside its space, so each of
/// these acts on the space.
fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(uuid::Uuid::nil))
}

macro_rules! in_space {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    space(&self.project_id)
                }
            }
        )*
    };
}

in_space!(
    collection_tools::CollectionListArguments,
    collection_tools::CollectionDescribeArguments,
    collection_tools::CollectionWriteArguments,
    collection_tools::RecordQueryArguments,
    collection_tools::RecordWriteArguments,
);

tool_op!(CollectionList {
    name: "collection_list",
    module: "collections",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List collections",
    description: "List a space's collections: record types it keeps, such as Bugs or \
        Playtesters, each with its description, title column, record count and version. \
        collection_get opens one's fields and views; record_list reads its records.",
    emits: [],
    input: collection_tools::CollectionListArguments,
    output: collection_tools::CollectionListValue,
    run: |call, input| { run_tool!(call, collection_tools::collection_list, input) }
});

tool_op!(CollectionGet {
    name: "collection_get",
    module: "collections",
    kind: Describe,
    permission: None,
    idempotency: None,
    title: "Open collection",
    description: "Open a collection (by name or ID): its fields with their types and options, \
        its saved views, how many records are archived, and its definition, which \
        collection_create takes as it is to make the same collection elsewhere. To show a view \
        live in a page, put `[name](/projects/{projectId}/collections/{collectionId}?view={viewId} \
        \"embed\")` on a line of its own.",
    emits: [],
    input: collection_tools::CollectionDescribeArguments,
    output: collection_tools::CollectionDescribeValue,
    run: |call, input| { run_tool!(call, collection_tools::collection_describe, input) }
});

macro_rules! field_types {
    () => {
        "Field types: text, long_text, number, date, single_choice, \
    multiple_choice, status (options with a category: backlog, unstarted, started, review, \
    verification, completed, canceled), person, url, email, phone, checkbox, link (target task, \
    page, or record:<collection name>), file (file_upload to the record, then name the \
    fileIds), and the record's own created_time, edited_time, created_by, edited_by and auto_id, \
    which nobody writes. A field's type never changes."
    };
}

tool_op!(CollectionCreate {
    name: "collection_create",
    module: "collections",
    kind: Create,
    permission: Some(crate::ops::Perm::CollectionsBuild),
    idempotency: Command,
    audited: true,
    guard_kind: "collection",
    title: "Create collection",
    description: concat!(
        "Make a collection with its fields and views in one call (name, fields: [{name, type, \
         options}], views: [{name, filters, sort, groupBy, columns}]). ",
        field_types!()
    ),
    emits: ["collection.updated"],
    input: collection_tools::CollectionWriteArguments,
    output: collection_tools::CollectionWriteValue,
    run: |call, input| {
        names_existing(input.collection.is_some(), true, "collection")?;
        keyed!(call, collection_tools::collection_write, input)
    }
});

tool_op!(CollectionUpdate {
    name: "collection_update",
    module: "collections",
    kind: Change,
    permission: Some(crate::ops::Perm::CollectionsBuild),
    idempotency: Command,
    audited: true,
    guard_kind: "collection",
    title: "Update collection",
    description: concat!(
        "Change a collection (by name or ID). Fields and views are matched by name, so the same \
         definition again changes nothing; pass field or view to rename one, remove: true to \
         remove a view, archived: true to archive a field or the collection (nothing is \
         deleted) and false to restore it. ",
        field_types!()
    ),
    emits: ["collection.updated"],
    input: collection_tools::CollectionWriteArguments,
    output: collection_tools::CollectionWriteValue,
    run: |call, input| {
        names_existing(input.collection.is_some(), false, "collection")?;
        keyed!(call, collection_tools::collection_write, input)
    }
});

tool_op!(RecordList {
    name: "record_list",
    module: "collections",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List records",
    description: "Find a collection's records, a page at a time: filter on field values \
        (filters: [{field, op, values}], op is, isNot, empty, notEmpty, contains, gt, gte, lt \
        or lte; \"me\" for yourself in a person field), search words, sort by up to 3 fields, \
        group by a choice, person, checkbox or date field to count each group, or start from a \
        saved view. Name records by number (#12) or ID to read just those, with their history \
        if asked. Returns each record's values by field name, the total and nextCursor.",
    emits: [],
    input: collection_tools::RecordQueryArguments,
    output: collection_tools::RecordQueryValue,
    run: |call, input| { run_tool!(call, collection_tools::record_query, input) }
});

macro_rules! record_values {
    () => {
        "Set its title and its values by field name (null clears one): \
    people by @handle or \"me\", tasks by key (AWD-12), pages by ID or link, dates as \
    YYYY-MM-DD, choices by option name. archived: true archives a record (nothing is deleted) \
    and false restores it. If any record is refused, none is written."
    };
}

tool_op!(RecordCreateMany {
    name: "record_create_many",
    module: "collections",
    kind: Create,
    permission: Some(crate::ops::Perm::CollectionsEdit),
    idempotency: Command,
    guard_kind: "record",
    title: "Add records",
    description: concat!(
        "Add up to 100 records to a collection, all or nothing (leave record out of each). ",
        record_values!()
    ),
    emits: ["record.created"],
    input: collection_tools::RecordWriteArguments,
    output: collection_tools::RecordWriteValue,
    run: |call, input| {
        if input.records.iter().any(|entry| entry.record.is_some()) {
            return Err(OpError::invalid(
                "records",
                "This adds records: leave record out of each, or use record_update_many.",
            ));
        }
        keyed!(call, collection_tools::record_write, input)
    }
});

tool_op!(RecordUpdateMany {
    name: "record_update_many",
    module: "collections",
    kind: Change,
    permission: Some(crate::ops::Perm::CollectionsEdit),
    idempotency: Command,
    guard_kind: "record",
    title: "Update records",
    description: concat!(
        "Change up to 100 of a collection's records, all or nothing: name each by number (#12) \
         or ID, with the version you read as expectedVersion if it matters. An entry without \
         record is added in the same batch. ",
        record_values!()
    ),
    emits: ["record.created", "record.updated"],
    input: collection_tools::RecordWriteArguments,
    output: collection_tools::RecordWriteValue,
    run: |call, input| {
        if input.records.iter().all(|entry| entry.record.is_none()) {
            return Err(OpError::invalid(
                "records",
                "Name the records to change (number or ID), or use record_create_many.",
            ));
        }
        keyed!(call, collection_tools::record_write, input)
    }
});
