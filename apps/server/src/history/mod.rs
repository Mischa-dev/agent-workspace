//! History as a seam (K3 3.4). Every kind keeps its own history table, with
//! its own rules; this is the one way to read and restore them: a list of
//! entries, one version's content, and a restore that makes a new version.
//! What a kind can't do yet says so with a reason, never silently.
//!
//! Adapters at the start: pages and whiteboards (list, show, restore),
//! messages (list, show), tasks (list); sheets (list, show, restore) since
//! C1.13; records (list, show, restore) since T1.41.

pub mod cold;
pub mod ops;

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    events::{self, Audience, NewEvent, catalog},
};

/// Whether a kind can do one part of history, and why not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Support {
    Yes,
    Not(&'static str),
}

impl Support {
    pub fn yes(self) -> bool {
        self == Self::Yes
    }
}

/// One kind's history: what it can do.
#[derive(Clone, Copy, Debug)]
pub struct Adapter {
    pub kind: &'static str,
    pub show: Support,
    pub restore: Support,
}

/// The kinds with a history, and what each can do. A kind not here has none
/// yet (conversations, decisions, files, runs, collections).
pub const ADAPTERS: &[Adapter] = &[
    Adapter {
        kind: "page",
        show: Support::Yes,
        restore: Support::Yes,
    },
    Adapter {
        kind: "whiteboard",
        show: Support::Yes,
        restore: Support::Yes,
    },
    Adapter {
        kind: "message",
        show: Support::Yes,
        restore: Support::Not("an edit is the restore: edit the message"),
    },
    // T1.41: a record's changes keep the values they replaced, so a
    // version is now with every later change taken back
    // (`collections::record_versions`).
    Adapter {
        kind: "record",
        show: Support::Yes,
        restore: Support::Yes,
    },
    Adapter {
        kind: "task",
        show: Support::Not("a task's history keeps each change, not whole versions"),
        restore: Support::Not("reverting a task's fields comes later"),
    },
    // C1.13: snapshots and each revision's inputs reach every revision
    // since snapshots began (the deploy that added them).
    Adapter {
        kind: "sheet",
        show: Support::Yes,
        restore: Support::Yes,
    },
];

pub fn adapter(kind: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|adapter| adapter.kind == kind)
}

/// What a kind's history can do, as the operations report it.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCaps {
    pub list: bool,
    pub show_version: bool,
    pub restore: bool,
    pub diff: bool,
}

pub fn caps(kind: &str) -> HistoryCaps {
    match adapter(kind) {
        Some(adapter) => HistoryCaps {
            list: true,
            show_version: adapter.show.yes(),
            restore: adapter.restore.yes(),
            diff: false,
        },
        None => HistoryCaps {
            list: false,
            show_version: false,
            restore: false,
            diff: false,
        },
    }
}

/// One entry in a thing's history, newest first.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// The version (page, message, task timeline) or revision (board,
    /// sheet) this entry made.
    pub version: i64,
    pub at: String,
    /// Who made it.
    #[schemars(with = "Option<String>")]
    pub actor_id: Option<Uuid>,
    /// Their name where it was made.
    pub actor: Option<String>,
    /// The connected app they acted through, when it shows.
    pub via: Option<String>,
    /// Who accepted it, when it was someone's proposal (T1.41).
    pub accepted_by: Option<String>,
    /// In a few words: "Edited", "Restored version 3", "Edited 4 cells".
    pub summary: String,
    /// Keys of what changed ("title", "body", a field id, a cell).
    pub changed: Vec<String>,
}

// Who and through what, for every kind: the account's name in the
// workspace, and the connected app when it shows itself.
const WHO: &str = r#"
    (SELECT COALESCE(profile.display_name, account.display_name)
       FROM actors author
       LEFT JOIN organization_actor_profiles profile
         ON profile.organization_id = $4 AND profile.actor_id = author.id
       LEFT JOIN users account ON account.actor_id = author.id
      WHERE author.id = h.actor_id) AS actor,
    (SELECT client.client_name FROM mcp_oauth_grants grant_row
       JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
      WHERE grant_row.id = h.via_id AND grant_row.show_via) AS via
"#;

