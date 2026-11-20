//! Outside data (T1.34, T1.35): a space's connections to databases kept
//! elsewhere, and saved queries over them whose answers are stored.
//!
//! A connection's address, password and all, is a secret (`secrets`); the
//! row shows `user@host:port/database`. Only a role with `data.connect`
//! sees connections, writes queries, reviews them and runs drafts.
//!
//! A query is one SELECT with `{{name}}` parameters. It runs as a job
//! (`run`): on a connection opened through `outbound` (the address checked
//! and pinned), in a read-only transaction, under a statement time limit,
//! keeping at most its row cap of rows (and at most [`MAX_STORED_BYTES`]).
//! Its answer is stored, so a dashboard shows it without anyone reaching
//! the database again.
//!
//! Review (T1.35): a query is a draft until someone with `data.connect`
//! reviews it, and any change to what it runs (its text, parameters, row
//! cap or connection, or the connection's address) makes it a draft again.
//! A reviewed query can be shared (`data.share`): everyone in the space then
//! sees the answer of its last reviewed run and may run it again, with its
//! defaults, and never sees its text or its connection. Answers are kept in
//! two places per query, its last draft run and its last reviewed run, so a
//! draft run never shows on a shared dashboard.

pub mod ops;
pub mod run;

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

/// Connections one space holds.
pub const MAX_CONNECTIONS: i64 = 50;
/// Saved queries one space holds.
pub const MAX_QUERIES: i64 = 200;
pub const MAX_NAME_CHARACTERS: usize = 80;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 2_000;
pub const MAX_STATEMENT_CHARACTERS: usize = 20_000;
pub const MAX_ADDRESS_CHARACTERS: usize = 1_000;
pub const MAX_PARAMETERS: usize = 20;
pub const DEFAULT_ROW_CAP: i32 = 1_000;
pub const MAX_ROW_CAP: i32 = 5_000;
/// The most a stored answer holds, as JSON; rows past it are left out, as
/// rows past the cap are.
pub const MAX_STORED_BYTES: usize = 2_000_000;
pub const MAX_COLUMNS: usize = 100;
/// How long one query may run on the outside database.
pub const STATEMENT_TIMEOUT_SECONDS: u64 = 30;

/// A connection as stored.
#[derive(Clone, Debug, FromRow)]
pub struct Connection {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub name: String,
    pub kind: String,
    pub address: String,
    pub secret_id: Option<Uuid>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const CONNECTION_COLUMNS: &str = "id, workspace_id, space_id, name, kind, address, secret_id, \
    version, created_at, updated_at";

pub async fn connections(
    connection: &mut PgConnection,
    space_id: Uuid,
) -> Result<Vec<Connection>, AppError> {
    Ok(sqlx::query_as::<_, Connection>(&format!(
        "SELECT {CONNECTION_COLUMNS} FROM data_connections WHERE space_id = $1 ORDER BY lower(name)"
    ))
    .bind(space_id)
    .fetch_all(connection)
    .await?)
}

pub async fn connection(
    connection: &mut PgConnection,
    space_id: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Option<Connection>, AppError> {
    Ok(sqlx::query_as::<_, Connection>(&format!(
        "SELECT {CONNECTION_COLUMNS} FROM data_connections WHERE space_id = $1 AND id = $2{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(space_id)
    .bind(id)
    .fetch_optional(connection)
    .await?)
}

/// What a parameter holds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParameterKind {
    Text,
    Number,
    /// A day, such as 2026-11-19.
    Date,
    Boolean,
}

/// A query's parameter: `{{name}}` in its text.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    /// Lower case letters, digits and _, starting with a letter.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ParameterKind,
    /// Its value when a run gives none (and the value every run by someone
    /// without data.connect uses); null when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// A parameter's value, as bound.
#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    Text(Option<String>),
    /// A whole number binds as `int8`, so `LIMIT {{n}}` and
    /// `generate_series(1, {{n}})` take it; a fraction as `float8`.
    Whole(Option<i64>),
    Number(Option<f64>),
    Date(Option<NaiveDate>),
    Boolean(Option<bool>),
}

impl Bound {
    /// `value` read as `kind`; `None` or null binds null.
    pub fn of(kind: ParameterKind, value: Option<&Value>) -> Result<Self, &'static str> {
        let value = value.filter(|value| !value.is_null());
        Ok(match kind {
            ParameterKind::Text => Self::Text(match value {
                None => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(Value::Number(number)) => Some(number.to_string()),
                Some(Value::Bool(flag)) => Some(flag.to_string()),
                Some(_) => return Err("a text parameter takes text"),
            }),
            ParameterKind::Number => {
                let number = match value {
                    None => None,
                    Some(Value::Number(number)) => number.as_f64(),
                    Some(Value::String(text)) => Some(
                        text.trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|number| number.is_finite())
                            .ok_or("a number parameter takes a number")?,
                    ),
                    Some(_) => return Err("a number parameter takes a number"),
                };
                const EXACT: f64 = 9_007_199_254_740_992.0;
                match number {
                    None => Self::Whole(None),
                    Some(number) if number.fract() == 0.0 && number.abs() <= EXACT =>
                    {
                        #[allow(clippy::cast_possible_truncation)]
                        Self::Whole(Some(number as i64))
                    }
                    Some(number) => Self::Number(Some(number)),
                }
            }
            ParameterKind::Date => Self::Date(match value {
                None => None,
                Some(Value::String(text)) => Some(
                    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                        .map_err(|_| "a date parameter takes a day such as 2026-11-19")?,
                ),
                Some(_) => return Err("a date parameter takes a day such as 2026-11-19"),
            }),
            ParameterKind::Boolean => Self::Boolean(match value {
                None => None,
                Some(Value::Bool(flag)) => Some(*flag),
                Some(Value::String(text)) if text == "true" => Some(true),
                Some(Value::String(text)) if text == "false" => Some(false),
                Some(_) => return Err("a boolean parameter takes true or false"),
            }),
        })
    }

