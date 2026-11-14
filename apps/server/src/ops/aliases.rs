//! Old tool names (K2 section 3.6). When a module's slice gives a tool its
//! canonical name, the old name stays callable as an alias: on MCP and the
//! public API v1, with its old arguments and its old result shape, through
//! an adapter. Each call is counted per workspace and day in
//! `operation_alias_usage`, so an alias is removed only once nobody uses
//! it (at least 180 days after the canonical operation ships, and 30 days
//! with no calls). MCP answers carry `_meta.deprecated`; the public API's
//! v1 route answers with `Deprecation` and, once a date is set, `Sunset`.
//! An answer that succeeds is the old tool's value alone, as it was; a
//! refusal keeps the operation's envelope.
//!
//! The rename table is data. A row is inert until its canonical operation
//! exists; the slice that adds the operation removes the hand-written tool
//! of the old name in the same change (a test checks they never coexist).

use std::sync::{OnceLock, RwLock};

use rmcp::model::JsonObject;
use serde_json::Value;

use super::registry;

/// An old name for an operation.
#[derive(Clone, Copy, Debug)]
pub struct Alias {
    /// The old tool name.
    pub name: &'static str,
    /// The operation it runs, chosen from the old arguments (`page_write`
    /// is a create without `pageId` and an update with it).
    pub canonical: fn(&JsonObject) -> &'static str,
    /// The canonical operations it may run, for checks and docs.
    pub runs: &'static [&'static str],
    /// The old arguments as the canonical operation's input.
    pub adapt_in: fn(JsonObject) -> JsonObject,
    /// The canonical operation's value in the old result shape.
    pub adapt_out: fn(&str, Value) -> Value,
    /// When it stops being served, once set (a `YYYY-MM-DD` date).
    pub sunset: Option<&'static str>,
}

fn same_in(arguments: JsonObject) -> JsonObject {
    arguments
}

fn same_out(_canonical: &str, value: Value) -> Value {
    value
}

/// Tools that already answered in the `{outcome, value}` envelope (A1.3's
/// run tools) keep it.
fn enveloped(_canonical: &str, value: Value) -> Value {
    serde_json::json!({ "outcome": "ok", "value": value })
}

/// A run tool renamed when it became an operation, answering as it did.
macro_rules! run_renamed {
    ($old:literal => $new:literal) => {
        Alias {
            name: $old,
            canonical: |_| $new,
            runs: &[$new],
            adapt_in: same_in,
            adapt_out: enveloped,
            sunset: None,
        }
    };
}

macro_rules! renamed {
    ($old:literal => $new:literal) => {
        Alias {
            name: $old,
            canonical: |_| $new,
            runs: &[$new],
            adapt_in: same_in,
            adapt_out: same_out,
            sunset: None,
        }
    };
}

/// `page_write` and the other `_write` tools: a create without the thing's
/// id, an update with it.
macro_rules! written {
    ($old:literal, $id:literal => $create:literal, $update:literal) => {
        Alias {
            name: $old,
            canonical: |arguments| {
                if arguments.contains_key($id) {
                    $update
                } else {
                    $create
                }
            },
            runs: &[$create, $update],
            adapt_in: same_in,
            adapt_out: same_out,
            sunset: None,
        }
    };
}

/// The rename plan of K2 section 3.6. Adapters beyond the identity land
/// with each row's canonical operation, in its module's slice.
pub const ALIASES: &[Alias] = &[
    renamed!("projects_list" => "project_list"),
    renamed!("project_open" => "project_get"),
    renamed!("task_open" => "task_get"),
    renamed!("conversation_open" => "conversation_get"),
    renamed!("thing_open" => "thing_get"),
    written!("page_write", "pageId" => "page_create", "page_update"),
    renamed!("page_edit" => "page_update_blocks"),
    written!("sheet_write", "sheetId" => "sheet_create", "sheet_update"),
    written!("collection_write", "collectionId" => "collection_create", "collection_update"),
    written!("view_write", "viewId" => "view_create", "view_update"),
    written!("label_write", "labelId" => "label_create", "label_update"),
    written!("field_write", "fieldId" => "field_create", "field_update"),
    written!("milestone_write", "milestoneId" => "milestone_create", "milestone_update"),
    written!("template_write", "templateId" => "template_create", "template_update"),
    written!("automation_write", "ruleId" => "automation_create", "automation_update"),
    written!("webhook_write", "webhookId" => "webhook_create", "webhook_update"),
    written!("environment_write", "environmentId" => "environment_create", "environment_update"),
    written!("bug_report_link_write", "linkId" => "bug_report_link_create", "bug_report_link_update"),
    renamed!("whiteboard_edit" => "whiteboard_update_elements"),
    renamed!("message_edit" => "message_update"),
    renamed!("task_comment_edit" => "task_comment_update"),
    renamed!("task_criteria" => "task_criteria_update"),
    renamed!("task_watch" => "task_watch_set"),
    renamed!("task_link_pull_request" => "task_link"),
    renamed!("mark_read" => "conversation_mark_read"),
    renamed!("briefing" => "briefing_get"),
    renamed!("settings_set" => "setting_update"),
    renamed!("project_member_set" => "member_update"),
    renamed!("project_layout_set" => "layout_update"),
    renamed!("ai_summarize" => "conversation_summarize"),
    renamed!("ai_analyze" => "thing_analyze"),
    run_renamed!("run_open" => "run_create"),
    run_renamed!("run_activity" => "run_activity_report"),
    run_renamed!("run_result" => "run_result_report"),
];