fn at(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')")
}

/// Each kind's rows as (version, at, actor_id, via_id, summary, changed),
/// for `$1` the thing, `$2` before (exclusive, or NULL) and `$3` the limit.
fn list_sql(kind: &str) -> Option<String> {
    let rows = match kind {
        "page" => format!(
            r#"
            SELECT version, {at} AS at, actor_id, via_oauth_grant_id AS via_id,
                   NULL::uuid AS accepted_by_id,
                   COALESCE(note, CASE WHEN version = 1 THEN 'Created' ELSE 'Edited' END) AS summary,
                   array_remove(ARRAY[
                       CASE WHEN title IS DISTINCT FROM lag(title) OVER w THEN 'title' END,
                       -- An archived body is compared by its digest (K3.25).
                       CASE WHEN CASE WHEN body IS NOT NULL AND lag(body) OVER w IS NOT NULL
                                      THEN body IS DISTINCT FROM lag(body) OVER w
                                      ELSE COALESCE(body_md5, md5(body))
                                           IS DISTINCT FROM COALESCE(lag(body_md5) OVER w,
                                                                     md5(lag(body) OVER w))
                                 END
                            THEN 'body' END
                   ], NULL) AS changed
            FROM project_page_revisions WHERE page_id = $1
            WINDOW w AS (ORDER BY version)
            "#,
            at = at("created_at")
        ),
        "whiteboard" => format!(
            r#"
            SELECT revision::bigint AS version, {at} AS at, actor_id,
                   via_oauth_grant_id AS via_id, NULL::uuid AS accepted_by_id,
                   COALESCE(note, 'Saved ' || element_count || ' elements') AS summary,
                   ARRAY['scene'] AS changed
            FROM whiteboard_revisions WHERE whiteboard_id = $1
            "#,
            at = at("created_at")
        ),
        "message" => format!(
            r#"
            SELECT revision::bigint AS version, {at} AS at,
                   written_by_actor_id AS actor_id, via_oauth_grant_id AS via_id,
                   NULL::uuid AS accepted_by_id,
                   CASE WHEN redacted_at IS NOT NULL THEN 'Removed'
                        WHEN revision = 1 THEN 'Written' ELSE 'Edited' END AS summary,
                   ARRAY['text'] AS changed
            FROM message_revisions WHERE message_id = $1
            "#,
            at = at("written_at")
        ),
        "record" => format!(
            r#"
            SELECT version, {at} AS at, actor_id, via_oauth_grant_id AS via_id,
                   accepted_by_actor_id AS accepted_by_id,
                   initcap(replace(action, '_', ' ')) AS summary,
                   ARRAY(SELECT change->>'field'
                         FROM jsonb_array_elements(
                             CASE WHEN jsonb_typeof(changes) = 'array' THEN changes
                                  ELSE '[]'::jsonb END) change
                         WHERE change->>'field' IS NOT NULL) AS changed
            FROM collection_record_history WHERE record_id = $1
            "#,
            at = at("created_at")
        ),
        "task" => format!(
            r#"
            SELECT entry.sequence AS version, {at} AS at,
                   COALESCE(activity.actor_id, comment.author_actor_id) AS actor_id,
                   NULL::uuid AS via_id, NULL::uuid AS accepted_by_id,
                   CASE WHEN entry.message_id IS NOT NULL THEN 'Commented'
                        ELSE initcap(replace(COALESCE(activity.kind, entry.kind), '_', ' ')) END
                       AS summary,
                   ARRAY(SELECT jsonb_object_keys(
                       CASE WHEN jsonb_typeof(activity.payload->'changes') = 'object'
                            THEN activity.payload->'changes' ELSE '{{}}'::jsonb END)) AS changed
            FROM work_item_timeline_entries entry
            LEFT JOIN work_item_activity activity ON activity.id = entry.activity_id
            -- visibility: the author of a comment on a task the reader may see
            LEFT JOIN messages comment ON comment.id = entry.message_id
            WHERE entry.work_item_id = $1
            "#,
            at = at("entry.created_at")
        ),
        "sheet" => format!(
            r#"
            SELECT revision AS version, {at} AS at, actor_id, via_oauth_grant_id AS via_id,
                   NULL::uuid AS accepted_by_id,
                   CASE WHEN summary ? 'restoredRevision'
                        THEN 'Restored revision ' || (summary->>'restoredRevision')
                        WHEN summary ? 'created' THEN 'Created'
                        WHEN summary->>'cells' = '0' AND jsonb_typeof(summary->'formats') = 'number'
                             AND summary->>'formats' <> '0'
                        THEN 'Formatted ' || (summary->>'formats') || ' cells'
                        WHEN jsonb_typeof(summary->'cells') = 'number'
                        THEN 'Edited ' || (summary->>'cells') || ' cells'
                        ELSE 'Edited' END AS summary,
                   ARRAY(SELECT key FROM jsonb_object_keys(
                       CASE WHEN jsonb_typeof(summary->'cells') = 'object'
                            THEN summary->'cells' ELSE '{{}}'::jsonb END) key
                         ORDER BY key LIMIT 64) AS changed
            FROM sheet_revisions WHERE sheet_id = $1
            "#,
            at = at("created_at")
        ),
        _ => return None,
    };
    Some(format!(
        r#"
        SELECT h.version, h.at, h.actor_id, h.summary, h.changed, {WHO},
               (SELECT COALESCE(profile.display_name, account.display_name)
                  FROM actors accepter
                  LEFT JOIN organization_actor_profiles profile
                    ON profile.organization_id = $4 AND profile.actor_id = accepter.id
                  LEFT JOIN users account ON account.actor_id = accepter.id
                 WHERE accepter.id = h.accepted_by_id) AS accepted_by
        FROM ({rows}) h
        WHERE $2::bigint IS NULL OR h.version < $2
        ORDER BY h.version DESC
        LIMIT $3
        "#
    ))
}

