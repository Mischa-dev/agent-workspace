//! Reading tasks out of Markdown, CSV and GitHub issues for imports
//! (docs/plans/roadmap.md, W11, T9). Pure functions: nothing here touches
//! the database, so the shapes other tools export can be tested directly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most rows one import reads.
pub(crate) const MAX_ROWS: usize = 5_000;
const MAX_FIELD_CHARACTERS: usize = 20_000;

/// A task as read from the source, before it is matched to the project.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportRow {
    /// Where it came from, such as an issue number or a Linear ID, so a
    /// second import skips it. None for Markdown lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Checked off or closed at the source.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    /// Closed as not planned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LabelValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<String>,
    /// The row this one is a subtask of, by its place in the import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// A parent named by its external ID, resolved into `parent` once all
    /// rows are read.
    #[serde(skip)]
    pub parent_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LabelValue {
    pub name: String,
    /// Six hex digits with a leading #, when the source has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn label(name: &str) -> LabelValue {
    LabelValue {
        name: name.trim().to_owned(),
        color: None,
    }
}

fn clip(value: &str) -> String {
    value.trim().chars().take(MAX_FIELD_CHARACTERS).collect()
}

fn present(value: &str) -> Option<String> {
    let value = clip(value);
    (!value.is_empty()).then_some(value)
}

// ---------------------------------------------------------------------------
// CSV

/// Splits CSV text (RFC 4180: quoted fields may hold commas, quotes as "",
/// and line breaks) into records. A byte-order mark is skipped and blank
/// lines are dropped.
pub(crate) fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = sniff_delimiter(text);
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_field_start = true;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if quoted {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(character);
            }
            continue;
        }
        match character {
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            character if character == delimiter => {
                record.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                if record.iter().any(|value| !value.trim().is_empty()) {
                    records.push(std::mem::take(&mut record));
                } else {
                    record.clear();
                }
                at_field_start = true;
                if records.len() > MAX_ROWS + 1 {
                    return Err(format!("Import at most {MAX_ROWS} rows at a time."));
                }
            }
            character => {
                field.push(character);
                at_field_start = false;
            }
        }
    }
    if quoted {
        return Err("A quoted field never ends. Check the file is complete.".to_owned());
    }
    record.push(field);
    if record.iter().any(|value| !value.trim().is_empty()) {
        records.push(record);
    }
    if records.len() > MAX_ROWS + 1 {
        return Err(format!("Import at most {MAX_ROWS} rows at a time."));
    }
    Ok(records)
}

/// Comma unless the header line has more semicolons or tabs, as
/// spreadsheets in some locales write.
fn sniff_delimiter(text: &str) -> char {
    let header = text.lines().next().unwrap_or_default();
    [',', ';', '\t']
        .into_iter()
        .max_by_key(|delimiter| (header.matches(*delimiter).count(), *delimiter == ','))
        .unwrap_or(',')
}

/// The task fields a column can fill.
pub(crate) const FIELDS: [&str; 13] = [
    "externalId",
    "title",
    "description",
    "status",
    "priority",
    "labels",
    "assignee",
    "type",
    "milestone",
    "startDate",
    "dueDate",
    "estimate",
    "parent",
];

/// Column names other tools use, lower case, best first. Linear and Jira
/// exports map without changes.
fn aliases(field: &str) -> &'static [&'static str] {
    match field {
        "externalId" => &[
            "id",
            "issue key",
            "key",
            "identifier",
            "issue id",
            "number",
            "#",
        ],
        "title" => &["title", "summary", "name", "task", "issue", "subject"],
        "description" => &["description", "body", "details", "notes", "content"],
        "status" => &["status", "state", "column", "stage"],
        "priority" => &["priority", "urgency"],
        "labels" => &["labels", "label", "tags", "tag"],
        "assignee" => &["assignee", "assigned to", "owner", "assignees"],
        "type" => &["issue type", "type", "kind"],
        "milestone" => &[
            "milestone",
            "project",
            "cycle name",
            "sprint",
            "fix version/s",
            "epic",
        ],
        "startDate" => &["start date", "start", "started"],
        "dueDate" => &["due date", "due", "deadline", "due on"],
        "estimate" => &[
            "estimate",
            "story points",
            "points",
            "custom field (story points)",
            "size",
        ],
        "parent" => &[
            "parent",
            "parent id",
            "parent issue",
            "parent key",
            "parent summary",
        ],
        _ => &[],
    }
}

