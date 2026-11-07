//! Before and after in task history (docs/plans/roadmap.md, T8).
//!
//! A change's activity payload keeps `changedFields` as before and adds
//! `changes`, keyed by field, with what the field was and what it became:
//! `{"priority": {"from": "high", "to": "urgent"}}`. Fields that name
//! something (assignee, type, milestone, parent) store its ID, like every
//! other payload, so a reader sees a name only when it looks it up.
//! Descriptions store a line diff instead of both texts, cut to fit the
//! payload's 8 KB limit.
//!
//! `label_changes` turns the IDs into names for the web timeline;
//! MCP history reads the same payload through its own lookups.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

/// Longest text kept on each side of a one-line change (title, blocker).
const MAX_TEXT_CHARACTERS: usize = 300;
/// Most bytes a description diff may take in a payload.
const DIFF_BUDGET_BYTES: usize = 4_500;
/// Lines of unchanged text kept around each change.
const CONTEXT_LINES: usize = 1;
/// Above this many cells the diff skips the full comparison and shows the
/// changed middle as removed and added.
const MAX_DIFF_CELLS: usize = 2_000_000;

/// The before and after of every field a change touched, in the order they
/// were recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldChanges {
    fields: Map<String, Value>,
    /// Bytes custom field changes have taken, against CUSTOM_BUDGET_BYTES.
    custom_bytes: usize,
}

/// Most bytes custom field changes may take in one payload, beside a
/// description diff and the one-line fields, under its 8 KB limit.
const CUSTOM_BUDGET_BYTES: usize = 1_800;
/// Longest text kept on each side of a custom field change.
const MAX_CUSTOM_CHARACTERS: usize = 80;

impl FieldChanges {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn into_map(self) -> Map<String, Value> {
        self.fields
    }

    /// Records a change between two JSON values, if they differ.
    pub fn value(&mut self, field: &str, from: Value, to: Value) {
        if from != to {
            self.fields
                .insert(field.to_owned(), json!({ "from": from, "to": to }));
        }
    }

    /// A one-line text field, cut to a readable length on each side.
    pub fn text(&mut self, field: &str, from: Option<&str>, to: Option<&str>) {
        if from == to {
            return;
        }
        let cut = |text: Option<&str>| text.map(|text| Value::String(cut_text(text)));
        self.value(
            field,
            cut(from).unwrap_or(Value::Null),
            cut(to).unwrap_or(Value::Null),
        );
    }

    /// A custom field (work_item_fields), keyed `field:<id>`, with how the
    /// value read before and after and the field's name. `label_changes`
    /// gives the field's current name.
    pub fn custom_field(&mut self, field_id: Uuid, name: &str, from: Value, to: Value) {
        if from == to {
            return;
        }
        let cut = |value: Value| match value {
            Value::String(text) => Value::String(cut_to(&text, MAX_CUSTOM_CHARACTERS)),
            Value::Array(items) => Value::String(cut_to(
                &items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                MAX_CUSTOM_CHARACTERS,
            )),
            other => other,
        };
        let key = format!("{CUSTOM_FIELD_PREFIX}{field_id}");
        // Past the budget a change still says which field changed, if that
        // fits; past that it's left out, and `changedFields` still says
        // "fields". The whole entry counts, key and all, so the budget holds
        // however many fields change together.
        let full = json!({ "field": name, "from": cut(from), "to": cut(to) });
        let named = json!({ "field": name, "from": null, "to": "…" });
        for change in [full, named] {
            let size = custom_entry_bytes(&key, &change);
            if self.custom_bytes + size <= CUSTOM_BUDGET_BYTES {
                self.custom_bytes += size;
                self.fields.insert(key, change);
                return;
            }
        }
    }

    pub fn id(&mut self, field: &str, from: Option<Uuid>, to: Option<Uuid>) {
        if from != to {
            self.value(field, json!(from), json!(to));
        }
    }

    pub fn description(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let (diff, truncated) = line_diff(from, to, DIFF_BUDGET_BYTES);
        self.fields.insert(
            "description".to_owned(),
            json!({ "diff": diff, "truncated": truncated }),
        );
    }
}

/// Bytes one custom field entry takes in the stored payload, as the 8 KB
/// check reads it (`payload::text`, which puts a space after every ":" and
/// ","): its key, its value and the ", " before the next entry.
fn custom_entry_bytes(key: &str, change: &Value) -> usize {
    Value::String(key.to_owned()).to_string().len()
        + ": ".len()
        + jsonb_text_len(change)
        + ", ".len()
}

