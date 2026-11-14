//! How each module's things leave and re-enter a workspace (H1.8).
//!
//! Every workspace-owned table is declared by its module as a kind of thing
//! ([`KindDecl`]), or listed as not exported with the reason. The
//! declarations live in [`registry`] until M1.3 moves them into
//! `Registrar::export` (R41); nothing about them is in `modules/*.json`.
//!
//! Most kinds need no hand code: [`export_kind`] reads the declared table
//! filtered by its tenant column from `tenancy::tables`, writing every
//! column but the excluded ones, and [`import_kind`] inserts the lines back.
//! The archive around them, id remapping in copy mode and the hand-written
//! parts (live page documents, sheets' cells, whiteboard elements, files)
//! are H1.9's.

pub mod registry;

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::tenancy::tables;

/// One module's export declaration.
#[derive(Debug)]
pub struct ExportDecl {
    pub module: &'static str,
    /// The module's export version; a line carries it as `v`.
    pub version: u32,
    /// Modules whose kinds are imported first.
    pub after: &'static [&'static str],
    pub kinds: &'static [KindDecl],
    /// Workspace tables of this module that never leave, with the reason.
    pub not_exported: &'static [(&'static str, &'static str)],
}

/// One kind of thing: a table, its references and what is left out.
#[derive(Debug)]
pub struct KindDecl {
    pub kind: &'static str,
    pub table: &'static str,
    /// Column and the kind (table) it points at: mapped on import when ids
    /// are copied rather than kept.
    pub references: &'static [(&'static str, &'static str)],
    /// Text columns that may hold ids (in links), rewritten in copy mode.
    pub text_references: &'static [&'static str],
    /// Columns never written: secrets, credential ids, request digests.
    pub exclude: &'static [&'static str],
}

/// What M1's `Registrar::export` slot takes. Until hand-written parts exist
/// (H1.9) every part is its declaration, exported and imported generically.
pub trait ExportPart: Send + Sync {
    fn declaration(&self) -> &ExportDecl;
}

impl ExportPart for ExportDecl {
    fn declaration(&self) -> &ExportDecl {
        self
    }
}

/// Every module's part, in import order.
pub fn parts() -> impl Iterator<Item = &'static dyn ExportPart> {
    registry::DECLARATIONS
        .iter()
        .map(|declaration| declaration as &dyn ExportPart)
}

/// One line of an archive's `things/<module>/<kind>.jsonl`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Line {
    pub kind: String,
    pub v: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("{0} has no workspace column yet (H1.4 to H1.7 add it)")]
    NoWorkspaceColumn(&'static str),
    #[error("{kind} is version {found}; this server reads version {expected}")]
    Version {
        kind: String,
        found: u32,
        expected: u32,
    },
    #[error("a line of kind {found} was given to {expected}")]
    WrongKind {
        found: String,
        expected: &'static str,
    },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn identifier(name: &'static str) -> &'static str {
    assert!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "declared identifiers are plain: {name}"
    );
    name
}

/// Every row of `kind` in `workspace`, as lines, without excluded columns.
pub async fn export_kind(
    connection: &mut PgConnection,
    module: &ExportDecl,
    kind: &KindDecl,
    workspace: Uuid,
) -> Result<Vec<Line>, ExportError> {
    let table = identifier(kind.table);
    let column =
        identifier(tables::tenant_column(table).ok_or(ExportError::NoWorkspaceColumn(table))?);
    let excluded = kind
        .exclude
        .iter()
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    let rows = sqlx::query_scalar::<_, serde_json::Value>(&format!(
        "SELECT to_jsonb(t) - $2::text[] FROM {table} t WHERE t.{column} = $1 \
         ORDER BY to_jsonb(t)->>'id', to_jsonb(t)::text"
    ))
    .bind(workspace)
    .bind(&excluded)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|data| Line {
            kind: kind.kind.to_owned(),
            v: module.version,
            id: data
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            data,
        })
        .collect())
}

/// Inserts `lines` of `kind` as they are (ids kept). Excluded columns come
/// back empty.
pub async fn import_kind(
    connection: &mut PgConnection,
    module: &ExportDecl,
    kind: &KindDecl,
    lines: &[Line],
) -> Result<u64, ExportError> {
    let table = identifier(kind.table);
    for line in lines {
        if line.kind != kind.kind {
            return Err(ExportError::WrongKind {
                found: line.kind.clone(),
                expected: kind.kind,
            });
        }
        if line.v != module.version {
            return Err(ExportError::Version {
                kind: line.kind.clone(),
                found: line.v,
                expected: module.version,
            });
        }
    }
    let rows = serde_json::Value::Array(lines.iter().map(|line| line.data.clone()).collect());
    let inserted = sqlx::query(&format!(
        "INSERT INTO {table} SELECT * FROM jsonb_populate_recordset(NULL::{table}, $1)"
    ))
    .bind(rows)
    .execute(&mut *connection)
    .await?;
    Ok(inserted.rows_affected())
}
