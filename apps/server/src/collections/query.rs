//! Finding a collection's records on the server, a page at a time (AWD-90).
//!
//! As task lists do (`work_items::list`), a page is found in two steps: the
//! records' IDs in order (filters, keyset and sort only), then the records
//! themselves (`records::by_ids`). A cursor is a keyset over the whole
//! order, group included, and remembers a fingerprint of what it was made
//! for, so paging never uses OFFSET, never reads the whole collection, and
//! a cursor made for other filters is refused. Group counts are one
//! GROUP BY over the same filters.

use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::views::{Direction, SortBy, ViewConfig};
use crate::{
    error::AppError,
    fields::{self, FieldClause, FieldDefinition, FieldOp, FieldType, GroupSql},
};

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;
/// Words one search takes; each must match.
pub const MAX_SEARCH_WORDS: usize = 8;
pub const MAX_SEARCH_CHARACTERS: usize = 200;
/// Records one query names by number or ID.
pub const MAX_NAMED_RECORDS: usize = 100;
/// Room for the longest cursor a query can make: a person grouping and three
/// text sorts, each cut to `fields::SORT_PREFIX_CHARACTERS`, at the most
/// bytes a character can take (see the test `the_longest_cursor_fits`).
const MAX_CURSOR_LENGTH: usize = 8_192;

/// Which records a query sees.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Archived {
    /// Only records that aren't archived.
    #[default]
    Exclude,
    Include,
    Only,
}

impl Archived {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "exclude" | "false" | "no" => Some(Self::Exclude),
            "include" | "all" | "true" | "yes" => Some(Self::Include),
            "only" => Some(Self::Only),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Exclude => "exclude",
            Self::Include => "include",
            Self::Only => "only",
        }
    }
}

/// One sort key, its field filled in for a field sort.
#[derive(Clone, Debug, PartialEq)]
pub struct Sort {
    pub by: SortBy,
    pub desc: bool,
    pub field: Option<FieldDefinition>,
}

/// What to find, already checked against the collection's fields.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    /// Field filters with stored values ("me" already the reader).
    pub clauses: Vec<FieldClause>,
    /// Words that must each appear in the title, a text or web address
    /// value or a chosen option's name, or be the record's number.
    pub words: Vec<String>,
    pub sorts: Vec<Sort>,
    /// A field whose type can group.
    pub group: Option<FieldDefinition>,
    pub archived: Archived,
    /// Only these records.
    pub only: Option<Vec<Uuid>>,
    /// Only records in this group of `group` (a key `group_counts` gives,
    /// such as an option's ID or "none"): one column of a board.
    pub in_group: Option<String>,
    /// Read the project's tasks: the built-in Tasks collection (`tasks`).
    pub tasks: bool,
    /// The collection's text, web address and choice fields, for `words`.
    search_fields: SearchFields,
}

#[derive(Clone, Debug, Default)]
struct SearchFields {
    text: Vec<String>,
    /// (field IDs, then for each word the option IDs whose names hold it).
    choices: Vec<String>,
    options: Vec<(String, String)>,
}

impl Spec {
    /// A spec from a view's settings (or one a screen is trying out),
    /// checked with `views::check` first. "me" in a person filter becomes
    /// `reader`; a field archived since is left out.
    pub fn from_config(config: &ViewConfig, definitions: &[FieldDefinition], reader: Uuid) -> Self {
        let config = config.active(definitions);
        let mut clauses = config.filters.clone();
        fields::resolve_me(&mut clauses, definitions, reader);
        let sorts = config
            .sort
            .iter()
            .filter_map(|rule| {
                let by = SortBy::parse(&rule.field)?;
                let field = match by {
                    SortBy::Field(id) => Some(definitions.iter().find(|field| field.id == id)?),
                    _ => None,
                };
                Some(Sort {
                    by,
                    desc: rule.direction == Direction::Desc,
                    field: field.cloned(),
                })
            })
            .collect();
        let group = config.group_by.and_then(|id| {
            definitions
                .iter()
                .find(|field| field.id == id && field.field_type.can_group())
                .cloned()
        });
        Self {
            clauses,
            sorts,
            group,
            ..Self::default()
        }
        .searching(Vec::new(), definitions)
    }