/// A thing's history, newest first. The caller has checked the viewer sees
/// the thing.
pub async fn list(
    connection: &mut PgConnection,
    kind: &str,
    id: Uuid,
    workspace_id: Uuid,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<HistoryEntry>, AppError> {
    let Some(sql) = list_sql(kind) else {
        return Ok(Vec::new());
    };
    Ok(sqlx::query_as::<_, HistoryEntry>(&sql)
        .bind(id)
        .bind(before)
        .bind(limit.clamp(1, 200))
        .bind(workspace_id)
        .fetch_all(connection)
        .await?)
}

/// One version as it was, or None when there is no such version. Kinds
/// that can't show a version are refused before this is called.
pub async fn show(
    connection: &mut PgConnection,
    kind: &str,
    id: Uuid,
    version: i64,
) -> Result<Option<Value>, AppError> {
    Ok(match kind {
        "page" => sqlx::query_as::<_, (String, String)>(
            "SELECT title, body FROM project_page_revisions WHERE page_id = $1 AND version = $2",
        )
        .bind(id)
        .bind(version)
        .fetch_optional(connection)
        .await?
        .map(|(title, body)| json!({ "title": title, "body": body })),
        "whiteboard" => sqlx::query_as::<_, (Value, i32, i64)>(
            "SELECT elements, element_count, scene_seq FROM whiteboard_revisions \
             WHERE whiteboard_id = $1 AND revision = $2",
        )
        .bind(id)
        .bind(i32::try_from(version).unwrap_or(-1))
        .fetch_optional(connection)
        .await?
        .map(|(elements, count, scene_seq)| {
            json!({ "elements": elements, "elementCount": count, "sceneSeq": scene_seq })
        }),
        // A removed message's versions are gone with its text.
        "sheet" => crate::sheets::history::state_at(connection, id, version)
            .await?
            .map(|state| state.to_json(version)),
        "message" => sqlx::query_as::<_, (Option<String>, Option<Value>)>(
            "SELECT plain_text, content FROM message_revisions \
             WHERE message_id = $1 AND revision = $2",
        )
        .bind(id)
        .bind(i32::try_from(version).unwrap_or(-1))
        .fetch_optional(connection)
        .await?
        .map(|(text, content)| match text {
            Some(text) => json!({ "text": text, "content": content }),
            None => json!({ "removed": true }),
        }),
        _ => None,
    })
}

/// Who restores, as each kind's own write path wants it.
pub struct Restorer {
    pub page: crate::memory::Writer,
    pub board: crate::whiteboards::Caller,
    pub space_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
}

/// Put back `version` as a new version; the thing's version after. Refuses
/// when `expected_version` isn't the thing's version now (a page's version,
/// a board's scene sequence). Kinds that can't restore are refused before.
pub async fn restore(
    transaction: &mut Transaction<'_, Postgres>,
    restorer: &Restorer,
    kind: &str,
    id: Uuid,
    version: i64,
    expected_version: i64,
) -> Result<i64, AppError> {
    let (ty, now) = match kind {
        "page" => {
            let input = crate::memory::PageInput {
                restore_version: Some(version),
                expected_version: Some(expected_version),
                ..Default::default()
            };
            crate::memory::update_page(transaction, restorer.page, id, &input).await?;
            let now =
                sqlx::query_scalar::<_, i64>("SELECT version FROM project_pages WHERE id = $1")
                    .bind(id)
                    .fetch_one(&mut **transaction)
                    .await?;
            (&catalog::PAGE_VERSION_RESTORED, now)
        }
        "whiteboard" => {
            let current = sqlx::query_scalar::<_, i64>(
                "SELECT scene_seq FROM whiteboards WHERE id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("whiteboard"))?;
            if current != expected_version {
                return Err(AppError::VersionConflict {
                    resource: "whiteboard",
                    current_version: current,
                });
            }
            let revision =
                i32::try_from(version).map_err(|_| AppError::NotFound("whiteboard revision"))?;
            crate::whiteboards::access(transaction, &restorer.board, id, true).await?;
            let outcome = crate::whiteboards::store::restore_revision(
                transaction,
                &restorer.board,
                id,
                revision,
            )
            .await?;
            (&catalog::WHITEBOARD_VERSION_RESTORED, outcome.scene_seq)
        }
        "sheet" => {
            let space =
                sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM sheets WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut **transaction)
                    .await?
                    .ok_or(AppError::NotFound("sheet"))?;
            let sheet = crate::sheets::lock(transaction, space, id)
                .await?
                .ok_or(AppError::NotFound("sheet"))?;
            if sheet.revision != expected_version {
                return Err(AppError::VersionConflict {
                    resource: "sheet",
                    current_version: sheet.revision,
                });
            }
            if sheet.archived {
                return Err(AppError::Conflict("sheet", "archived"));
            }
            let applied = match crate::sheets::history::restore(
                transaction,
                &sheet,
                restorer.actor_id,
                version,
            )
            .await?
            {
                Ok(applied) => applied,
                Err(crate::sheets::history::Refusal::NotKept) => {
                    return Err(AppError::Refused {
                        field: "version",
                        message: "That revision is from before the sheet's history was kept; restore a later one.",
                    });
                }
                Err(crate::sheets::history::Refusal::Problem(_)) => {
                    return Err(AppError::Refused {
                        field: "version",
                        message: "That revision can't be put back as the sheet is now (a tab name or a limit is in the way).",
                    });
                }
            };
            sqlx::query(
                "UPDATE sheet_revisions SET summary = summary || jsonb_build_object('restoredRevision', $3::bigint) \
                 WHERE sheet_id = $1 AND revision = $2",
            )
            .bind(id)
            .bind(applied.revision)
            .bind(version)
            .execute(&mut **transaction)
            .await?;
            crate::outbox::enqueue_sheet_updated(transaction, space, id, applied.revision).await?;
            (&catalog::SHEET_VERSION_RESTORED, applied.revision)
        }
        _ => return Err(AppError::Internal("restore on a kind that has none")),
    };
    // A board across a workspace is heard by the workspace.
    let mut event = NewEvent::thing(ty, restorer.space_id.unwrap_or_default(), kind, id)
        .version(now)
        .actor(restorer.actor_id)
        .workspace(restorer.workspace_id)
        .payload(json!({ "restoredVersion": version }));
    if restorer.space_id.is_none() {
        event.space_id = None;
        event = event.audience(Audience::Workspace);
    }
    events::emit(transaction, event).await?;
    Ok(now)
}
