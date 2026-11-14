//! The shared contract every module builds on (principle 1 of the build-out
//! plan, "Everything works with everything"), first version. A module's
//! manifest speaks these words, and so do the tools and screens that work
//! across modules:
//!
//! - **Things.** Everything a module makes is a [`Thing`]: an id, a kind, a
//!   title, a status and an owner, with a link to open it. A manifest
//!   declares its kinds ([`ThingDecl`]) and the fields they carry.
//! - **Links.** Any thing can link to any other with a [`LinkKind`]; links
//!   read both ways ([`LinkDirection`]), which is what "Referenced in"
//!   (AWD-58) draws.
//! - **Embeds.** A module offers live blocks for pages by declaring them
//!   ([`EmbedDecl`]), never by changing the page editor. Docs (AWD-53) reads
//!   these declarations.
//! - **Fields and views.** One set of field types ([`FieldType`]) and view
//!   types ([`ViewType`]) for tasks, collections and modules' records. Task
//!   fields (AWD-89) and collections (AWD-90) fill these in.
//! - **Pieces.** A module describes what it shows in pieces the app draws
//!   ([`Piece`]): a list, a table, a board, a detail view, fields, a form, a
//!   card or actions. The web kit draws each one at desktop and phone widths
//!   (principle 5).
//! - **Events.** Every change is an event named `<thing>.<what happened>`,
//!   such as `task.created`; a manifest says which it emits and which it
//!   listens to, for automations, webhooks, notifications and search.
//! - **Agent tools.** Each tool does one of five things ([`ToolVerb`]):
//!   describe, query, create, change or suggest. An agent that knows one
//!   module's tools can use any.
//!
//! `docs/plans/modules.md` explains the contract and what comes next.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a tool does, in the one pattern every module's tools follow.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolVerb {
    /// Open one thing (or the module itself) with what it links to.
    Describe,
    /// Find or list things.
    Query,
    /// Make a new thing.
    Create,
    /// Change, move, link or remove a thing.
    Change,
    /// Propose a change for a person to accept, such as a model's
    /// suggestion or an agent's edit to someone's page.
    Suggest,
}

/// How one thing relates to another. Every link reads both ways: a task
/// that `blocks` another is, from the other side, blocked by it.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A subtask of a task, a task in a milestone, a record in a project.
    PartOf,
    /// Has to be done before the other can be.
    Blocks,
    /// Is about, or discusses, the other: a thread about a task, a
    /// whiteboard sketching a page.
    About,
    /// Was made from the other: tasks from a plan page.
    MadeFrom,
    RelatesTo,
    Duplicates,
}

impl LinkKind {
    pub const ALL: [Self; 6] = [
        Self::PartOf,
        Self::Blocks,
        Self::About,
        Self::MadeFrom,
        Self::RelatesTo,
        Self::Duplicates,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::PartOf => "part_of",
            Self::Blocks => "blocks",
            Self::About => "about",
            Self::MadeFrom => "made_from",
            Self::RelatesTo => "relates_to",
            Self::Duplicates => "duplicates",
        }
    }

    /// The words for the link from each side: (outgoing, incoming).
    pub fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::PartOf => ("Part of", "Contains"),
            Self::Blocks => ("Blocks", "Blocked by"),
            Self::About => ("About", "Referenced in"),
            Self::MadeFrom => ("Made from", "Made into"),
            Self::RelatesTo => ("Related to", "Related to"),
            Self::Duplicates => ("Duplicates", "Duplicated by"),
        }
    }
}

/// Which side of a link a thing is on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// This thing links to the other.
    Outgoing,
    /// The other thing links to this one.
    Incoming,
}

/// A field's type, shared by tasks, collections and every module's records:
/// the one list in `crate::fields` (T1.2), so a manifest, a task field and a
/// collection's field name types with the same words.
pub use crate::fields::FieldType;

/// A way to look at many things at once: the one list in `crate::views`
/// (T1.2).
pub use crate::views::ViewKind as ViewType;

/// A piece of screen the app draws for a module. Modules never draw their
/// own layout: they say which piece, of which kind of thing, with which
/// fields, and the kit draws it for every width.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PieceType {
    /// Rows, optionally grouped by a field.
    List,
    /// Columns of fields; a list of cards on a phone.
    Table,
    /// Cards in a column for each value of a field; scrolls sideways on a phone.
    Board,
    /// One thing: its title, status, owner and fields, with actions.
    Detail,
    /// A thing's fields as labelled values.
    Fields,
    /// Fields to fill in, with a button.
    Form,
    /// One thing, small, for Home, embeds and links.
    Card,
    /// Buttons; a menu on a phone.
    Actions,
}