/// Picks a column for each field from the header names.
pub(crate) fn auto_mapping(headers: &[String]) -> BTreeMap<String, String> {
    let lower = headers
        .iter()
        .map(|header| header.trim().to_lowercase())
        .collect::<Vec<_>>();
    let mut mapping = BTreeMap::new();
    let mut used = Vec::<usize>::new();
    for field in FIELDS {
        for alias in aliases(field) {
            if let Some(index) = lower
                .iter()
                .enumerate()
                .position(|(index, header)| header == alias && !used.contains(&index))
            {
                used.push(index);
                mapping.insert(field.to_owned(), headers[index].trim().to_owned());
                break;
            }
        }
    }
    mapping
}

/// Turns CSV records into rows by `mapping` (field to column name). Every
/// column with a mapped name counts, so Jira's repeated Labels columns all
/// land. Rows without a title are kept so the preview can say so.
pub(crate) fn csv_rows(
    records: &[Vec<String>],
    mapping: &BTreeMap<String, String>,
) -> Result<Vec<ImportRow>, String> {
    let Some((headers, body)) = records.split_first() else {
        return Err("The file is empty.".to_owned());
    };
    if body.len() > MAX_ROWS {
        return Err(format!("Import at most {MAX_ROWS} rows at a time."));
    }
    let columns = |field: &str| -> Vec<usize> {
        mapping
            .get(field)
            .map(|name| {
                headers
                    .iter()
                    .enumerate()
                    .filter(|(_, header)| header.trim().eq_ignore_ascii_case(name.trim()))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    };
    if columns("title").is_empty() {
        return Err("Pick the column that holds each task's title.".to_owned());
    }
    let first = |record: &Vec<String>, field: &str| -> Option<String> {
        columns(field)
            .into_iter()
            .filter_map(|index| record.get(index))
            .find_map(|value| present(value))
    };
    let mut rows = Vec::with_capacity(body.len());
    for record in body {
        let mut labels = Vec::<LabelValue>::new();
        for index in columns("labels") {
            let Some(value) = record.get(index) else {
                continue;
            };
            for name in value.split([',', ';', '|']) {
                let name = name.trim();
                if !name.is_empty()
                    && !labels
                        .iter()
                        .any(|kept| kept.name.eq_ignore_ascii_case(name))
                {
                    labels.push(label(name));
                }
            }
        }
        let status = first(record, "status");
        let done = status.as_deref().is_some_and(looks_done);
        rows.push(ImportRow {
            external_id: first(record, "externalId").map(|id| id.chars().take(200).collect()),
            title: first(record, "title").unwrap_or_default(),
            description: first(record, "description").unwrap_or_default(),
            status,
            done,
            canceled: false,
            priority: first(record, "priority"),
            labels,
            assignee: first(record, "assignee"),
            type_name: first(record, "type"),
            milestone: first(record, "milestone"),
            start_date: first(record, "startDate"),
            due_date: first(record, "dueDate"),
            estimate: first(record, "estimate"),
            parent: None,
            parent_reference: first(record, "parent"),
        });
    }
    link_parents(&mut rows);
    Ok(rows)
}

fn looks_done(status: &str) -> bool {
    matches!(
        status.trim().to_lowercase().as_str(),
        "done" | "closed" | "complete" | "completed" | "resolved" | "shipped" | "merged"
    )
}

/// Points each row whose parent column names another row's external ID
/// (or, for Jira's "Parent summary", its title) at that row.
fn link_parents(rows: &mut [ImportRow]) {
    let by_id = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| Some((row.external_id.clone()?.to_lowercase(), index)))
        .collect::<BTreeMap<_, _>>();
    let by_title = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.title.to_lowercase(), index))
        .collect::<BTreeMap<_, _>>();
    for index in 0..rows.len() {
        let Some(reference) = rows[index].parent_reference.take() else {
            continue;
        };
        let key = reference.to_lowercase();
        let parent = by_id.get(&key).or_else(|| by_title.get(&key)).copied();
        // Only an earlier row can be a parent, so creating in order always
        // has the parent first and a loop can't form.
        rows[index].parent = parent.filter(|parent| *parent < index);
    }
}

