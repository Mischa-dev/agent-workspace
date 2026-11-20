//! Running a saved query (T1.34): asking for a run marks its answer's place
//! pending and queues a job; the job opens the connection through
//! `outbound`, runs the query in a read-only transaction under a time
//! limit, keeps at most its row cap of rows, and stores the answer (or the
//! database's refusal) in place of the last one. No transaction of this
//! server's is open while the outside database works.

use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{
    Column, Connection as _, Executor, Statement,
    postgres::{PgArguments, PgTypeInfo},
    query::QueryScalar,
};
use uuid::Uuid;

use super::{
    Bound, MAX_COLUMNS, MAX_STORED_BYTES, Query, ReviewState, STATEMENT_TIMEOUT_SECONDS, prepare,
};
use crate::{
    AppState,
    error::AppError,
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    outbound::{self, DatabaseAddress, DatabaseRefusal, Refusal},
};

pub const RUN_KIND: &str = "outside_data.run";

/// The longest a whole run takes: connecting, the query, reading its rows.
const RUN_TIMEOUT: Duration = Duration::from_secs(STATEMENT_TIMEOUT_SECONDS + 30);
/// A run asked for this long ago and not finished was lost (its server
/// stopped twice); another may start.
const STALE_PENDING: &str = "5 minutes";
const MAX_ERROR_CHARACTERS: usize = 2_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunJob {
    query_id: Uuid,
    reviewed: bool,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        RUN_KIND,
        RUN_TIMEOUT + Duration::from_secs(30),
        |context: JobContext| async move {
            let job: RunJob = context.job.payload_as()?;
            run(&context.state, job.query_id, job.reviewed).await
        },
    );
}

/// What asking for a run did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requested {
    Queued,
    /// A run of this answer's place is already under way.
    AlreadyRunning,
}

/// Asks for a run of `query` into its draft or its reviewed place, with
/// `values` (by parameter name) over its defaults. Part of the caller's
/// transaction: the job exists exactly when the request commits.
pub async fn request(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    query: &Query,
    reviewed: bool,
    requested_by: Uuid,
    values: &Map<String, Value>,
) -> Result<Requested, AppError> {
    let marked = sqlx::query_scalar::<_, bool>(&format!(
        "INSERT INTO data_query_results \
             (query_id, reviewed, workspace_id, space_id, pending_since, pending_parameters, \
              requested_by_actor_id) \
         VALUES ($1, $2, $3, $4, now(), $5, $6) \
         ON CONFLICT (query_id, reviewed) DO UPDATE \
             SET pending_since = now(), pending_parameters = EXCLUDED.pending_parameters, \
                 requested_by_actor_id = EXCLUDED.requested_by_actor_id \
             WHERE data_query_results.pending_since IS NULL \
                OR data_query_results.pending_since < now() - interval '{STALE_PENDING}' \
         RETURNING true"
    ))
    .bind(query.id)
    .bind(reviewed)
    .bind(query.workspace_id)
    .bind(query.space_id)
    .bind(Value::Object(values.clone()))
    .bind(requested_by)
    .fetch_optional(&mut **transaction)
    .await?
    .is_some();
    if !marked {
        return Ok(Requested::AlreadyRunning);
    }
    jobs::enqueue(
        &mut **transaction,
        NewJob::new(RUN_KIND)
            .payload(RunJob {
                query_id: query.id,
                reviewed,
            })
            .project(query.workspace_id, query.space_id)
            .unique(format!("{RUN_KIND}:{}:{reviewed}", query.id))
            .max_attempts(2),
    )
    .await?;
    Ok(Requested::Queued)
}

/// What one run found.
struct Answer {
    columns: Vec<String>,
    rows: Vec<Value>,
    truncated: bool,
}