    /// As stored beside an answer.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Text(value) => value.clone().map(Value::String).unwrap_or(Value::Null),
            Self::Whole(value) => value.map(Value::from).unwrap_or(Value::Null),
            Self::Number(value) => value
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Self::Date(value) => value
                .map(|day| Value::String(day.format("%Y-%m-%d").to_string()))
                .unwrap_or(Value::Null),
            Self::Boolean(value) => value.map(Value::Bool).unwrap_or(Value::Null),
        }
    }
}

fn parameter_name_ok(name: &str) -> bool {
    let mut characters = name.chars();
    (1..=40).contains(&name.len())
        && characters.next().is_some_and(|c| c.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks a query's parameters as written: each named once, rightly, with a
/// default of its kind.
pub fn check_parameters(parameters: &[Parameter]) -> Result<(), String> {
    if parameters.len() > MAX_PARAMETERS {
        return Err(format!(
            "A query takes at most {MAX_PARAMETERS} parameters."
        ));
    }
    for (index, parameter) in parameters.iter().enumerate() {
        if !parameter_name_ok(&parameter.name) {
            return Err(format!(
                "\"{}\" can't name a parameter: use lower case letters, digits and _, starting \
                 with a letter.",
                parameter.name
            ));
        }
        if parameters[..index]
            .iter()
            .any(|other| other.name == parameter.name)
        {
            return Err(format!("The parameter {} is named twice.", parameter.name));
        }
        Bound::of(parameter.kind, parameter.default.as_ref())
            .map_err(|why| format!("The default of {}: {why}.", parameter.name))?;
    }
    Ok(())
}

/// A query ready to run: its rows as JSON arrays (`$1` the row limit, then
/// each parameter in `order`), and the same select for its columns.
#[derive(Debug)]
pub struct Prepared {
    pub rows_sql: String,
    pub columns_sql: String,
    /// Index into the query's parameters for `$2`, `$3` and on.
    pub order: Vec<usize>,
}

/// Turns a query's text into what runs: `{{name}}` becomes a numbered
/// parameter, and the select is wrapped so each row comes back as one JSON
/// array in column order. A name the query doesn't declare is refused.
pub fn prepare(statement: &str, parameters: &[Parameter]) -> Result<Prepared, String> {
    let body = statement
        .trim_end()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    if body.trim().is_empty() {
        return Err("The query is empty.".into());
    }
    let mut out = String::with_capacity(body.len() + 16);
    let mut order: Vec<usize> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err("A {{ has no }} after it.".into());
        };
        let name = after[..end].trim();
        let Some(index) = parameters.iter().position(|p| p.name == name) else {
            return Err(format!(
                "{{{{{name}}}}} isn't one of the query's parameters: add it, or take it out."
            ));
        };
        let number = match order.iter().position(|seen| *seen == index) {
            Some(at) => at + 2,
            None => {
                order.push(index);
                order.len() + 1
            }
        };
        out.push('$');
        out.push_str(&number.to_string());
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(Prepared {
        rows_sql: format!(
            "SELECT (SELECT coalesce(json_agg(entry.value), '[]'::json) \
             FROM json_each(row_to_json(answer)) AS entry)::text \
             FROM (\n{out}\n) AS answer LIMIT $1"
        ),
        columns_sql: format!("SELECT * FROM (\n{out}\n) AS answer LIMIT $1"),
        order,
    })
}

/// Whether a query is a draft or reviewed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReviewState {
    Draft,
    Reviewed,
}