// ---------------------------------------------------------------------------
// Markdown

/// Reads a Markdown task list. Each list item (`- [ ] task`, `- task`,
/// `1. task`) is a task; items indented under it are its subtasks; other
/// indented lines become its description. `[x]` marks it done and trailing
/// `#words` become labels. Headings become milestones when asked, and are
/// otherwise left out.
pub(crate) fn markdown_rows(
    text: &str,
    headings_as_milestones: bool,
) -> Result<Vec<ImportRow>, String> {
    let mut rows: Vec<ImportRow> = Vec::new();
    // (indent, row index) of the open list items, outermost first.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut heading: Option<String> = None;
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            if let Some(&(_, index)) = stack.last() {
                append_description(&mut rows[index], trimmed);
            }
            continue;
        }
        if in_code {
            if let Some(&(_, index)) = stack.last() {
                append_description(&mut rows[index], line.trim_end());
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let indent = line[..indent]
            .chars()
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum::<usize>();
        if let Some(title) = heading_text(trimmed) {
            stack.clear();
            heading = Some(title);
            continue;
        }
        if let Some((done, text)) = list_item(trimmed) {
            while stack.last().is_some_and(|&(open, _)| open >= indent) {
                stack.pop();
            }
            let (title, labels) = split_tags(text);
            if title.is_empty() {
                continue;
            }
            if rows.len() >= MAX_ROWS {
                return Err(format!("Import at most {MAX_ROWS} tasks at a time."));
            }
            let parent = stack.last().map(|&(_, index)| index);
            rows.push(ImportRow {
                title: title.chars().take(240).collect(),
                done,
                labels,
                milestone: if headings_as_milestones {
                    heading.clone()
                } else {
                    None
                },
                parent,
                ..ImportRow::default()
            });
            stack.push((indent, rows.len() - 1));
            continue;
        }
        // A plain line: the description of the item it sits under, if any.
        if let Some(&(open, index)) = stack.last()
            && indent > open
        {
            append_description(&mut rows[index], trimmed);
        }
    }
    Ok(rows)
}

fn append_description(row: &mut ImportRow, line: &str) {
    if row.description.chars().count() >= MAX_FIELD_CHARACTERS {
        return;
    }
    if !row.description.is_empty() {
        row.description.push('\n');
    }
    row.description.push_str(line);
}

fn heading_text(line: &str) -> Option<String> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.starts_with(' ') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    (!text.is_empty()).then(|| text.chars().take(120).collect())
}

/// The text of a list item, and whether it's checked off.
fn list_item(line: &str) -> Option<(bool, &str)> {
    let rest = if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        rest
    } else {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))?
    };
    let rest = rest.trim_start();
    for (marker, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(text) = rest.strip_prefix(marker) {
            return Some((done, text.trim()));
        }
    }
    Some((false, rest.trim()))
}

/// Pulls trailing `#tag` words off a title as labels.
fn split_tags(text: &str) -> (String, Vec<LabelValue>) {
    let mut words = text.split_whitespace().collect::<Vec<_>>();
    let mut labels = Vec::new();
    while let Some(word) = words.last() {
        let Some(tag) = word.strip_prefix('#') else {
            break;
        };
        if tag.is_empty() || tag.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        labels.insert(0, label(tag));
        words.pop();
    }
    (words.join(" "), labels)
}

