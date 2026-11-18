//! Where a package to preview or install comes from (M1.11): written
//! inline, an entry of the directory (built in, a server or workspace
//! source, or a saved template), or an uploaded file (a zipped package or
//! its JSON).

use serde_json::Value;

use super::{directory, format::Package, install};
use crate::ops::{Edge, OpCall, OpError, refs};

/// A package read for an install, and where it came from.
pub struct Origin {
    pub package: Package,
    /// Its stored form.
    pub document: Value,
    /// `packages.source`.
    pub source: &'static str,
    pub source_ref: Option<String>,
    /// A saved template's id and name, when that's what it is.
    pub template: Option<(String, String)>,
}

/// The most bytes of an uploaded package read.
const MAX_UPLOAD_BYTES: u64 = super::zipped::MAX_ZIP_BYTES as u64;

/// Reads the one package source given: `package`, `entry` or `file_id`.
pub async fn resolve(
    call: &mut OpCall<'_>,
    package: Option<&Value>,
    entry: Option<&str>,
    file_id: Option<&str>,
) -> Result<Origin, OpError> {
    let given = [package.is_some(), entry.is_some(), file_id.is_some()]
        .iter()
        .filter(|given| **given)
        .count();
    if given != 1 {
        return Err(OpError::invalid(
            "package",
            "Give the package one way: inline (package), from the directory (entry, from \
             directory_list) or as an uploaded file (fileId).",
        ));
    }
    if let Some(document) = package {
        return Ok(Origin {
            package: install::read(document)?,
            document: document.clone(),
            source: "upload",
            source_ref: None,
            template: None,
        });
    }
    if let Some(file_id) = file_id {
        return uploaded(call, file_id).await;
    }
    let entry = entry.unwrap_or_default();
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    match directory::find(tx, workspace, entry).await? {
        Ok(found) => Ok(Origin {
            package: found.package,
            document: found.document,
            source: found.source,
            source_ref: Some(found.source_ref),
            template: found.template,
        }),
        Err(message) => Err(OpError::Invalid {
            field: "entry".into(),
            message: message.into(),
        }),
    }
}

/// An uploaded file the caller can read, in this workspace, as a package.
/// Read before the call's transaction starts: the file store may be
/// across a network.
async fn uploaded(call: &mut OpCall<'_>, file_id: &str) -> Result<Origin, OpError> {
    let not_found = || OpError::not_found("That file");
    let id = refs::parse_id(file_id).ok_or_else(not_found)?;
    let ctx = call.ctx;
    let reader = match ctx.edge {
        Edge::Web { .. } => crate::files::Reader::Browser {
            actor_id: ctx.caller.account_id,
        },
        _ => crate::files::Reader::Mcp {
            actor_id: ctx.caller.account_id,
            connection_id: ctx.caller.connection_id,
            confined_project: ctx.caller.confined_space,
        },
    };
    let state = call.state;
    let row = match crate::files::readable_file(&state.pool, reader, id).await {
        Ok(row) => row,
        Err(crate::error::AppError::NotFound(_)) => return Err(not_found()),
        Err(error) => return Err(OpError::logged("package_file", &error)),
    };
    if row.byte_size as u64 > MAX_UPLOAD_BYTES {
        return Err(OpError::invalid(
            "fileId",
            "A package file is at most 5 MB.",
        ));
    }
    let bytes = crate::files::store::FileStore::from_config(&state.config)
        .read_all(&row.storage_key, MAX_UPLOAD_BYTES)
        .await
        .map_err(|error| OpError::logged("package_file", &error))?;
    let tx = call.tx().await?;
    let workspace =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(row.project_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|error| OpError::logged("package_file", &error.into()))?;
    if workspace != ctx.workspace_id {
        return Err(not_found());
    }
    let document = directory::document_of(&bytes).map_err(|message| OpError::Invalid {
        field: "fileId".into(),
        message: message.into(),
    })?;
    Ok(Origin {
        package: install::read(&document)?,
        document,
        source: "upload",
        source_ref: Some(format!("file:{id}")),
        template: None,
    })
}