impl ReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Reviewed => "reviewed",
        }
    }
}

/// A saved query as stored.
#[derive(Clone, Debug)]
pub struct Query {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub connection_id: Uuid,
    pub name: String,
    pub description: String,
    pub statement: String,
    pub parameters: Vec<Parameter>,
    pub row_cap: i32,
    pub review: ReviewState,
    pub reviewed_by_actor_id: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub shared: bool,
    pub version: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct QueryRow {
    id: Uuid,
    workspace_id: Uuid,
    space_id: Uuid,
    connection_id: Uuid,
    name: String,
    description: String,
    statement: String,
    parameters: Value,
    row_cap: i32,
    review_state: String,
    reviewed_by_actor_id: Option<Uuid>,
    reviewed_at: Option<DateTime<Utc>>,
    shared: bool,
    version: i64,
    updated_at: DateTime<Utc>,
}

impl From<QueryRow> for Query {
    fn from(row: QueryRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            space_id: row.space_id,
            connection_id: row.connection_id,
            name: row.name,
            description: row.description,
            statement: row.statement,
            parameters: serde_json::from_value(row.parameters).unwrap_or_default(),
            row_cap: row.row_cap,
            review: if row.review_state == "reviewed" {
                ReviewState::Reviewed
            } else {
                ReviewState::Draft
            },
            reviewed_by_actor_id: row.reviewed_by_actor_id,
            reviewed_at: row.reviewed_at,
            shared: row.shared,
            version: row.version,
            updated_at: row.updated_at,
        }
    }
}

const QUERY_COLUMNS: &str = "id, workspace_id, space_id, connection_id, name, description, \
    statement, parameters, row_cap, review_state, reviewed_by_actor_id, reviewed_at, shared, \
    version, updated_at";

/// A space's saved queries by name; with `shared_only`, the shared ones.
pub async fn queries(
    connection: &mut PgConnection,
    space_id: Uuid,
    shared_only: bool,
) -> Result<Vec<Query>, AppError> {
    let rows = sqlx::query_as::<_, QueryRow>(&format!(
        "SELECT {QUERY_COLUMNS} FROM data_queries WHERE space_id = $1 AND (shared OR NOT $2) \
         ORDER BY lower(name)"
    ))
    .bind(space_id)
    .bind(shared_only)
    .fetch_all(connection)
    .await?;
    Ok(rows.into_iter().map(Query::from).collect())
}