async fn run(state: &AppState, query_id: Uuid, reviewed: bool) -> Result<(), JobError> {
    // The job runs in its workspace's scope; nothing of this server's stays
    // open while the outside database works.
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = sqlx::query_as::<_, (Uuid, Value)>(
        "SELECT space_id, pending_parameters FROM data_query_results \
         WHERE query_id = $1 AND reviewed = $2 AND pending_since IS NOT NULL",
    )
    .bind(query_id)
    .bind(reviewed)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((space_id, values)) = found else {
        return Ok(());
    };
    let Some(query) = super::query(&mut transaction, space_id, query_id, false).await? else {
        return Ok(());
    };
    // Changed since it was asked for: a reviewed run of a draft never runs.
    if reviewed && query.review != ReviewState::Reviewed {
        sqlx::query(
            "UPDATE data_query_results SET pending_since = NULL \
             WHERE query_id = $1 AND reviewed = true",
        )
        .bind(query_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok(());
    }
    let found = super::connection(&mut transaction, space_id, query.connection_id, false).await?;
    let secret = match found.and_then(|found| found.secret_id) {
        None => None,
        Some(secret_id) => {
            let keys = crate::secrets::keyring(&state.config)
                .map_err(|_| JobError::permanent("keyring"))?;
            Some(
                crate::secrets::reveal(
                    &mut *transaction,
                    keys.as_ref(),
                    secret_id,
                    "outside_data.run",
                )
                .await?,
            )
        }
    };
    transaction.commit().await?;
    let started = Instant::now();
    let outcome = match secret {
        None => Err("This query's connection has no address; set it again.".to_owned()),
        Some(secret) => match DatabaseAddress::parse(secret.expose()) {
            Err(why) => Err(why.to_owned()),
            Ok(address) => {
                match tokio::time::timeout(
                    RUN_TIMEOUT,
                    execute(
                        &address,
                        state.config.allow_private_addresses,
                        &query,
                        values.as_object().cloned().unwrap_or_default(),
                    ),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => Err(format!(
                        "The query ran past {} seconds and was stopped.",
                        RUN_TIMEOUT.as_secs()
                    )),
                }
            }
        },
    };
    let duration = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
    let (columns, rows, truncated, error) = match outcome {
        Ok(answer) => (answer.columns, answer.rows, answer.truncated, None),
        Err(why) => (
            Vec::new(),
            Vec::new(),
            false,
            Some(why.chars().take(MAX_ERROR_CHARACTERS).collect::<String>()),
        ),
    };
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query(
        "UPDATE data_query_results \
         SET pending_since = NULL, ran_at = now(), ran_by_actor_id = requested_by_actor_id, \
             parameters = pending_parameters, columns = $3, rows = $4, row_count = $5, \
             truncated = $6, error = $7, duration_ms = $8 \
         WHERE query_id = $1 AND reviewed = $2",
    )
    .bind(query_id)
    .bind(reviewed)
    .bind(serde_json::json!(columns))
    .bind(Value::Array(rows.clone()))
    .bind(i32::try_from(rows.len()).unwrap_or(i32::MAX))
    .bind(truncated)
    .bind(error)
    .bind(duration)
    .execute(&mut *transaction)
    .await?;
    super::announce_answer(&mut transaction, space_id, query_id).await?;
    super::emit_changed(
        &mut transaction,
        query.workspace_id,
        space_id,
        "data_query",
        query_id,
        None,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

fn type_of(value: &Bound) -> PgTypeInfo {
    PgTypeInfo::with_name(match value {
        Bound::Text(_) => "text",
        Bound::Whole(_) => "int8",
        Bound::Number(_) => "float8",
        Bound::Date(_) => "date",
        Bound::Boolean(_) => "bool",
    })
}

fn bind<'q>(
    query: QueryScalar<'q, sqlx::Postgres, String, PgArguments>,
    value: Bound,
) -> QueryScalar<'q, sqlx::Postgres, String, PgArguments> {
    match value {
        Bound::Text(value) => query.bind(value),
        Bound::Whole(value) => query.bind(value),
        Bound::Number(value) => query.bind(value),
        Bound::Date(value) => query.bind(value),
        Bound::Boolean(value) => query.bind(value),
    }
}

fn database_words(error: sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(error) => format!("The database said: {}", error.message()),
        other => format!("The database couldn't be read: {other}"),
    }
}

fn refusal_words(refusal: DatabaseRefusal) -> String {
    match refusal {
        DatabaseRefusal::Address(Refusal::Private) => {
            "That address is on a private network, which this server doesn't connect to.".into()
        }
        DatabaseRefusal::Address(Refusal::Unresolved) => {
            "That address's name doesn't resolve.".into()
        }
        DatabaseRefusal::ClientCertificate => {
            "This server is set up with a database client certificate of its own, so it \
             doesn't connect to outside databases."
                .into()
        }
        DatabaseRefusal::Timeout => "The database didn't answer in time.".into(),
        DatabaseRefusal::Refused(words) => format!("The database refused the connection: {words}"),
    }
}

async fn execute(
    address: &DatabaseAddress,
    allow_private: bool,
    query: &Query,
    values: Map<String, Value>,
) -> Result<Answer, String> {
    let prepared = prepare(&query.statement, &query.parameters)?;
    let mut bounds = Vec::with_capacity(prepared.order.len());
    let mut types = vec![PgTypeInfo::with_name("int8")];
    for index in &prepared.order {
        let parameter = &query.parameters[*index];
        let value = values.get(&parameter.name).or(parameter.default.as_ref());
        let bound = Bound::of(parameter.kind, value)
            .map_err(|why| format!("{}: {why}.", parameter.name))?;
        types.push(type_of(&bound));
        bounds.push(bound);
    }
    let settings = [
        ("default_transaction_read_only", "on".to_owned()),
        ("statement_timeout", format!("{STATEMENT_TIMEOUT_SECONDS}s")),
        ("lock_timeout", "5s".to_owned()),
        ("idle_in_transaction_session_timeout", "60s".to_owned()),
    ];
    let mut connection = outbound::database_connection(address, allow_private, &settings)
        .await
        .map_err(refusal_words)?;
    let answer = async {
        let mut transaction = connection.begin().await.map_err(database_words)?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(database_words)?;
        let statement = (&mut *transaction)
            .prepare_with(&prepared.columns_sql, &types)
            .await
            .map_err(database_words)?;
        let columns = statement
            .columns()
            .iter()
            .map(|column| column.name().to_owned())
            .collect::<Vec<_>>();
        if columns.len() > MAX_COLUMNS {
            return Err(format!(
                "The query gives {} columns; at most {MAX_COLUMNS} are kept. Name the ones you need.",
                columns.len()
            ));
        }
        let cap = usize::try_from(query.row_cap).unwrap_or(0);
        let mut select = sqlx::query_scalar::<_, String>(&prepared.rows_sql)
            .bind(i64::from(query.row_cap) + 1);
        for value in bounds {
            select = bind(select, value);
        }
        let mut rows = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        {
            let mut stream = select.fetch(&mut *transaction);
            while let Some(row) = stream.try_next().await.map_err(database_words)? {
                bytes += row.len();
                if rows.len() >= cap || bytes > MAX_STORED_BYTES {
                    truncated = true;
                    break;
                }
                rows.push(serde_json::from_str::<Value>(&row).unwrap_or(Value::Null));
            }
        }
        transaction.rollback().await.map_err(database_words)?;
        Ok(Answer {
            columns,
            rows,
            truncated,
        })
    }
    .await;
    let _ = connection.close().await;
    answer
}
