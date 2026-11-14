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