// ---------------------------------------------------------------------------
// GitHub

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GitHubIssue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    #[serde(default)]
    pub assignee: Option<GitHubUser>,
    #[serde(default)]
    pub milestone: Option<GitHubMilestone>,
    /// Present on pull requests, which the issues list includes.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GitHubLabel {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GitHubUser {
    pub login: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GitHubMilestone {
    pub title: String,
}

/// Issues as rows, pull requests left out.
pub(crate) fn github_rows(issues: Vec<GitHubIssue>) -> Vec<ImportRow> {
    issues
        .into_iter()
        .filter(|issue| issue.pull_request.is_none())
        .take(MAX_ROWS)
        .map(|issue| {
            let closed = issue.state == "closed";
            let canceled = closed && issue.state_reason.as_deref() == Some("not_planned");
            ImportRow {
                external_id: Some(issue.number.to_string()),
                title: clip(&issue.title).chars().take(240).collect(),
                description: issue.body.as_deref().map(clip).unwrap_or_default(),
                status: None,
                done: closed && !canceled,
                canceled,
                priority: None,
                labels: issue
                    .labels
                    .iter()
                    .map(|label| LabelValue {
                        name: label.name.trim().to_owned(),
                        color: label
                            .color
                            .as_deref()
                            .filter(|color| {
                                color.len() == 6 && color.chars().all(|c| c.is_ascii_hexdigit())
                            })
                            .map(|color| format!("#{}", color.to_lowercase())),
                    })
                    .collect(),
                assignee: issue.assignee.map(|user| user.login),
                type_name: None,
                milestone: issue
                    .milestone
                    .as_ref()
                    .map(|milestone| milestone.title.clone()),
                start_date: None,
                due_date: None,
                estimate: None,
                parent: None,
                parent_reference: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Values

/// A priority as other tools write it.
pub(crate) fn priority_of(value: &str) -> Option<&'static str> {
    let value = value.trim().to_lowercase();
    let value = value
        .trim_start_matches("priority")
        .trim_matches(|c: char| c == ':' || c == '-' || c.is_whitespace());
    Some(match value {
        "urgent" | "highest" | "critical" | "blocker" | "p0" | "1" => "urgent",
        "high" | "major" | "p1" | "2" => "high",
        "medium" | "normal" | "p2" | "3" => "medium",
        "low" | "lowest" | "minor" | "trivial" | "p3" | "p4" | "4" => "low",
        "" | "none" | "no priority" | "no_priority" | "0" | "-" => "no_priority",
        _ => return None,
    })
}

/// A date in the forms exports use: 2026-10-16, 2026-10-16T09:00:00Z,
/// 16/Oct/26 (Jira), 10/16/2026.
pub(crate) fn date_of(value: &str) -> Option<chrono::NaiveDate> {
    use chrono::NaiveDate;
    let value = value.trim();
    if value.len() >= 10
        && let Ok(date) = NaiveDate::parse_from_str(&value[..10], "%Y-%m-%d")
    {
        return Some(date);
    }
    let first = value.split_whitespace().next().unwrap_or_default();
    ["%d/%b/%y", "%d/%b/%Y", "%m/%d/%Y", "%d.%m.%Y", "%b %d, %Y"]
        .into_iter()
        .find_map(|format| {
            NaiveDate::parse_from_str(first, format)
                .or_else(|_| NaiveDate::parse_from_str(value, format))
                .ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_handles_quotes_line_breaks_and_semicolons() {
        let records =
            parse_csv("Title,Body\r\n\"Fix \"\"login\"\"\",\"line one\nline two\"\n\n,\nNext,x\n")
                .unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1], vec!["Fix \"login\"", "line one\nline two"]);
        let semicolons = parse_csv("Title;Status\nA;Done\n").unwrap();
        assert_eq!(semicolons[1], vec!["A", "Done"]);
        assert!(parse_csv("Title\n\"open").is_err());
    }

    #[test]
    fn linear_export_maps_without_changes() {
        let text = "ID,Team,Title,Description,Status,Estimate,Priority,Project,Assignee,Labels,Due Date,Parent issue\n\
                    ENG-1,Eng,Ship it,,In Progress,3,High,Launch,ana,\"Bug, Frontend\",2026-11-01,\n\
                    ENG-2,Eng,Write docs,,Done,,No priority,Launch,,,,ENG-1\n";
        let records = parse_csv(text).unwrap();
        let mapping = auto_mapping(&records[0]);
        assert_eq!(mapping.get("externalId").map(String::as_str), Some("ID"));
        assert_eq!(
            mapping.get("milestone").map(String::as_str),
            Some("Project")
        );
        let rows = csv_rows(&records, &mapping).unwrap();
        assert_eq!(rows[0].title, "Ship it");
        assert_eq!(rows[0].labels.len(), 2);
        assert_eq!(rows[0].estimate.as_deref(), Some("3"));
        assert!(rows[1].done);
        assert_eq!(rows[1].parent, Some(0));
    }

    #[test]
    fn jira_repeated_label_columns_all_count() {
        let text = "Summary,Issue key,Issue Type,Status,Labels,Labels,Due Date\nA,PRJ-1,Bug,Open,one,two,16/Oct/26\n";
        let records = parse_csv(text).unwrap();
        let mapping = auto_mapping(&records[0]);
        let rows = csv_rows(&records, &mapping).unwrap();
        assert_eq!(
            rows[0]
                .labels
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(rows[0].type_name.as_deref(), Some("Bug"));
        assert_eq!(
            date_of(rows[0].due_date.as_deref().unwrap()),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 16)
        );
    }

    #[test]
    fn markdown_lists_become_tasks_and_subtasks() {
        let text = "# Launch\n\n- [ ] Ship the app #release\n  Notes about it\n  - [x] Write tests\n- Plain item\n\n## Later\n1. Numbered #idea\n";
        let rows = markdown_rows(text, true).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].title, "Ship the app");
        assert_eq!(rows[0].labels[0].name, "release");
        assert_eq!(rows[0].description, "Notes about it");
        assert_eq!(rows[0].milestone.as_deref(), Some("Launch"));
        assert_eq!(rows[1].parent, Some(0));
        assert!(rows[1].done);
        assert_eq!(rows[2].parent, None);
        assert_eq!(rows[3].milestone.as_deref(), Some("Later"));
        assert!(markdown_rows(text, false).unwrap()[0].milestone.is_none());
    }

    #[test]
    fn github_issues_skip_pull_requests() {
        let issues: Vec<GitHubIssue> = serde_json::from_value(serde_json::json!([
            {"number": 1, "title": "Bug", "state": "open", "labels": [{"name": "bug", "color": "D73A4A"}]},
            {"number": 2, "title": "PR", "state": "open", "pull_request": {}},
            {"number": 3, "title": "Won't do", "state": "closed", "state_reason": "not_planned"}
        ]))
        .unwrap();
        let rows = github_rows(issues);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].labels[0].color.as_deref(), Some("#d73a4a"));
        assert!(rows[1].canceled && !rows[1].done);
    }

    #[test]
    fn priorities_and_dates_read_common_forms() {
        assert_eq!(priority_of("P1"), Some("high"));
        assert_eq!(priority_of("No priority"), Some("no_priority"));
        assert_eq!(priority_of("Highest"), Some("urgent"));
        assert_eq!(priority_of("whenever"), None);
        assert_eq!(
            date_of("2026-10-16T09:00:00Z"),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 16)
        );
        assert_eq!(
            date_of("10/16/2026"),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 16)
        );
        assert_eq!(date_of("soon"), None);
    }
}
