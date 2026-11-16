//! Shared integration fixtures: the app, its databases, the callers that
//! reach it (a person, an agent, a connected app) and worlds built from
//! them. New tests start here; the older files re-export what moved.
//! Later workstreams build on this, so not every helper has a caller yet.
#![allow(dead_code)]

pub(crate) mod app;
pub(crate) mod callers;
pub(crate) mod db;
pub(crate) mod two_workspaces;
pub(crate) mod world;

/// Runs K1.12's one-time step again, as it ran for agents that existed when
/// suggesting became a permission: every agent holding a role with
/// `pages.edit` moves to that role's "suggests" copy. Tests that set an agent
/// up and then expect its edits to others' pages to be suggestions call this
/// after the setup.
pub(crate) async fn existing_agents_suggest(pool: &sqlx::PgPool) {
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261116223000_suggest_roles.sql"
    ))
    .execute(&mut *transaction)
    .await
    .expect("suggest roles");
    transaction.commit().await.expect("commit");
}
