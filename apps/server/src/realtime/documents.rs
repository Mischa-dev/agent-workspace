//! Documents a live connection may watch (K5.10): pages, whiteboards and
//! sheets. `can_view` is a stand-in for C1's `documents::can_view` until it
//! lands: the thing registry's own resolver for the kind, as the account
//! sees it now (the space it is in, its modules), so a document someone
//! can't open can't be watched either.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

/// A document by kind and id, as the wire names it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRef {
    pub kind: DocumentKind,
    pub id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Page,
    Whiteboard,
    Sheet,
}

impl DocumentKind {
    fn thing_kind(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Whiteboard => "whiteboard",
            Self::Sheet => "sheet",
        }
    }
}

/// Whether `account` may see the document now.
pub async fn can_view(
    pool: &PgPool,
    account: Uuid,
    document: DocumentRef,
) -> Result<bool, AppError> {
    if document.kind == DocumentKind::Whiteboard {
        return crate::whiteboards::visible_to(pool, document.id, account).await;
    }
    // A socket is served for no account in particular, so the row rules don't
    // hide a document kept to chosen people: ask the Docs rule by name.
    if !crate::docs::open_to(pool, document.id, account).await? {
        return Ok(false);
    }
    // A page shared with them on its own (K1.32) is watched like any other.
    if document.kind == DocumentKind::Page
        && crate::access::sharing::page_level(pool, account, document.id)
            .await?
            .is_some()
    {
        return Ok(true);
    }
    let mut connection = crate::db::scoped_tx(pool).await?;
    let Some(located) = crate::things::locate(&mut connection, document.id).await? else {
        return Ok(false);
    };
    if located.deleted || located.kind.as_str() != document.kind.thing_kind() {
        return Ok(false);
    }
    let Some(space) = located.space_id else {
        return Ok(false);
    };
    // Only in a space it can open now.
    if !crate::search::reachable_projects(pool, account, None)
        .await?
        .contains(&space)
    {
        return Ok(false);
    }
    let Some(modules) = crate::modules::state::for_project(&mut connection, space, account).await?
    else {
        return Ok(false);
    };
    let projects = [space];
    let workspaces = BTreeMap::new();
    let viewer = crate::things::Viewer {
        actor_id: account,
        projects: &projects,
        task_projects: &[],
        modules,
        workspaces: &workspaces,
        addresses: crate::addresses::Addresses::LEGACY,
    };
    Ok(crate::things::get(&mut connection, &viewer, document.id)
        .await?
        .is_some_and(|thing| thing.kind == document.kind.thing_kind()))
}