/// A piece, as a manifest declares it.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Piece {
    #[serde(rename = "type")]
    pub kind: PieceType,
    /// The kind of thing it shows, declared by some module's `things`.
    pub thing: String,
    /// Fields to show, in order: the shared ones (`status`, `owner`,
    /// `title`) or ones the thing declares.
    #[serde(default)]
    pub fields: Vec<String>,
    /// For lists and boards: the field that groups them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// Actions it offers, such as `open`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

/// A field a kind of thing carries beyond the shared ones.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDecl {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    /// For choice fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// How a value reads as a chip (labels and choices), by value; the
    /// rest read muted (AWD-98).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tones: std::collections::BTreeMap<String, ChipTone>,
    /// For long text written in Markdown, such as why a decision was made:
    /// drawn in full rather than as a two-line summary (AWD-116).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub markdown: bool,
}

/// The tones a chip comes in, as the app draws them.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChipTone {
    Muted,
    Agent,
    Warning,
    Success,
    Danger,
}

/// A kind of thing a module makes.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingDecl {
    /// Unique across modules: `task`, `page`, `whiteboard`.
    pub kind: String,
    pub one: String,
    pub many: String,
    /// Where it opens in the app, with `{id}`, `{projectId}` or `{url}`.
    pub href: String,
    #[serde(default)]
    pub fields: Vec<FieldDecl>,
}

/// A kind of link a module makes between two kinds of thing.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkDecl {
    pub kind: LinkKind,
    pub from: String,
    pub to: String,
}

/// A live block a module offers for pages, and for Home and other slots.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbedDecl {
    /// `<module>.<name>`.
    pub id: String,
    pub label: String,
    pub description: String,
    pub thing: String,
    pub piece: Piece,
}

/// The events a module sends and the ones it follows.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventDecl {
    #[serde(default)]
    pub emits: Vec<String>,
    #[serde(default)]
    pub listens: Vec<String>,
}

/// One thing, as any module, tool or screen sees it.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thing {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: String,
    /// The module that makes it.
    pub module: String,
    pub title: String,
    /// In the thing's own words (a task's status name, a run's state, a
    /// thread's status); none for kinds without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ThingOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Uuid>,
    /// Where it opens in the app.
    pub href: String,
}

/// Who a thing belongs to: a task's assignee, a page's last editor, a run's
/// starter.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThingOwner {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
}

/// A link, read from one thing's side.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThingLink {
    pub kind: LinkKind,
    pub direction: LinkDirection,
    /// The link in words from this side, such as "Blocked by".
    pub label: String,
    pub other: Thing,
}

#[cfg(test)]
mod tests {
    use super::{FieldType, ViewType};

    /// One vocabulary (T1.2): the contract's field and view types are the
    /// field layer's and the views engine's own, not copies to keep in step.
    #[test]
    fn contract_and_fields_are_the_same_types() {
        let same_field: fn(crate::fields::FieldType) -> FieldType = |kind| kind;
        let same_view: fn(crate::views::ViewKind) -> ViewType = |kind| kind;
        assert_eq!(same_field(FieldType::Text), crate::fields::FieldType::Text);
        assert_eq!(same_view(ViewType::Board), crate::views::ViewKind::Board);
        for (kind, stored) in [
            (FieldType::Text, "text"),
            (FieldType::LongText, "long_text"),
            (FieldType::SingleChoice, "single_choice"),
            (FieldType::People, "people"),
            (FieldType::Labels, "labels"),
            (FieldType::Link, "link"),
        ] {
            assert_eq!(
                serde_json::to_value(kind).expect("name"),
                serde_json::json!(stored)
            );
        }
    }

    /// Exactly one definition of each type in the server's sources.
    #[test]
    fn one_field_type_and_one_view_kind_are_defined() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        fn walk(folder: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    found.push(path);
                }
            }
        }
        walk(&root, &mut files);
        let defined = |needle: &str| {
            files
                .iter()
                .filter(|file| {
                    std::fs::read_to_string(file).is_ok_and(|text| {
                        text.lines().any(|line| {
                            line.trim_start().starts_with(needle)
                                || line.contains(&format!("pub {needle}"))
                        })
                    })
                })
                .count()
        };
        assert_eq!(defined("enum FieldType "), 1, "one `enum FieldType`");
        assert_eq!(defined("enum ViewKind "), 1, "one `enum ViewKind`");
        assert_eq!(
            defined("enum ViewType "),
            0,
            "views are a `crate::views::ViewKind`"
        );
    }
}
