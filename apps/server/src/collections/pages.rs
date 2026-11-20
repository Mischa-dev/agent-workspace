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
     name and settings don't change. Add fields to it with collection_write, and set a page's \
     fields with record_write (the record's ID is the page's).";

/// What to say when a write asks to add, retitle or archive one of its
/// records.
pub const RECORDS_MESSAGE: &str = "These records are this space's pages: make, rename and archive \
     pages on Pages (page_write). Here you set their fields.";
