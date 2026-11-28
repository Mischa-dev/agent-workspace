//! Moving a page to another space (C1.37): `page_move`. The page keeps its
//! id, versions, comments, files, proposals and change sets; access is the
//! new space's from the moment the move commits, so a reader of the old
//! space who isn't in the new one loses it at once. Links and embeds name
//! the page by id and keep opening it for whoever can see it. This is how a
//! draft in someone's personal space is published.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::Writer;
use crate::{
    access::{self, Need},
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// Move a page to another space.
pub struct PageMove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageMoveInput {
    /// The page: its id or link.
    pub page_id: IdRef,
    /// The space it goes to: its id, link or key.
    #[serde(alias = "toProjectId")]
    pub to_space_id: IdRef,
    /// The page's version as you last read it.
    pub expected_version: i64,
    /// Put it inside this page where it goes; left out, at the top. The
    /// pages inside it go with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_page_id: Option<IdRef>,
}

impl Targeted for PageMoveInput {
    fn target(&self) -> Target {
        Target::thing("page", self.page_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PageMoved {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub version: i64,
}

impl Operation for PageMove {
    const NAME: &'static str = "page_move";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Move a page to another space";
    const DESCRIPTION: &'static str = "Move a page to another space in the workspace, such as a \
        draft from your own space to a team's. It keeps its versions, comments and files, and \
        links to it keep working; who can see it is the new space's. The pages inside it go with \
        it. Needs edit on the page and on pages where it goes.";
    /// Staged by the page's trigger as its row changes.
    const EMITS: &'static [&'static str] = &["page.updated"];

    type Input = PageMoveInput;
    type Output = PageMoved;

    async fn run(call: &mut OpCall<'_>, input: PageMoveInput) -> Result<PageMoved, OpError> {
        let page_id = input.page_id.id();
        let to = input.to_space_id.id();
        let workspace_id = call.ctx.workspace_id;
        let writer = Writer::from(call.ctx);
        let caller = call.ctx.caller.clone();
        let authorized = call.ctx.space_id;
        let tx = call.tx().await?;
        // Where it goes is checked as where it is: the whole caller, with
        // edit on pages there. A space the caller can't reach reads as
        // missing.
        let located = crate::ops::target::locate(
            tx,
            &Target::Space(to),
            caller.account_id,
            Some(workspace_id),
        )
        .await
        .map_err(|_| OpError::not_found("That space"))?;
        if located.workspace_id != workspace_id {
            return Err(OpError::not_found("That space"));
        }
        access::check_locked(
            tx,
            &caller,
            Need::permission(Perm::PagesEdit).writes(),
            access::Target::Space(to),
        )
        .await?
        .map(|_| ())
        .map_err(|_| OpError::not_found("That space"))?;
        let target_archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM projects WHERE id = $1 FOR SHARE",
        )
        .bind(to)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That space"))?;
        if target_archived {
            return Err(OpError::invalid(
                "toSpaceId",
                "That space is archived. Bring it back first.",
            ));
        }

        // Both trees change: take them before the page (see lock_page_trees).
        let trees = authorized.map_or_else(|| vec![to], |from| vec![from, to]);
        super::lock_page_trees(tx, &trees).await?;
        let (from, version, title, archived, is_guide) =
            sqlx::query_as::<_, (Uuid, i64, String, bool, bool)>(
                "SELECT project_id, version, title, archived_at IS NOT NULL, is_guide \
                 FROM project_pages WHERE id = $1 FOR UPDATE",
            )
            .bind(page_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?
            .ok_or_else(|| OpError::not_found("That page"))?;
        // The call was allowed in the space the page was in when it was
        // resolved; a move of a page above it that landed since (which
        // carries this one along without a new version) leaves it no say.
        if authorized != Some(from) {
            return Err(OpError::not_found("That page"));
        }
        if version != input.expected_version {
            return Err(AppError::VersionConflict {
                resource: "page",
                current_version: version,
            }
            .into());
        }
        if from == to {
            return Err(OpError::invalid("toSpaceId", "The page is already there."));
        }
        if archived {
            return Err(OpError::invalid(
                "pageId",
                "That page is archived. Bring it back first.",
            ));
        }
        if is_guide {
            return Err(OpError::invalid(
                "pageId",
                "A space's guide stays with it. Make another page the guide first.",
            ));
        }
        // The pages inside it go with it (C1.38).
        let moving = sqlx::query_scalar::<_, Uuid>(
            r#"
            WITH RECURSIVE down(id, depth) AS (
                SELECT id, 0 FROM project_pages WHERE id = $1
                UNION ALL
                SELECT page.id, down.depth + 1
                FROM project_pages page JOIN down ON page.parent_page_id = down.id
                WHERE down.depth < 50
            )
            SELECT id FROM down
            "#,
        )
        .bind(page_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        // A page inside that is kept from the mover would be carried along
        // unseen, with its comments and files left behind: it stays put.
        let unseen = sqlx::query_scalar::<_, bool>(
            r#"
            WITH RECURSIVE down(id, depth) AS (
                SELECT $1::uuid, 0
                UNION ALL
                SELECT place.thing_id, down.depth + 1
                FROM doc_places place JOIN down ON place.parent_id = down.id
                WHERE down.depth < 50
            )
            SELECT EXISTS (SELECT 1 FROM down WHERE id <> ALL($2))
            "#,
        )
        .bind(page_id)
        .bind(&moving)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if unseen {
            return Err(OpError::invalid(
                "pageId",
                "A page inside this one is kept to chosen people, and you aren't one of them. \
                 Someone who can see every page inside it can move it.",
            ));
        }
        let clash = sqlx::query_scalar::<_, String>(
            "SELECT moving.title FROM project_pages moving \
             JOIN project_pages there ON there.project_id = $2 \
              AND lower(there.title) = lower(moving.title) AND there.archived_at IS NULL \
             WHERE moving.id = ANY($1) AND moving.archived_at IS NULL LIMIT 1",
        )
        .bind(&moving)
        .bind(to)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if clash.is_some() {
            return Err(OpError::invalid(
                "toSpaceId",
                "That space already has a page with a title this one (or a page inside it) has. \
                 Rename one of them first.",
            ));
        }
        let guide_inside = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_pages \
             WHERE id = ANY($1) AND is_guide AND archived_at IS NULL)",
        )
        .bind(&moving)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if guide_inside {
            return Err(OpError::invalid(
                "pageId",
                "The space's guide is inside this page, and a guide stays with its space.",
            ));
        }
        let parent = input.parent_page_id.map(|parent| parent.id());
        if let Some(parent) = parent {
            super::check_parent(tx, to, Some(page_id), parent).await?;
        }
        let position = super::next_position(tx, to, parent).await?;

        // Where someone last was in the old space isn't a place in the new one.
        sqlx::query("DELETE FROM project_navigation_states WHERE page_id = ANY($1)")
            .bind(&moving)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        // The live documents and the pages inside follow by their keys (ON
        // UPDATE CASCADE).
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE project_pages \
             SET project_id = $2, parent_page_id = $4, position = $5, version = version + 1, \
                 updated_by_actor_id = $3, updated_at = now() \
             WHERE id = $1 RETURNING version",
        )
        .bind(page_id)
        .bind(to)
        .bind(writer.actor_id)
        .bind(parent)
        .bind(position)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let body = sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1")
            .bind(page_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
        super::record_revision(
            tx,
            writer,
            page_id,
            version,
            &title,
            &body,
            Some("Moved to another space"),
            None,
        )
        .await?;
        for moved in &moving {
            crate::discussion::follow(tx, crate::discussion::SubjectKind::Page, *moved, to).await?;
            follow_space(tx, "page", *moved, to).await?;
        }
        call.audit_target("page", page_id, json!({"from": from, "to": to}));
        Ok(PageMoved {
            id: page_id,
            space_id: to,
            version,
        })
    }
}

/// What hangs off a document by its kind and id follows it to `space_id`:
/// its own files, its proposals, its change sets and the references its
/// text makes. Called in the transaction that moves the document, after its
/// own row and its discussion.
pub(crate) async fn follow_space(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    id: Uuid,
    space_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("UPDATE files SET project_id = $3 WHERE thing_kind = $1 AND thing_id = $2")
        .bind(kind)
        .bind(id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE proposals SET space_id = $3 WHERE thing_kind = $1 AND thing_id = $2")
        .bind(kind)
        .bind(id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE document_change_sets SET space_id = $3 WHERE thing_kind = $1 AND thing_id = $2",
    )
    .bind(kind)
    .bind(id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    // Links name things, not spaces: they need nothing.
    Ok(())
}
