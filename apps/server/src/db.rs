//! Database work in a tenancy scope (H1.20).
//!
//! [`scoped_tx`] begins a transaction whose first statement, sent with
//! `BEGIN` in one round trip, sets `app.tenancy` transaction-locally to the
//! current [`crate::tenancy::Tenancy`]. [`scoped_one`] runs one piece of
//! work in such a transaction and commits it. Nothing is ever set on the
//! session, so a transaction-mode pooler in front of PostgreSQL is safe:
//! the setting ends with the transaction (R24 rule 3).
//!
//! The call sites that still query the pool outside a scoped transaction are
//! counted per directory by the ratchet in `tenancy/ratchet.json` (H1.44),
//! which may only go down.

use std::{future::Future, panic::Location};

use sqlx::{
    PgConnection, PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::tenancy::{self, Tenancy};

/// How the server connects: the database URL, with as many prepared
/// statements per connection as configured (none behind a transaction-mode
/// pooler, K5.17).
pub fn connect_options(config: &crate::config::AppConfig) -> Result<PgConnectOptions, sqlx::Error> {
    Ok(config
        .database_url
        .expose()
        .parse::<PgConnectOptions>()?
        .statement_cache_capacity(config.database_statement_cache))
}

/// The server's pool, sized and connected as configured.
pub async fn connect(config: &crate::config::AppConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect_with(connect_options(config)?)
        .await
}

/// Begins a transaction in the current tenancy scope.
///
/// With no scope at all (a background loop not yet given one), the
/// transaction carries none: in observe mode that is allowed, and the call
/// site is logged once and counted.
#[track_caller]
pub fn scoped_tx(
    pool: &PgPool,
) -> impl Future<Output = Result<Transaction<'static, Postgres>, sqlx::Error>> + '_ {
    let caller = Location::caller();
    async move {
        let tenancy = tenancy::resolved(pool).await?;
        crate::metrics::tenancy_transaction(tenancy.as_ref().map_or("unscoped", Tenancy::kind));
        match tenancy {
            Some(tenancy) => {
                pool.begin_with(format!(
                    "BEGIN; SELECT set_config('app.tenancy', '{}', true)",
                    tenancy.setting()
                ))
                .await
            }
            None => {
                observe_unscoped(caller);
                pool.begin().await
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