pub async fn query(
    connection: &mut PgConnection,
    space_id: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Option<Query>, AppError> {
    Ok(sqlx::query_as::<_, QueryRow>(&format!(
        "SELECT {QUERY_COLUMNS} FROM data_queries WHERE space_id = $1 AND id = $2{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(space_id)
    .bind(id)
    .fetch_optional(connection)
    .await?
    .map(Query::from))
}

/// A stored answer: a query's last draft run, or its last reviewed one.
#[derive(Clone, Debug, FromRow)]
pub struct Stored {
    pub reviewed: bool,
    pub pending_since: Option<DateTime<Utc>>,
    pub ran_at: Option<DateTime<Utc>>,
    pub ran_by_actor_id: Option<Uuid>,
    pub parameters: Value,
    pub columns: Value,
    pub rows: Value,
    pub row_count: i32,
    pub truncated: bool,
    pub error: Option<String>,
    pub duration_ms: Option<i32>,
}

/// A query's stored answers, the reviewed one first.
pub async fn stored(
    connection: &mut PgConnection,
    query_id: Uuid,
) -> Result<Vec<Stored>, AppError> {
    Ok(sqlx::query_as::<_, Stored>(
        "SELECT reviewed, pending_since, ran_at, ran_by_actor_id, parameters, columns, rows, \
                row_count, truncated, error, duration_ms \
         FROM data_query_results WHERE query_id = $1 ORDER BY reviewed DESC",
    )
    .bind(query_id)
    .fetch_all(connection)
    .await?)
}

/// The answer `full` (someone with data.connect) sees: the latest of the
/// two, a run pending counting as latest. Anyone else sees the reviewed one,
/// and only of a shared query.
pub fn answer_for(query: &Query, answers: Vec<Stored>, full: bool) -> Option<Stored> {
    if !full {
        return query
            .shared
            .then(|| answers.into_iter().find(|answer| answer.reviewed))
            .flatten();
    }
    answers.into_iter().max_by_key(|answer| {
        (
            answer.pending_since.is_some(),
            answer.pending_since.or(answer.ran_at),
        )
    })
}

/// Tells open screens that each shared dashboard showing `query_id` has a
/// new answer.
pub async fn announce_answer(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    space_id: Uuid,
    query_id: Uuid,
) -> Result<(), AppError> {
    let dashboards = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, version FROM dashboards \
         WHERE space_id = $1 AND owner_actor_id IS NULL AND tiles @> $2",
    )
    .bind(space_id)
    .bind(serde_json::json!([{ "query": query_id }]))
    .fetch_all(&mut **transaction)
    .await?;
    for (dashboard, version) in dashboards {
        crate::outbox::enqueue_dashboard_updated(transaction, space_id, dashboard, version).await?;
    }
    Ok(())
}

/// Says a connection or a saved query changed (or a query's answer came
/// in), for open screens and automations: ids only, never its text.
pub async fn emit_changed(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    space_id: Uuid,
    kind: &'static str,
    id: Uuid,
    actor: Option<Uuid>,
) -> Result<(), AppError> {
    use crate::events::{self, NewEvent, catalog};
    let ty = if kind == "data_connection" {
        &catalog::DATA_CONNECTION_CHANGED
    } else {
        &catalog::DATA_QUERY_CHANGED
    };
    let field = if kind == "data_connection" {
        "connectionId"
    } else {
        "queryId"
    };
    let mut event = NewEvent::thing(ty, space_id, kind, id)
        .payload(serde_json::json!({ field: id }))
        .workspace(workspace_id);
    if let Some(actor) = actor {
        event = event.actor(actor);
    }
    events::emit(transaction, event).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parameter(name: &str, kind: ParameterKind) -> Parameter {
        Parameter {
            name: name.into(),
            kind,
            default: None,
        }
    }

    #[test]
    fn parameters_become_numbered_and_the_select_is_wrapped() {
        let parameters = [
            parameter("since", ParameterKind::Date),
            parameter("team", ParameterKind::Text),
        ];
        let prepared = prepare(
            "SELECT * FROM orders WHERE team = {{ team }} AND day >= {{since}} \
             AND owner = {{team}};  \n",
            &parameters,
        )
        .expect("prepared");
        assert_eq!(prepared.order, vec![1, 0]);
        assert!(
            prepared
                .columns_sql
                .contains("team = $2 AND day >= $3 AND owner = $2\n) AS answer LIMIT $1"),
            "{}",
            prepared.columns_sql
        );
        assert!(
            prepared
                .rows_sql
                .starts_with("SELECT (SELECT coalesce(json_agg")
        );
    }

    #[test]
    fn an_undeclared_parameter_or_an_open_brace_is_refused() {
        assert!(prepare("SELECT {{who}}", &[]).is_err());
        assert!(prepare("SELECT {{who", &[parameter("who", ParameterKind::Text)]).is_err());
        assert!(prepare(" ;; ", &[]).is_err());
    }

    #[test]
    fn values_are_read_as_their_kind() {
        assert_eq!(
            Bound::of(ParameterKind::Number, Some(&json!("4.5"))),
            Ok(Bound::Number(Some(4.5)))
        );
        assert_eq!(
            Bound::of(ParameterKind::Number, Some(&json!(12))),
            Ok(Bound::Whole(Some(12)))
        );
        assert_eq!(
            Bound::of(ParameterKind::Date, Some(&json!("2026-11-19"))),
            Ok(Bound::Date(NaiveDate::from_ymd_opt(2026, 11, 19)))
        );
        assert!(Bound::of(ParameterKind::Date, Some(&json!("tomorrow"))).is_err());
        assert_eq!(
            Bound::of(ParameterKind::Boolean, Some(&Value::Null)),
            Ok(Bound::Boolean(None))
        );
        let twice = [
            parameter("a", ParameterKind::Text),
            parameter("a", ParameterKind::Text),
        ];
        assert!(check_parameters(&twice).is_err());
        assert!(check_parameters(&[parameter("Bad", ParameterKind::Text)]).is_err());
    }
}