/// Length of a value as PostgreSQL writes jsonb as text, at most.
fn jsonb_text_len(value: &Value) -> usize {
    if let Value::Number(number) = value {
        return expanded_number_len(&number.to_string());
    }
    let separators = |count: usize| 2 * count.saturating_sub(1);
    match value {
        Value::Object(map) => {
            2 + separators(map.len())
                + map
                    .iter()
                    .map(|(key, value)| {
                        Value::String(key.clone()).to_string().len() + 2 + jsonb_text_len(value)
                    })
                    .sum::<usize>()
        }
        Value::Array(items) => {
            2 + separators(items.len()) + items.iter().map(jsonb_text_len).sum::<usize>()
        }
        other => other.to_string().len(),
    }
}

/// At most how long a JSON number is once PostgreSQL writes it out: jsonb
/// keeps numbers as numeric, which prints every digit and never an
/// exponent, so 1e-300 becomes "0.000…1", 302 bytes. The digits as written,
/// one more per power of ten, and room for "0.".
fn expanded_number_len(written: &str) -> usize {
    match written.split_once(['e', 'E']) {
        Some((digits, exponent)) => {
            let power = exponent
                .trim_start_matches(['+', '-'])
                .parse::<usize>()
                .unwrap_or(usize::MAX / 4);
            digits.len().saturating_add(power).saturating_add(2)
        }
        None => written.len(),
    }
}

fn cut_text(text: &str) -> String {
    cut_to(text, MAX_TEXT_CHARACTERS)
}

fn cut_to(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut = text.chars().take(max - 1).collect::<String>();
    cut.push('…');
    cut
}

/// A line diff of `from` against `to`: changed lines start with "-" or "+",
/// unchanged context lines with " ", and skipped stretches of unchanged text
/// show as " …". The second value is true when it was cut to `budget` bytes.
pub fn line_diff(from: &str, to: &str, budget: usize) -> (String, bool) {
    let old = from.lines().collect::<Vec<_>>();
    let new = to.lines().collect::<Vec<_>>();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];

    // (tag, line) for the whole text, then trimmed to context.
    let mut operations: Vec<(char, &str)> = Vec::with_capacity(old.len() + new.len());
    operations.extend(old[..prefix].iter().map(|line| (' ', *line)));
    if old_middle.len().saturating_mul(new_middle.len()) > MAX_DIFF_CELLS {
        operations.extend(old_middle.iter().map(|line| ('-', *line)));
        operations.extend(new_middle.iter().map(|line| ('+', *line)));
    } else {
        operations.extend(middle_operations(old_middle, new_middle));
    }
    operations.extend(old[old.len() - suffix..].iter().map(|line| (' ', *line)));

    let changed = operations
        .iter()
        .enumerate()
        .filter(|(_, (tag, _))| *tag != ' ')
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let near_change = |index: usize| {
        changed
            .iter()
            .any(|changed| changed.abs_diff(index) <= CONTEXT_LINES)
    };
    let mut lines = Vec::new();
    let mut skipping = false;
    for (index, (tag, line)) in operations.iter().enumerate() {
        if *tag == ' ' && !near_change(index) {
            if !skipping {
                lines.push(" …".to_owned());
                skipping = true;
            }
            continue;
        }
        skipping = false;
        lines.push(format!("{tag}{line}"));
    }
    let mut diff = String::new();
    for line in lines {
        let needed = line.len() + usize::from(!diff.is_empty());
        if diff.len() + needed > budget {
            return (diff, true);
        }
        if !diff.is_empty() {
            diff.push('\n');
        }
        diff.push_str(&line);
    }
    (diff, false)
}

/// Removals and additions turning `old` into `new`, by longest common
/// subsequence.
fn middle_operations<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(char, &'a str)> {
    let rows = old.len() + 1;
    let columns = new.len() + 1;
    let mut lengths = vec![0_u32; rows * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * columns + j] = if old[i] == new[j] {
                lengths[(i + 1) * columns + j + 1] + 1
            } else {
                lengths[(i + 1) * columns + j].max(lengths[i * columns + j + 1])
            };
        }
    }
    let mut operations = Vec::with_capacity(old.len() + new.len());
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            operations.push((' ', old[i]));
            i += 1;
            j += 1;
        } else if lengths[(i + 1) * columns + j] >= lengths[i * columns + j + 1] {
            operations.push(('-', old[i]));
            i += 1;
        } else {
            operations.push(('+', new[j]));
            j += 1;
        }
    }
    operations.extend(old[i..].iter().map(|line| ('-', *line)));
    operations.extend(new[j..].iter().map(|line| ('+', *line)));
    operations
}

/// How a custom field's change is keyed in `changes`.
pub const CUSTOM_FIELD_PREFIX: &str = "field:";