    /// Sets the words to search for, read against `definitions`.
    pub fn searching(mut self, words: Vec<String>, definitions: &[FieldDefinition]) -> Self {
        let active = definitions.iter().filter(|field| !field.archived);
        let mut search = SearchFields::default();
        for field in active {
            match field.field_type {
                FieldType::Text | FieldType::Url => search.text.push(field.id.to_string()),
                FieldType::SingleChoice | FieldType::MultipleChoice => {
                    search.choices.push(field.id.to_string());
                    for option in &field.config.options {
                        search
                            .options
                            .push((option.id.to_string(), option.name.to_lowercase()));
                    }
                }
                _ => {}
            }
        }
        self.words = words;
        self.search_fields = search;
        self
    }

    fn keys(&self) -> Vec<Key> {
        let mut keys = self
            .group
            .as_ref()
            .and_then(group_sql)
            .map(|group| {
                group
                    .order
                    .into_iter()
                    .map(|key| Key {
                        expr: key.expr,
                        ty: key.ty,
                        desc: false,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let sorts = if self.sorts.is_empty() {
            vec![Sort {
                by: SortBy::Number,
                desc: false,
                field: None,
            }]
        } else {
            self.sorts.clone()
        };
        for sort in &sorts {
            match (sort.by, &sort.field) {
                (SortBy::Title, _) => keys.push(Key::new(
                    &format!(
                        "left(lower(record.title), {})",
                        fields::SORT_PREFIX_CHARACTERS
                    ),
                    "text",
                    sort.desc,
                )),
                (SortBy::Number, _) => keys.push(Key::new("record.number", "bigint", sort.desc)),
                (SortBy::Updated, _) => {
                    keys.push(Key::new("record.updated_at", "timestamptz", sort.desc));
                }
                (SortBy::Field(_), Some(field)) => {
                    keys.extend(
                        fields::sort_keys(field, "record.field_values", "record.project_id")
                            .into_iter()
                            .map(|key| Key {
                                expr: key.expr,
                                ty: key.ty,
                                desc: sort.desc && key.follows_direction,
                            }),
                    );
                }
                (SortBy::Field(_), None) => {}
            }
        }
        keys.push(Key::new("record.id", "uuid", false));
        keys
    }

    /// Changes whenever anything that decides the rows or their order does.
    fn fingerprint(&self, project_id: Uuid, collection_id: Uuid) -> String {
        let mut hasher = Sha256::new();
        hasher.update(project_id.as_bytes());
        hasher.update(collection_id.as_bytes());
        hasher.update(fields::clauses_param(&self.clauses));
        hasher.update(format!("{:?}", self.words));
        for sort in &self.sorts {
            hasher.update(format!("{}:{}", sort.by.key(), sort.desc));
            if let Some(field) = &sort.field {
                hasher.update(format!("{:?}", field.config));
            }
        }
        if let Some(group) = &self.group {
            hasher.update(group.id.as_bytes());
            hasher.update(format!("{:?}", group.config));
        }
        hasher.update(self.archived.as_str());
        if let Some(key) = &self.in_group {
            hasher.update(b"in-group:");
            hasher.update(key.as_bytes());
        }
        if let Some(only) = &self.only {
            for id in only {
                hasher.update(id.as_bytes());
            }
        }
        hex::encode(&hasher.finalize()[..12])
    }

    /// The table (or the tasks' subquery) records are read from, aliased
    /// `record`.
    fn source_sql(&self) -> String {
        if self.tasks {
            format!("{} record", super::tasks::source())
        } else {
            "collection_records record".to_owned()
        }
    }

    fn push_where(
        &self,
        builder: &mut QueryBuilder<'_, Postgres>,
        project_id: Uuid,
        collection_id: Uuid,
    ) {
        builder.push(" WHERE record.project_id = ");
        builder.push_bind(project_id);
        builder.push(" AND record.collection_id = ");
        builder.push_bind(collection_id);
        builder.push(match self.archived {
            Archived::Exclude => " AND record.archived_at IS NULL",
            Archived::Include => "",
            Archived::Only => " AND record.archived_at IS NOT NULL",
        });
        if let Some(only) = &self.only {
            builder.push(" AND record.id = ANY(");
            builder.push_bind(only.clone());
            builder.push(")");
        }
        fields::push_clauses(builder, "record.field_values", &self.clauses);
        if let (Some(key), Some(group)) = (&self.in_group, self.group.as_ref().and_then(group_sql))
        {
            // The group's key worked out for this record alone, so the page,
            // the total and the group counts read it the same way.
            builder.push(format!(
                " AND (SELECT {key} FROM (SELECT record.field_values, record.project_id) record {join}) = ",
                key = group.key,
                join = group.join,
            ));
            builder.push_bind(key.clone());
        }
        for word in &self.words {
            let escaped = word
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            builder.push(" AND (record.title ILIKE ");
            builder.push_bind(pattern.clone());
            if !self.search_fields.text.is_empty() {
                builder.push(
                    " OR EXISTS (SELECT 1 FROM jsonb_each_text(record.field_values) \
                     AS search_value(key, value) WHERE search_value.key = ANY(",
                );
                builder.push_bind(self.search_fields.text.clone());
                builder.push(") AND search_value.value ILIKE ");
                builder.push_bind(pattern);
                builder.push(")");
            }
            let lower = word.to_lowercase();
            let options = self
                .search_fields
                .options
                .iter()
                .filter(|(_, name)| name.contains(&lower))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if !options.is_empty() {
                // A single choice stores the option's ID, a multiple choice
                // a list of them; `?|` reads both.
                builder.push(
                    " OR EXISTS (SELECT 1 FROM jsonb_each(record.field_values) \
                     AS search_choice(key, value) WHERE search_choice.key = ANY(",
                );
                builder.push_bind(self.search_fields.choices.clone());
                builder.push(") AND search_choice.value ?| ");
                builder.push_bind(options);
                builder.push("::text[])");
            }
            if let Some(number) = word
                .trim_start_matches('#')
                .parse::<i64>()
                .ok()
                .filter(|number| *number > 0)
            {
                builder.push(" OR record.number = ");
                builder.push_bind(number);
            }
            builder.push(")");
        }
    }
}

/// The words of a search, at most `MAX_SEARCH_WORDS`. None when the search
/// is too long.
pub fn search_words(value: &str) -> Option<Vec<String>> {
    if value.chars().count() > MAX_SEARCH_CHARACTERS || value.chars().any(char::is_control) {
        return None;
    }
    let mut seen = HashSet::new();
    let words = value
        .split_whitespace()
        .filter(|word| seen.insert(word.to_lowercase()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (words.len() <= MAX_SEARCH_WORDS).then_some(words)
}

/// The longest group key a query takes: an option's or person's ID, or
/// "none" or "true".
pub const MAX_GROUP_KEY_LENGTH: usize = 64;

/// The days of a calendar's range at most: six weeks and a bit.
pub const MAX_RANGE_DAYS: i64 = 62;

/// The filters for records whose `field` (an active date field) is from
/// `from` up to, not including, `to`, both YYYY-MM-DD.
pub fn date_range(
    definitions: &[FieldDefinition],
    field: Uuid,
    from: &str,
    to: &str,
) -> Result<Vec<FieldClause>, String> {
    let Some(definition) = definitions
        .iter()
        .find(|definition| definition.id == field && !definition.archived)
    else {
        return Err("The date field names a field this collection doesn't have.".to_owned());
    };
    if definition.field_type != FieldType::Date {
        return Err(format!("\"{}\" isn't a date field.", definition.name));
    }
    let parse = |value: &str| NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok();
    let (Some(start), Some(end)) = (parse(from), parse(to)) else {
        return Err("from and to are dates, YYYY-MM-DD.".to_owned());
    };
    let days = (end - start).num_days();
    if !(1..=MAX_RANGE_DAYS).contains(&days) {
        return Err(format!(
            "A date range runs forward, at most {MAX_RANGE_DAYS} days."
        ));
    }
    Ok(vec![
        FieldClause {
            field,
            op: FieldOp::Gte,
            values: vec![start.format("%Y-%m-%d").to_string()],
        },
        FieldClause {
            field,
            op: FieldOp::Lt,
            values: vec![end.format("%Y-%m-%d").to_string()],
        },
    ])
}

fn group_sql(field: &FieldDefinition) -> Option<GroupSql> {
    if super::tasks::is_builtin_field(field.id)
        && let Some(group) =
            super::tasks::group_sql(field, "record.field_values", "record.project_id")
    {
        return Some(group);
    }
    fields::group_sql(
        field.id,
        field.field_type,
        "record.field_values",
        "record.project_id",
    )
}

/// One sort key: a non-null SQL expression, the type its cursor value is
/// cast back to, and the direction.
#[derive(Clone, Debug)]
struct Key {
    expr: String,
    ty: &'static str,
    desc: bool,
}

impl Key {
    fn new(expr: &str, ty: &'static str, desc: bool) -> Self {
        Self {
            expr: expr.to_owned(),
            ty,
            desc,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorBody {
    f: String,
    k: Vec<String>,
}

fn encode_cursor(fingerprint: String, keys: Vec<String>) -> Result<String, AppError> {
    let body = serde_json::to_vec(&CursorBody {
        f: fingerprint,
        k: keys,
    })
    .map_err(|_| AppError::Internal("encode record cursor"))?;
    Ok(URL_SAFE_NO_PAD.encode(body))
}

/// The cursor's keys, or None when it isn't one made for this query (the
/// caller starts over).
fn decode_cursor(value: &str, fingerprint: &str, key_count: usize) -> Option<Vec<String>> {
    if value.len() > MAX_CURSOR_LENGTH {
        return None;
    }
    let body: CursorBody = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    (body.f == fingerprint && body.k.len() == key_count).then_some(body.k)
}

/// Rows strictly after `after` in the order of `keys`: the lexicographic
/// comparison spelt out, since the directions can differ.
fn push_after(builder: &mut QueryBuilder<'_, Postgres>, keys: &[Key], after: Vec<String>) {
    builder.push(" AND (");
    for index in 0..keys.len() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder.push("(");
        for (equal, value) in keys.iter().zip(&after).take(index) {
            builder.push(format!("({}) = CAST(", equal.expr));
            builder.push_bind(value.clone());
            builder.push(format!(" AS {}) AND ", equal.ty));
        }
        let key = &keys[index];
        builder.push(format!(
            "({}) {} CAST(",
            key.expr,
            if key.desc { "<" } else { ">" }
        ));
        builder.push_bind(after[index].clone());
        builder.push(format!(" AS {}))", key.ty));
    }
    builder.push(")");
}

/// The group a record is in.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRef {
    pub key: String,
    pub label: String,
}

#[derive(Debug, FromRow)]
struct PageRow {
    id: Uuid,
    group_key: Option<String>,
    group_label: Option<String>,
    keys: Vec<String>,
}

/// One page of record IDs in order, each with its group when grouped.
#[derive(Clone, Debug)]
pub struct Page {
    pub rows: Vec<(Uuid, Option<GroupRef>)>,
    pub next_cursor: Option<String>,
}

/// Refused cursors, for the caller's message.
pub const CURSOR_MESSAGE: &str = "That cursor was made for another query. Start again without it.";

/// One page of the records `spec` finds. Err(()) when the cursor isn't one
/// made for this query.
pub async fn page(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    spec: &Spec,
    cursor: Option<&str>,
    limit: i64,
) -> Result<Result<Page, ()>, AppError> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let keys = spec.keys();
    let fingerprint = spec.fingerprint(project_id, collection_id);
    let after = match cursor {
        Some(value) => match decode_cursor(value, &fingerprint, keys.len()) {
            Some(after) => Some(after),
            None => return Ok(Err(())),
        },
        None => None,
    };
    let group = spec.group.as_ref().and_then(group_sql);
    let (join, group_key, group_label) = match &group {
        Some(group) => (group.join.clone(), group.key.clone(), group.label.clone()),
        None => (
            String::new(),
            "NULL::text".to_owned(),
            "NULL::text".to_owned(),
        ),
    };
    let key_array = keys
        .iter()
        .map(|key| format!("({})::text", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT record.id, {group_key} AS group_key, {group_label} AS group_label, \
         ARRAY[{key_array}] AS keys FROM {from} {join}",
        from = spec.source_sql(),
    ));
    spec.push_where(&mut builder, project_id, collection_id);
    if let Some(after) = after {
        push_after(&mut builder, &keys, after);
    }
    builder.push(" ORDER BY ");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(format!(
            "{} {}",
            key.expr,
            if key.desc { "DESC" } else { "ASC" }
        ));
    }
    builder.push(" LIMIT ");
    builder.push_bind(limit + 1);
    let mut rows = builder
        .build_query_as::<PageRow>()
        .fetch_all(executor)
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = match (has_more, rows.last()) {
        (true, Some(last)) => Some(encode_cursor(fingerprint, last.keys.clone())?),
        _ => None,
    };
    Ok(Ok(Page {
        rows: rows
            .into_iter()
            .map(|row| {
                let group = group
                    .as_ref()
                    .and(row.group_key.zip(row.group_label))
                    .map(|(key, label)| GroupRef { key, label });
                (row.id, group)
            })
            .collect(),
        next_cursor,
    }))
}

/// How many records `spec` finds, across every page.
pub async fn total(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    spec: &Spec,
) -> Result<i64, AppError> {
    let mut builder =
        QueryBuilder::<Postgres>::new(format!("SELECT count(*) FROM {}", spec.source_sql()));
    spec.push_where(&mut builder, project_id, collection_id);
    Ok(builder
        .build_query_scalar::<i64>()
        .fetch_one(executor)
        .await?)
}

/// A group with records under the filters, and how many.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupCount {
    pub key: String,
    pub label: String,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Every group with records under `spec`'s filters, in the list's order.
/// Empty when it doesn't group.
pub async fn group_counts(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    spec: &Spec,
) -> Result<Vec<GroupCount>, AppError> {
    let Some(group) = spec.group.as_ref().and_then(group_sql) else {
        return Ok(Vec::new());
    };
    let order = group
        .order
        .iter()
        .map(|key| format!("min({})", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT {key} AS key, min({label}) AS label, count(*) AS count, min({color}) AS color \
         FROM {from} {join}",
        from = spec.source_sql(),
        key = group.key,
        label = group.label,
        color = group.color,
        join = group.join,
    ));
    spec.push_where(&mut builder, project_id, collection_id);
    builder.push(format!(
        " GROUP BY {} ORDER BY {order}, {}",
        group.key, group.key
    ));
    Ok(builder
        .build_query_as::<GroupCount>()
        .fetch_all(executor)
        .await?)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{Archived, MAX_CURSOR_LENGTH, Spec, decode_cursor, encode_cursor, search_words};
    use crate::{
        collections::views::{Direction, SortRule, ViewConfig},
        fields::{FieldConfig, FieldDefinition, FieldType, SORT_PREFIX_CHARACTERS},
    };

    fn field(name: &str, field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
        }
    }

    /// The most keys a query can sort by, each at its longest: text cut to
    /// SORT_PREFIX_CHARACTERS of the character JSON spends most bytes on, and
    /// anything else (numbers, times, IDs) at 40 characters. Its cursor
    /// still reads back, so paging never stops on a record with long values.
    #[test]
    fn the_longest_cursor_fits() {
        let owner = field("Owner", FieldType::Person);
        let texts = ["Notes", "Steps", "Link"].map(|name| field(name, FieldType::Text));
        let mut definitions = vec![owner.clone()];
        definitions.extend(texts.iter().cloned());
        let config = ViewConfig {
            sort: texts
                .iter()
                .map(|text| SortRule {
                    field: text.id.to_string(),
                    direction: Direction::Asc,
                })
                .collect(),
            group_by: Some(owner.id),
            ..ViewConfig::default()
        };
        let spec = Spec::from_config(&config, &definitions, Uuid::now_v7());
        let keys = spec.keys();
        // The person group's three, two for each text sort, then the ID.
        assert_eq!(keys.len(), 10);
        let longest = keys
            .iter()
            .map(|key| {
                if key.ty == "text" {
                    "\u{1}".repeat(SORT_PREFIX_CHARACTERS)
                } else {
                    "9".repeat(40)
                }
            })
            .collect::<Vec<_>>();
        let fingerprint = spec.fingerprint(Uuid::now_v7(), Uuid::now_v7());
        let cursor = encode_cursor(fingerprint.clone(), longest.clone()).expect("cursor");
        assert!(
            cursor.len() <= MAX_CURSOR_LENGTH,
            "{} characters",
            cursor.len()
        );
        assert_eq!(
            decode_cursor(&cursor, &fingerprint, keys.len()),
            Some(longest)
        );
        // A title sort is cut the same way.
        let by_title = Spec::from_config(
            &ViewConfig {
                sort: vec![SortRule {
                    field: "title".to_owned(),
                    direction: Direction::Desc,
                }],
                ..ViewConfig::default()
            },
            &definitions,
            Uuid::now_v7(),
        );
        assert!(
            by_title.keys()[0]
                .expr
                .contains(&format!(", {SORT_PREFIX_CHARACTERS})")),
            "{:?}",
            by_title.keys()
        );
    }

    #[test]
    fn searches_are_a_few_words() {
        assert_eq!(
            search_words("  crash  level ").expect("two words"),
            vec!["crash".to_owned(), "level".to_owned()]
        );
        assert_eq!(
            search_words("a A a").expect("one word"),
            vec!["a".to_owned()]
        );
        let nine = (1..=9).map(|n| format!("w{n}")).collect::<Vec<_>>();
        assert!(search_words(&nine.join(" ")).is_none());
        assert_eq!(
            search_words(&nine[..8].join(" "))
                .expect("eight words")
                .len(),
            8
        );
        assert!(search_words(&"x".repeat(201)).is_none());
    }

    #[test]
    fn a_cursor_only_reads_back_for_its_own_query() {
        let field = FieldDefinition {
            id: Uuid::now_v7(),
            name: "Score".to_owned(),
            description: String::new(),
            field_type: FieldType::Number,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
        };
        let definitions = vec![field.clone()];
        let config = ViewConfig {
            sort: vec![SortRule {
                field: field.id.to_string(),
                direction: Direction::Desc,
            }],
            ..ViewConfig::default()
        };
        let spec = Spec::from_config(&config, &definitions, Uuid::now_v7());
        // A field sort: records without a value last, the value, then ID.
        assert_eq!(spec.keys().len(), 3);
        let (project, collection) = (Uuid::now_v7(), Uuid::now_v7());
        let fingerprint = spec.fingerprint(project, collection);
        let cursor = encode_cursor(
            fingerprint.clone(),
            vec!["0".into(), "3".into(), "x".into()],
        )
        .expect("cursor");
        assert!(decode_cursor(&cursor, &fingerprint, 3).is_some());
        let archived = Spec {
            archived: Archived::Include,
            ..spec.clone()
        };
        assert_ne!(archived.fingerprint(project, collection), fingerprint);
        assert_ne!(spec.fingerprint(project, Uuid::now_v7()), fingerprint);
        assert!(decode_cursor(&cursor, &fingerprint, 2).is_none());
        assert!(decode_cursor("not a cursor", &fingerprint, 3).is_none());
    }
}
