//! The one views engine (T1): the ways to look at many records at once,
//! for tasks, every collection's records and, later, other lists.
//!
//! * [`ViewKind`]: the seven kinds of view, the one list the module
//!   contract, collections and the web app name kinds from (T1.2).
//! * [`definition`]: a view as stored (filters, sort, grouping, columns),
//!   checked against its source's fields on every write.
//! * [`filter`]: a view's filter, a tree of clauses joined by "and" and
//!   "or" (a plain list of clauses is the "and" of them).
//! * [`source`]: where a view's records come from (`RecordSource`), and
//!   one collection's records (`CollectionSource`).
//! * [`query`]: the one query compiler: a page of records in order with a
//!   keyset cursor, the total, and counts by group, for any source.
//!
//! Collections reach `definition` and `query` at their old paths
//! (`collections::views`, `collections::query`); tasks get the engine
//! through `collections::tasks::TasksSource`.

pub mod definition;
pub mod filter;
pub mod query;
pub mod source;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// A way to look at many records at once.
#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// Rows and columns, a field to a column.
    Table,
    /// A column for each value of the field it groups by; moving a card to
    /// another column changes that value.
    Board,
    /// Dense rows, keyboard first.
    List,
    /// A month, with each record on the day its date field holds.
    Calendar,
    /// Bars from a start date to an end date.
    Timeline,
    /// A card for each record, with the view's shown fields.
    Gallery,
    /// A chart of the records.
    Chart,
}

impl ViewKind {
    pub const ALL: [Self; 7] = [
        Self::Table,
        Self::Board,
        Self::List,
        Self::Calendar,
        Self::Timeline,
        Self::Gallery,
        Self::Chart,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Board => "board",
            Self::List => "list",
            Self::Calendar => "calendar",
            Self::Timeline => "timeline",
            Self::Gallery => "gallery",
            Self::Chart => "chart",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::ViewKind;

    #[test]
    fn kinds_read_back_by_name() {
        for kind in ViewKind::ALL {
            assert_eq!(ViewKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).expect("name"),
                serde_json::json!(kind.as_str())
            );
        }
        assert_eq!(ViewKind::parse(" Board "), Some(ViewKind::Board));
        assert_eq!(ViewKind::parse("kanban"), None);
    }
}