/// The fields whose `changes` hold IDs, and what each names.
const ID_FIELDS: [(&str, IdKind); 5] = [
    ("assignee", IdKind::Member),
    ("type", IdKind::Type),
    ("status", IdKind::Status),
    ("milestone", IdKind::Milestone),
    ("parent", IdKind::Task),
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum IdKind {
    Member,
    Type,
    Status,
    Milestone,
    Task,
}

/// Rewrites the `changes` of activity payloads from one project for a reader
/// of that project: IDs become names (a member's name, a status, a task's
/// key), and a status move recorded as `previousStatusId`/`statusId` is added
/// as `changes.status`. Something that no longer exists reads as null.
pub async fn label_changes(
    pool: &PgPool,
    project_id: Uuid,
    payloads: &mut [&mut Value],
) -> Result<(), AppError> {
    for payload in payloads.iter_mut() {
        let status = (
            uuid_at(payload, "previousStatusId"),
            uuid_at(payload, "statusId").or_else(|| uuid_at(payload, "newStatusId")),
        );
        if let (Some(from), Some(to)) = status
            && from != to
            && let Some(object) = payload.as_object_mut()
        {
            let changes = object
                .entry("changes")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(changes) = changes.as_object_mut() {
                changes
                    .entry("status")
                    .or_insert_with(|| json!({ "from": from, "to": to }));
            }
        }
    }
    name_custom_fields(pool, project_id, payloads).await?;
    let mut wanted: HashMap<IdKind, BTreeSet<Uuid>> = HashMap::new();
    for payload in payloads.iter() {
        let Some(changes) = payload.get("changes") else {
            continue;
        };
        for (field, kind) in ID_FIELDS {
            if let Some(change) = changes.get(field) {
                for side in ["from", "to"] {
                    if let Some(id) = uuid_at(change, side) {
                        wanted.entry(kind).or_default().insert(id);
                    }
                }
            }
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let mut names: HashMap<Uuid, String> = HashMap::new();
    for (kind, ids) in &wanted {
        let ids = ids.iter().copied().collect::<Vec<_>>();
        let query = match kind {
            IdKind::Member => {
                "SELECT actor_id, display_name FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)"
            }
            IdKind::Type => {
                "SELECT id, name FROM work_item_types WHERE project_id = $1 AND id = ANY($2)"
            }
            IdKind::Status => {
                "SELECT id, name FROM work_item_statuses WHERE project_id = $1 AND id = ANY($2)"
            }
            IdKind::Milestone => {
                "SELECT id, name FROM project_milestones WHERE project_id = $1 AND id = ANY($2)"
            }
            IdKind::Task => {
                "SELECT item.id, project.project_key || '-' || item.number FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.project_id = $1 AND item.id = ANY($2)"
            }
        };
        let rows = sqlx::query_as::<_, (Uuid, String)>(query)
            .bind(project_id)
            .bind(&ids)
            .fetch_all(pool)
            .await?;
        names.extend(rows);
    }
    for payload in payloads.iter_mut() {
        let Some(changes) = payload.get_mut("changes").and_then(Value::as_object_mut) else {
            continue;
        };
        for (field, _) in ID_FIELDS {
            let Some(change) = changes.get_mut(field).and_then(Value::as_object_mut) else {
                continue;
            };
            for side in ["from", "to"] {
                if let Some(value) = change.get_mut(side) {
                    *value = match value.as_str().and_then(|id| id.parse::<Uuid>().ok()) {
                        Some(id) => names.get(&id).cloned().map_or(Value::Null, Value::String),
                        None => Value::Null,
                    };
                }
            }
        }
    }
    Ok(())
}

/// Gives each custom field change its field's current name, so a renamed
/// field reads the same through its whole history.
async fn name_custom_fields(
    pool: &PgPool,
    project_id: Uuid,
    payloads: &mut [&mut Value],
) -> Result<(), AppError> {
    let mut ids = BTreeSet::new();
    for payload in payloads.iter() {
        if let Some(changes) = payload.get("changes").and_then(Value::as_object) {
            ids.extend(
                changes
                    .keys()
                    .filter_map(|key| key.strip_prefix(CUSTOM_FIELD_PREFIX)?.parse::<Uuid>().ok()),
            );
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    let ids = ids.into_iter().collect::<Vec<_>>();
    let names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM field_definitions WHERE project_id = $1 AND id = ANY($2)",
    )
    .bind(project_id)
    .bind(&ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    for payload in payloads.iter_mut() {
        let Some(changes) = payload.get_mut("changes").and_then(Value::as_object_mut) else {
            continue;
        };
        for (key, change) in changes.iter_mut() {
            let Some(id) = key
                .strip_prefix(CUSTOM_FIELD_PREFIX)
                .and_then(|id| id.parse::<Uuid>().ok())
            else {
                continue;
            };
            if let (Some(name), Some(change)) = (names.get(&id), change.as_object_mut()) {
                change.insert("field".to_owned(), Value::String(name.clone()));
            }
        }
    }
    Ok(())
}

fn uuid_at(value: &Value, key: &str) -> Option<Uuid> {
    value.get(key)?.as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{CUSTOM_BUDGET_BYTES, FieldChanges, jsonb_text_len, line_diff};

    /// Many custom fields changing at once stay inside their share of the
    /// payload's 8 KB, counted as PostgreSQL stores it, keys included.
    #[test]
    fn custom_field_changes_hold_their_budget() {
        // PostgreSQL's jsonb text: a space after every ":" and ",".
        assert_eq!(
            jsonb_text_len(&json!({"a": 1, "b": [1, 2]})),
            r#"{"a": 1, "b": [1, 2]}"#.len()
        );
        let name = "é".repeat(60);
        let mut changes = FieldChanges::default();
        let first = Uuid::now_v7();
        changes.custom_field(first, "Points", json!(1), json!(2));
        for _ in 0..50 {
            changes.custom_field(Uuid::now_v7(), &name, json!("a"), json!("b"));
        }
        let map = changes.into_map();
        assert!(map.len() > 1 && map.len() < 51, "{} entries", map.len());
        assert!(jsonb_text_len(&serde_json::Value::Object(map.clone())) <= CUSTOM_BUDGET_BYTES + 2);
        assert_eq!(
            map[&format!("field:{first}")],
            json!({"field": "Points", "from": 1, "to": 2})
        );
    }

    /// PostgreSQL writes 1e-300 out in full, so tiny numbers count at that
    /// length: short in JSON, about 300 bytes each when stored.
    #[test]
    fn tiny_numbers_count_as_postgresql_writes_them() {
        let expanded = format!("0.{}1", "0".repeat(299));
        assert!(jsonb_text_len(&json!(1e-300)) >= expanded.len());
        assert!(jsonb_text_len(&json!(-1.5e-7)) >= "-0.00000015".len());
        assert_eq!(jsonb_text_len(&json!(2.5)), 3);
        let mut changes = FieldChanges::default();
        for number in 0..20 {
            changes.custom_field(
                Uuid::now_v7(),
                &format!("N{number}"),
                json!(1e-300),
                json!(2e-300),
            );
        }
        let map = changes.into_map();
        assert!(!map.is_empty() && map.len() < 20, "{} entries", map.len());
        assert!(jsonb_text_len(&serde_json::Value::Object(map)) <= CUSTOM_BUDGET_BYTES + 2);
    }

    #[test]
    fn records_only_fields_that_changed() {
        let mut changes = FieldChanges::default();
        changes.value("priority", json!("high"), json!("urgent"));
        changes.value("estimate", json!(3.0), json!(3.0));
        changes.text("title", Some("Same"), Some("Same"));
        changes.id("assignee", None, None);
        let assignee = Uuid::nil();
        changes.id("milestone", Some(assignee), None);
        let map = changes.into_map();
        assert_eq!(map.len(), 2);
        assert_eq!(map["priority"], json!({"from": "high", "to": "urgent"}));
        assert_eq!(map["milestone"]["to"], json!(null));
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let mut changes = FieldChanges::default();
        let long = "é".repeat(400);
        changes.text("blockedReason", None, Some(&long));
        let to = changes.into_map()["blockedReason"]["to"]
            .as_str()
            .expect("cut text")
            .to_owned();
        assert_eq!(to.chars().count(), 300);
        assert!(to.ends_with('…'));
    }

    #[test]
    fn diff_shows_changed_lines_with_context() {
        let from = "one\ntwo\nthree\nfour\nfive\nsix";
        let to = "one\ntwo\nTHREE\nfour\nfive\nsix\nseven";
        let (diff, truncated) = line_diff(from, to, 1_000);
        assert!(!truncated);
        assert_eq!(diff, " …\n two\n-three\n+THREE\n four\n …\n six\n+seven");
    }

    #[test]
    fn diff_of_new_text_is_all_additions() {
        let (diff, _) = line_diff("", "a\nb", 1_000);
        assert_eq!(diff, "+a\n+b");
    }

    #[test]
    fn diff_is_cut_to_its_budget_on_a_line() {
        let to = (0..200)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (diff, truncated) = line_diff("", &to, 100);
        assert!(truncated);
        assert!(diff.len() <= 100);
        assert!(diff.lines().all(|line| line.starts_with("+line ")));
    }

    #[test]
    fn huge_rewrites_skip_the_full_comparison() {
        let from = (0..3_000)
            .map(|line| format!("a{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let to = (0..3_000)
            .map(|line| format!("b{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (diff, truncated) = line_diff(&from, &to, 200);
        assert!(truncated);
        assert!(diff.starts_with("-a0"));
    }
}