/// Aliases added by tests outside the crate.
fn extras() -> &'static RwLock<Vec<Alias>> {
    static EXTRAS: OnceLock<RwLock<Vec<Alias>>> = OnceLock::new();
    EXTRAS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Adds an alias for the tests (integration tests can't extend the table).
pub fn register_alias_for_tests(alias: Alias) {
    if let Ok(mut extras) = extras().write() {
        extras.retain(|existing| existing.name != alias.name);
        extras.push(alias);
    }
}

/// The alias of this name whose canonical operations exist, if any.
pub fn active(name: &str) -> Option<Alias> {
    let registry = registry();
    let live =
        |alias: &Alias| alias.name == name && alias.runs.iter().all(|runs| registry.contains(runs));
    ALIASES.iter().copied().find(live).or_else(|| {
        extras()
            .read()
            .ok()
            .and_then(|extras| extras.iter().copied().find(live))
    })
}

impl Alias {
    /// The `Sunset` header's value (RFC 8594: an HTTP date), once set.
    pub fn sunset_header(&self) -> Option<String> {
        let date = chrono::NaiveDate::parse_from_str(self.sunset?, "%Y-%m-%d").ok()?;
        Some(date.format("%a, %d %b %Y 00:00:00 GMT").to_string())
    }
}

/// What an MCP answer through an alias says about it.
pub fn deprecation(alias: &Alias, canonical: &str) -> String {
    match alias.sunset {
        Some(date) => format!("use {canonical}; {} stops working after {date}", alias.name),
        None => format!("use {canonical}"),
    }
}

/// Counts one call through an alias (`operation_alias_usage`).
pub(super) async fn count(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    workspace_id: uuid::Uuid,
    alias: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO operation_alias_usage (workspace_id, alias, day, calls)
        VALUES ($1, $2, current_date, 1)
        ON CONFLICT (workspace_id, alias, day)
        DO UPDATE SET calls = operation_alias_usage.calls + 1
        "#,
    )
    .bind(workspace_id)
    .bind(alias)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rename_table_is_well_formed() {
        let mut names = std::collections::BTreeSet::new();
        for alias in ALIASES {
            assert!(names.insert(alias.name), "{} twice", alias.name);
            assert!(!alias.runs.is_empty(), "{}", alias.name);
            for canonical in alias.runs {
                assert_ne!(*canonical, alias.name, "{} renames to itself", alias.name);
                super::super::conventions::check_name(canonical, None)
                    .unwrap_or_else(|problem| panic!("{}: {problem}", alias.name));
            }
            // The adapters' pick is one of the operations it runs.
            let picked = (alias.canonical)(&JsonObject::new());
            assert!(alias.runs.contains(&picked), "{}", alias.name);
        }
    }

    #[test]
    fn an_active_alias_has_no_hand_written_tool() {
        for alias in ALIASES {
            if active(alias.name).is_some() {
                assert!(
                    !crate::mcp::has_legacy_tool(alias.name),
                    "{} is an alias of {:?} and still a hand-written tool: remove the tool",
                    alias.name,
                    alias.runs
                );
            }
        }
    }

    #[test]
    fn a_write_alias_picks_create_or_update() {
        let page_write = ALIASES
            .iter()
            .find(|alias| alias.name == "page_write")
            .expect("page_write");
        let mut arguments = JsonObject::new();
        assert_eq!((page_write.canonical)(&arguments), "page_create");
        arguments.insert("pageId".into(), Value::String("p".into()));
        assert_eq!((page_write.canonical)(&arguments), "page_update");
    }

    #[test]
    fn an_alias_answers_in_its_old_shape() {
        // A test alias with real adapters, as a module's slice writes them:
        // the old name took `name` and answered `{ thing: { name } }`.
        let alias = Alias {
            name: "thing_write",
            canonical: |_| "thing_create",
            runs: &["thing_create"],
            adapt_in: |mut arguments| {
                if let Some(name) = arguments.remove("name") {
                    arguments.insert("title".into(), name);
                }
                arguments
            },
            adapt_out: |_, value| serde_json::json!({ "thing": { "name": value["title"] } }),
            sunset: Some("2027-06-01"),
        };
        let mut old = JsonObject::new();
        old.insert("name".into(), Value::String("Kept".into()));
        let input = (alias.adapt_in)(old);
        assert_eq!(Value::Object(input), serde_json::json!({ "title": "Kept" }));
        let answered = (alias.adapt_out)(
            "thing_create",
            serde_json::json!({ "id": "t-1", "title": "Kept" }),
        );
        assert_eq!(
            serde_json::to_string(&answered).expect("JSON"),
            r#"{"thing":{"name":"Kept"}}"#
        );
        assert_eq!(
            deprecation(&alias, "thing_create"),
            "use thing_create; thing_write stops working after 2027-06-01"
        );
        assert_eq!(
            alias.sunset_header().as_deref(),
            Some("Tue, 01 Jun 2027 00:00:00 GMT")
        );
    }
}
