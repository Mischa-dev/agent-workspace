//! Pages as a built-in collection (T1.36). A space's pages are the records
//! of its built-in Pages collection, each with its page's ID (migration
//! 20261119005917), so a page's fields are that record's values and any
//! view of the collection lists, filters, sorts and groups pages.
//!
//! The page owns its title and archiving: database triggers copy them onto
//! the record and refuse any other write to them. Records here are never
//! added or removed by hand; only their field values change, through the
//! usual record writes (`records::write` refuses the rest first, in words).
//! The collection is made with the space's first page.

/// `collections.builtin` for the Pages collection.
pub const BUILTIN: &str = "pages";

/// What to say when a write asks to change the Pages collection itself.
pub const READ_ONLY_MESSAGE: &str = "Pages is the built-in collection of this space's pages: its \
     name and settings don't change. Add fields to it with collection_update, and set a page's \
     fields with record_update_many (the record's ID is the page's).";

/// What to say when a write asks to add, retitle or archive one of its
/// records.
pub const RECORDS_MESSAGE: &str = "These records are this space's pages: make, rename and archive \
     pages on Pages (page_write). Here you set their fields.";

/// After a page's title or archiving changed (its record follows by
/// trigger): the record's worked-out values, the records whose rollups
/// read it, and the event that names it. A page with no record (a
/// conversation's memory) has nothing to follow.
pub async fn page_record_changed(
    connection: &mut sqlx::PgConnection,
    project_id: uuid::Uuid,
    page_id: uuid::Uuid,
) -> Result<(), crate::error::AppError> {
    let Some(collection_id) = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT collection_id FROM collection_records WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(page_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(());
    };
    let records = [page_id];
    super::computed::refresh_records(connection, project_id, collection_id, &records).await?;
    super::computed::mark_dependents(connection, project_id, collection_id, &records, 1).await?;
    crate::outbox::enqueue_records_changed(connection, project_id, collection_id, &records, "page")
        .await?;
    Ok(())
}
