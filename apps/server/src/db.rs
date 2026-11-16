//! Database work in a tenancy scope (H1.20).
//!
//! [`scoped_tx`] begins a transaction whose first statement, sent with
//! `BEGIN` in one round trip, sets `app.tenancy` transaction-locally to the
//! current [`crate::tenancy::Tenancy`]. [`scoped_one`] runs one piece of
//! work in such a transaction and commits it. Nothing is ever set on the
//! session, so a transaction-mode pooler in front of PostgreSQL is safe:
//! the setting ends with the transaction (R24 rule 3).
//!
//! [`scoped`] stands in for the pool where a query is handed one
//! (`.fetch_one(db::scoped(&state.pool))`): every statement run through it
//! gets a scoped transaction of its own, committed when it finishes, which is
//! [`scoped_one`] without the closure. H1.45 to H1.62 move the pool call
//! sites onto these, one module directory at a time.
//!
//! The call sites that still query the pool outside a scoped transaction are
//! counted per directory by the ratchet in `tenancy/ratchet.json` (H1.44),
//! which may only go down.

use std::{future::Future, panic::Location};

use futures_util::{
    StreamExt, TryStreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};
use sqlx::{
    Describe, Either, Execute, Executor, PgConnection, PgPool, Postgres, Transaction,
    postgres::{PgQueryResult, PgRow, PgStatement, PgTypeInfo},
};

use crate::tenancy::{self, Tenancy};

/// Begins a transaction in the current tenancy scope.
///
/// With no scope at all (a background loop not yet given one), the
/// transaction carries none: in observe mode that is allowed, and the call
/// site is logged once and counted.
#[track_caller]
pub fn scoped_tx(
    pool: &PgPool,
) -> impl Future<Output = Result<Transaction<'static, Postgres>, sqlx::Error>> + '_ {
    begin_scoped(pool, Location::caller(), "")
}

/// A read-only snapshot in the current tenancy scope: [`scoped_tx`] begun
/// `ISOLATION LEVEL REPEATABLE READ, READ ONLY`, for reads that must agree
/// with each other (a page of results and its count). The mode is set by the
/// `BEGIN` itself, since nothing may run before it.
#[track_caller]
pub fn scoped_snapshot(
    pool: &PgPool,
) -> impl Future<Output = Result<Transaction<'static, Postgres>, sqlx::Error>> + '_ {
    begin_scoped(
        pool,
        Location::caller(),
        " ISOLATION LEVEL REPEATABLE READ, READ ONLY",
    )
}

fn begin_scoped<'p>(
    pool: &'p PgPool,
    caller: &'static Location<'static>,
    mode: &'static str,
) -> impl Future<Output = Result<Transaction<'static, Postgres>, sqlx::Error>> + 'p {
    async move {
        let tenancy = tenancy::resolved(pool).await?;
        crate::metrics::tenancy_transaction(tenancy.as_ref().map_or("unscoped", Tenancy::kind));
        match tenancy {
            Some(tenancy) => {
                pool.begin_with(format!(
                    "BEGIN{mode}; SELECT set_config('app.tenancy', '{}', true)",
                    tenancy.setting()
                ))
                .await
            }
            None => {
                observe_unscoped(caller);
                if mode.is_empty() {
                    pool.begin().await
                } else {
                    pool.begin_with(format!("BEGIN{mode}")).await
                }
            }
        }
    }
}

/// Runs `work` on a connection inside a scoped transaction and commits it:
/// for a single statement that used to run on the pool.
///
/// ```ignore
/// let name = db::scoped_one(&state.pool, async |connection| {
///     sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1")
///         .bind(project_id)
///         .fetch_one(connection)
///         .await
/// })
/// .await?;
/// ```
#[track_caller]
pub fn scoped_one<'p, T, E, F>(pool: &'p PgPool, work: F) -> impl Future<Output = Result<T, E>> + 'p
where
    F: AsyncFnOnce(&mut PgConnection) -> Result<T, E> + 'p,
    E: From<sqlx::Error>,
{
    let begin = scoped_tx(pool);
    async move {
        let mut transaction = begin.await?;
        let value = work(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(value)
    }
}

/// The pool, with every statement in a scoped transaction of its own: hand
/// it to a query where the pool used to go.
///
/// ```ignore
/// let name = sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1")
///     .bind(project_id)
///     .fetch_one(db::scoped(&state.pool))
///     .await?;
/// ```
///
/// Each statement begins its transaction with the current scope set (as
/// [`scoped_tx`] does) and commits it once the statement's results are in.
/// A streamed query (`fetch`) reads its rows before the first is yielded,
/// since the transaction has to end; work through a large result set in a
/// [`scoped_tx`] instead.
#[derive(Clone, Copy, Debug)]
pub struct Scoped<'p> {
    pool: &'p PgPool,
    caller: &'static Location<'static>,
}

/// The pool `pool`, scoped: see [`Scoped`].
#[track_caller]
pub fn scoped(pool: &PgPool) -> Scoped<'_> {
    Scoped {
        pool,
        caller: Location::caller(),
    }
}

impl<'p> Executor<'p> for Scoped<'p> {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, sqlx::Error>>
    where
        'p: 'e,
        E: 'q + Execute<'q, Postgres>,
    {
        stream::once(async move {
            let mut transaction = begin_scoped(self.pool, self.caller, "").await?;
            let steps = (&mut *transaction)
                .fetch_many(query)
                .try_collect::<Vec<_>>()
                .await?;
            transaction.commit().await?;
            Ok::<_, sqlx::Error>(stream::iter(steps.into_iter().map(Ok)))
        })
        .try_flatten()
        .boxed()
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<PgRow>, sqlx::Error>>
    where
        'p: 'e,
        E: 'q + Execute<'q, Postgres>,
    {
        Box::pin(async move {
            let mut transaction = begin_scoped(self.pool, self.caller, "").await?;
            let row = (&mut *transaction).fetch_optional(query).await?;
            transaction.commit().await?;
            Ok(row)
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [PgTypeInfo],
    ) -> BoxFuture<'e, Result<PgStatement<'q>, sqlx::Error>>
    where
        'p: 'e,
    {
        self.pool.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<Describe<Postgres>, sqlx::Error>>
    where
        'p: 'e,
    {
        self.pool.describe(sql)
    }
}

/// Counts a call site that began a transaction with no tenancy scope in the
/// metrics (`aw_tenancy_unscoped_sites_total`), and logs it the first time.
fn observe_unscoped(caller: &'static Location<'static>) {
    if crate::metrics::unscoped_call_site(caller.file(), caller.line()) {
        tracing::warn!(
            target: "agent_workspace::tenancy",
            file = caller.file(),
            line = caller.line(),
            "a scoped transaction began with no tenancy scope"
        );
    }
}
