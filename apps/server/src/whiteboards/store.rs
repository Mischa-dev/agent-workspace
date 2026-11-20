//! A board's stored scene: the element rows, their merge, the images, and
//! the saved revisions. Browsers and agents write through `write_elements`
//! alone, so every change merges the same way, bumps the board's `scene_seq`
//! once, and reaches open boards through the outbox.

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use rmcp::schemars;
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    Caller,
    elements::{self, Element, MAX_ELEMENTS},
    scene::{self, Object},
};
use crate::{
    AppState,
    error::AppError,
    files,
    outbox::{OutboxEventPayload, WhiteboardUpdatedPayload, enqueue_typed},
};

/// Most image bytes one board keeps.
pub const MAX_BOARD_FILE_BYTES: i64 = 64 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// A revision is saved before an edit when the last one is older than this.
const REVISION_INTERVAL_MINUTES: i32 = 10;
/// Saved versions older than this have their elements moved to the file
/// store by the compaction job (C1.34); they still open and restore.
const COMPACT_AFTER_DAYS: i32 = 30;
/// Versions moved per compaction run.
const COMPACT_BATCH: i64 = 200;
const COMPACT_KIND: &str = "history.compact";

/// Tells open boards (and the boards list) that a board changed.
pub async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
) -> Result<(), AppError> {
    let (project_id, scene_seq, version) = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT project_id, scene_seq, version FROM whiteboards WHERE id = $1",
    )
    .bind(whiteboard_id)
    .fetch_one(&mut **transaction)
    .await?;
    // Every board lives in a space (K4.10); realtime delivers its events by
    // whiteboard_visible_to.
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::WhiteboardUpdated(WhiteboardUpdatedPayload {
            whiteboard_id,
            scene_seq,
            version,
        }),
    )
    .await?;
    Ok(())
}

/// The board's own fact when an edit kept a new revision (K3.5): once per
/// edit session, which rules and webhooks hear. A workspace board's fact
/// belongs to the workspace and is heard by whoever may see the board.
pub async fn updated(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    revision: i32,
) -> Result<(), AppError> {
    use crate::events::{Audience, NewEvent, catalog};
    let (organization_id, project_id) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        "SELECT organization_id, project_id FROM whiteboards WHERE id = $1",
    )
    .bind(whiteboard_id)
    .fetch_one(&mut **transaction)
    .await?;
    let event = match project_id {
        Some(project_id) => NewEvent::thing(
            &catalog::WHITEBOARD_UPDATED,
            project_id,
            "whiteboard",
            whiteboard_id,
        ),
        None => NewEvent {
            space_id: None,
            ..NewEvent::thing(
                &catalog::WHITEBOARD_UPDATED,
                Uuid::nil(),
                "whiteboard",
                whiteboard_id,
            )
            .workspace(organization_id)
            .audience(Audience::Thing)
        },
    };
    crate::events::emit(
        transaction,
        event.version(i64::from(revision)).actor(caller.actor_id),
    )
    .await?;
    Ok(())
}

/// What a write did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOutcome {
    /// The board's change counter after the write.
    pub scene_seq: i64,
    /// Elements stored as sent.
    pub accepted: Vec<String>,
    /// Elements where the stored copy won (a later version, or the tie
    /// rule): the stored copy, so the sender can take it.
    pub rejected: Vec<Value>,
}

/// Locks a board for a scene write and returns its change counter. Writes to
/// one board run one at a time; the lock is held until the caller commits.
pub async fn lock(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
) -> Result<i64, AppError> {
    sqlx::query_scalar::<_, i64>("SELECT scene_seq FROM whiteboards WHERE id = $1 FOR UPDATE")
        .bind(whiteboard_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("whiteboard"))
}

/// Merges elements into a board the caller has locked (see `lock`) and may
/// edit. Each element is kept or refused on its own by Excalidraw's rule
/// (`elements::incoming_wins`), so a stale copy of one shape never undoes
/// someone's change to it and never touches any other shape.
pub async fn write_elements(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    scene_seq: i64,
    incoming: Vec<Element>,
) -> Result<WriteOutcome, AppError> {
    crate::events::ensure_actor(transaction, caller.actor_id).await?;
    crate::domain::set_via_grant(transaction, caller.via_grant_id).await?;
    let mut outcome = WriteOutcome {
        scene_seq,
        ..WriteOutcome::default()
    };
    if incoming.is_empty() {
        return Ok(outcome);
    }
    // The last copy of an id in one batch is the one that counts.
    let mut latest = HashMap::new();
    for element in incoming {
        latest.insert(element.id.clone(), element);
    }
    let ids = latest.keys().cloned().collect::<Vec<_>>();
    let stored = sqlx::query_as::<_, (String, i32, i64, Value)>(
        r#"
        SELECT element_id, version, version_nonce, data FROM whiteboard_elements
        WHERE whiteboard_id = $1 AND element_id = ANY($2)
        "#,
    )
    .bind(whiteboard_id)
    .bind(&ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, version, nonce, data)| (id, (version, nonce, data)))
    .collect::<HashMap<_, _>>();
    let new_rows = ids.iter().filter(|id| !stored.contains_key(*id)).count() as i64;
    if new_rows > 0 {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM whiteboard_elements WHERE whiteboard_id = $1",
        )
        .bind(whiteboard_id)
        .fetch_one(&mut **transaction)
        .await?;
        if count + new_rows > MAX_ELEMENTS {
            return Err(AppError::Conflict(
                "whiteboard_full",
                "This board holds as many elements as it can. Delete some, or start another board.",
            ));
        }
    }
    let next_seq = scene_seq + 1;
    let mut accepted = Vec::new();
    for (id, element) in latest {
        match stored.get(&id) {
            Some((version, nonce, data))
                if !elements::incoming_wins(
                    (element.version, element.version_nonce),
                    (*version, *nonce),
                ) =>
            {
                // The same copy echoed back is neither news nor a conflict.
                if (element.version, element.version_nonce) != (*version, *nonce) {
                    outcome.rejected.push(data.clone());
                }
            }
            _ => accepted.push(element),
        }
    }
    if accepted.is_empty() {
        return Ok(outcome);
    }
    accepted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut element_ids = Vec::new();
    let mut kinds = Vec::new();
    let mut versions = Vec::new();
    let mut nonces = Vec::new();
    let mut deleted = Vec::new();
    let mut indices = Vec::new();
    let mut data = Vec::new();
    for element in &accepted {
        element_ids.push(element.id.clone());
        kinds.push(element.kind.clone());
        versions.push(element.version);
        nonces.push(element.version_nonce);
        deleted.push(element.is_deleted);
        indices.push(element.index.clone());
        data.push(
            serde_json::to_string(&element.data).map_err(|_| AppError::Internal("element json"))?,
        );
    }
    sqlx::query(
        r#"
        INSERT INTO whiteboard_elements (
            whiteboard_id, element_id, element_type, version, version_nonce, is_deleted,
            sort_index, data, seq, updated_by_actor_id, updated_at
        )
        SELECT $1, incoming.element_id, incoming.element_type, incoming.version,
               incoming.version_nonce, incoming.is_deleted, incoming.sort_index,
               incoming.data::jsonb, $9, $10, now()
        FROM unnest($2::text[], $3::text[], $4::int[], $5::bigint[], $6::bool[], $7::text[], $8::text[])
          AS incoming(element_id, element_type, version, version_nonce, is_deleted, sort_index, data)
        ON CONFLICT (whiteboard_id, element_id) DO UPDATE SET
            element_type = EXCLUDED.element_type, version = EXCLUDED.version,
            version_nonce = EXCLUDED.version_nonce, is_deleted = EXCLUDED.is_deleted,
            sort_index = EXCLUDED.sort_index, data = EXCLUDED.data, seq = EXCLUDED.seq,
            updated_by_actor_id = EXCLUDED.updated_by_actor_id, updated_at = now()
        "#,
    )
    .bind(whiteboard_id)
    .bind(&element_ids)
    .bind(&kinds)
    .bind(&versions)
    .bind(&nonces)
    .bind(&deleted)
    .bind(&indices)
    .bind(&data)
    .bind(next_seq)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE whiteboards board
        SET scene_seq = $2, updated_by_actor_id = $3, updated_at = now(), scene_updated_at = now(),
            element_count = (
                SELECT count(*) FROM whiteboard_elements element
                WHERE element.whiteboard_id = board.id AND NOT element.is_deleted
            ),
            text_content = left(COALESCE((
                SELECT string_agg(
                    COALESCE(element.data->>'originalText', element.data->>'text', element.data->>'name'),
                    E'\n' ORDER BY element.sort_index COLLATE "C", element.element_id
                )
                FROM whiteboard_elements element
                WHERE element.whiteboard_id = board.id AND NOT element.is_deleted
                  AND element.element_type IN ('text', 'frame', 'magicframe')
            ), ''), 20000)
        WHERE board.id = $1
        "#,
    )
    .bind(whiteboard_id)
    .bind(next_seq)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    let touched = accepted
        .iter()
        .map(|element| crate::change_sets::ElementBefore {
            id: element.id.clone(),
            before: stored.get(&element.id).map(|(_, _, data)| data.clone()),
            version: element.version,
        })
        .collect();
    crate::change_sets::record_board(
        transaction,
        caller.actor_id,
        caller.via_grant_id,
        whiteboard_id,
        (scene_seq, next_seq),
        touched,
        // Drawing in the board's own editor comes in many small writes.
        caller.channel == "browser",
    )
    .await?;
    notify(transaction, whiteboard_id).await?;
    outcome.scene_seq = next_seq;
    outcome.accepted = element_ids;
    Ok(outcome)
}

/// Saves the current scene as a revision when the last one is older than
/// the interval (or `force`), so the state before an edit session, a
/// restore or a big removal can always be brought back.
pub async fn save_revision(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    force: bool,
    note: Option<&str>,
) -> Result<Option<i32>, AppError> {
    if !force {
        let recent = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM whiteboard_revisions
                WHERE whiteboard_id = $1 AND created_at > now() - make_interval(mins => $2)
            ) OR NOT EXISTS (
                SELECT 1 FROM whiteboard_elements WHERE whiteboard_id = $1 AND NOT is_deleted
            )
            "#,
        )
        .bind(whiteboard_id)
        .bind(REVISION_INTERVAL_MINUTES)
        .fetch_one(&mut **transaction)
        .await?;
        if recent {
            return Ok(None);
        }
    }
    let revision = sqlx::query_scalar::<_, i32>(
        r#"
        INSERT INTO whiteboard_revisions (
            whiteboard_id, revision, scene_seq, elements, element_count, actor_id,
            via_oauth_grant_id, note
        )
        SELECT board.id,
               COALESCE((SELECT max(revision) FROM whiteboard_revisions WHERE whiteboard_id = board.id), 0) + 1,
               board.scene_seq,
               COALESCE((
                   SELECT jsonb_agg(element.data ORDER BY element.sort_index COLLATE "C", element.element_id)
                   FROM whiteboard_elements element
                   WHERE element.whiteboard_id = board.id AND NOT element.is_deleted
               ), '[]'::jsonb),
               board.element_count, $2, $3, $4
        FROM whiteboards board WHERE board.id = $1
        RETURNING revision
        "#,
    )
    .bind(whiteboard_id)
    .bind(caller.actor_id)
    .bind(caller.via_grant_id)
    .bind(note)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Some(revision))
}

/// A saved version's elements, from the row or, once the compaction job
/// moved them, from the file store. None when there is no such version.
pub async fn revision_elements(
    store: &files::FileStore,
    connection: &mut sqlx::PgConnection,
    whiteboard_id: Uuid,
    revision: i32,
) -> Result<Option<Value>, AppError> {
    let Some((elements, key)) = sqlx::query_as::<_, (Value, Option<String>)>(
        "SELECT elements, payload_key FROM whiteboard_revisions WHERE whiteboard_id = $1 AND revision = $2",
    )
    .bind(whiteboard_id)
    .bind(revision)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    let Some(key) = key else {
        return Ok(Some(elements));
    };
    let bytes = store.read_all(&key, MAX_PAYLOAD_BYTES).await?;
    let mut json = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(bytes.as_slice()),
        &mut json,
    )
    .map_err(|_| AppError::Internal("a moved board version couldn't be read"))?;
    Ok(Some(serde_json::from_slice(&json).map_err(|_| {
        AppError::Internal("a moved board version couldn't be read")
    })?))
}

/// The largest moved version read back, compressed.
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

pub fn register_compaction(registry: &mut crate::jobs::JobRegistry) {
    registry.register(
        COMPACT_KIND,
        |context: crate::jobs::JobContext| async move {
            compact_revisions(&context.state)
                .await
                .map_err(|error| crate::jobs::JobError::retry(error.log_category()))?;
            // Sheets' old snapshots go the same way.
            crate::sheets::history::compact_snapshots(&context.state)
                .await
                .map_err(|error| crate::jobs::JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
    registry.every(COMPACT_KIND, std::time::Duration::from_secs(6 * 60 * 60));
}

/// Moves the elements of board versions older than 30 days into the file
/// store, compressed, a batch per run (C1.34). The row keeps everything
/// else; the version still opens and restores. Returns how many moved.
pub async fn compact_revisions(state: &AppState) -> Result<u64, AppError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid, i32)>(
        r#"
        SELECT board.organization_id, revision.whiteboard_id, revision.revision
        FROM whiteboard_revisions revision
        JOIN whiteboards board ON board.id = revision.whiteboard_id
        WHERE revision.payload_key IS NULL
          AND revision.created_at < now() - make_interval(days => $1)
          AND jsonb_array_length(revision.elements) > 0
        ORDER BY revision.created_at
        LIMIT $2
        "#,
    )
    .bind(COMPACT_AFTER_DAYS)
    .bind(COMPACT_BATCH)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let store = files::FileStore::from_config(&state.config);
    let mut moved = 0;
    for (workspace, board, revision) in due {
        let elements = sqlx::query_scalar::<_, Value>(
            "SELECT elements FROM whiteboard_revisions WHERE whiteboard_id = $1 AND revision = $2",
        )
        .bind(board)
        .bind(revision)
        .fetch_one(crate::db::scoped(&state.pool))
        .await?;
        let json = serde_json::to_vec(&elements)
            .map_err(|_| AppError::Internal("a board version couldn't be written"))?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &json)
            .map_err(|_| AppError::Internal("a board version couldn't be compressed"))?;
        let compressed = encoder
            .finish()
            .map_err(|_| AppError::Internal("a board version couldn't be compressed"))?;
        let key = format!(
            "{}/history/whiteboard-{}-{revision}.json.gz",
            workspace.simple(),
            board.simple()
        );
        store.put(&key, &compressed).await?;
        // Only the version as it was read: elements never change after a
        // version is saved, so this only guards against a second run.
        let updated = sqlx::query(
            r#"
            UPDATE whiteboard_revisions SET elements = '[]'::jsonb, payload_key = $3
            WHERE whiteboard_id = $1 AND revision = $2 AND payload_key IS NULL
            "#,
        )
        .bind(board)
        .bind(revision)
        .bind(&key)
        .execute(crate::db::scoped(&state.pool))
        .await?
        .rows_affected();
        moved += updated;
    }
    Ok(moved)
}

#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSummary {
    pub revision: i32,
    pub scene_seq: i64,
    pub element_count: i32,
    pub author: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

pub async fn revisions(
    pool: &PgPool,
    whiteboard_id: Uuid,
    limit: i64,
) -> Result<Vec<RevisionSummary>, AppError> {
    Ok(sqlx::query_as::<_, RevisionSummary>(
        r#"
        SELECT revision.revision, revision.scene_seq, revision.element_count,
               (SELECT COALESCE(profile.display_name, account.display_name)
                  FROM actors author
                  LEFT JOIN organization_actor_profiles profile
                    ON profile.organization_id = board.organization_id AND profile.actor_id = author.id
                  LEFT JOIN users account ON account.actor_id = author.id
                 WHERE author.id = revision.actor_id) AS author,
               revision.note, revision.created_at::text AS created_at
        FROM whiteboard_revisions revision
        JOIN whiteboards board ON board.id = revision.whiteboard_id
        WHERE revision.whiteboard_id = $1
        ORDER BY revision.revision DESC
        LIMIT $2
        "#,
    )
    .bind(whiteboard_id)
    .bind(limit.clamp(1, 100))
    .fetch_all(crate::db::scoped(pool))
    .await?)
}

/// Puts a board back the way a revision saved it: its elements return as
/// new versions and everything added since is deleted, so open browsers
/// merge the restore like any edit. The current state is saved first.
pub async fn restore_revision(
    store: &files::FileStore,
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    revision: i32,
) -> Result<WriteOutcome, AppError> {
    let saved = revision_elements(store, transaction, whiteboard_id, revision)
        .await?
        .ok_or(AppError::NotFound("whiteboard revision"))?;
    let scene_seq = lock(transaction, whiteboard_id).await?;
    let before = save_revision(
        transaction,
        caller,
        whiteboard_id,
        true,
        Some(&format!("Before restoring revision {revision}")),
    )
    .await?;
    let current = load_rows(transaction, whiteboard_id).await?;
    let versions = current
        .iter()
        .map(|element| {
            (
                scene::id(element).to_owned(),
                element.get("version").and_then(Value::as_i64).unwrap_or(0),
            )
        })
        .collect::<HashMap<_, _>>();
    let now = chrono::Utc::now().timestamp_millis();
    let mut writes = Vec::new();
    let mut kept = HashSet::new();
    for value in saved.as_array().cloned().unwrap_or_default() {
        let Value::Object(mut element) = value else {
            continue;
        };
        let id = scene::id(&element).to_owned();
        let saved_version = element.get("version").and_then(Value::as_i64).unwrap_or(1);
        let version = versions.get(&id).copied().unwrap_or(0).max(saved_version) + 1;
        element.insert("version".to_owned(), version.into());
        element.insert("versionNonce".to_owned(), scene::random_nonce().into());
        element.insert("isDeleted".to_owned(), false.into());
        element.insert("updated".to_owned(), now.into());
        kept.insert(id);
        writes.push(Value::Object(element));
    }
    for mut element in current {
        let id = scene::id(&element).to_owned();
        if kept.contains(&id) || scene::is_deleted(&element) {
            continue;
        }
        let version = element.get("version").and_then(Value::as_i64).unwrap_or(1) + 1;
        element.insert("version".to_owned(), version.into());
        element.insert("versionNonce".to_owned(), scene::random_nonce().into());
        element.insert("isDeleted".to_owned(), true.into());
        element.insert("updated".to_owned(), now.into());
        writes.push(Value::Object(element));
    }
    let parsed = writes
        .into_iter()
        .map(elements::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::Internal("stored revision element"))?;
    let outcome = write_elements(transaction, caller, whiteboard_id, scene_seq, parsed).await?;
    // A restore changes the board like an edit does: rules and webhooks on
    // `whiteboard.updated` hear it (K3.5).
    if let Some(before) = before {
        updated(transaction, caller, whiteboard_id, before).await?;
    }
    Ok(outcome)
}

/// Every element row of a board (deleted ones too), in stacking order.
pub async fn load_rows(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
) -> Result<Vec<Object>, AppError> {
    Ok(sqlx::query_scalar::<_, Value>(
        r#"
        SELECT data FROM whiteboard_elements WHERE whiteboard_id = $1
        ORDER BY sort_index COLLATE "C" NULLS LAST, element_id
        "#,
    )
    .bind(whiteboard_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .filter_map(|value| match value {
        Value::Object(object) => Some(object),
        _ => None,
    })
    .collect())
}

/// The scene, or what changed since `since`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneView {
    pub whiteboard_id: Uuid,
    pub scene_seq: i64,
    /// With `since`, every element changed after it, deleted ones included
    /// (so the client drops them); without it, the live elements.
    pub elements: Vec<Value>,
    /// The ids of the board's images; fetch each from its files route.
    pub files: Vec<FileSummary>,
    /// Who changed the board in the last few minutes, newest first.
    pub recent_editors: Vec<Editor>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSummary {
    pub id: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Editor {
    #[schemars(with = "String")]
    pub actor_id: Uuid,
    pub name: String,
    /// human or external_agent.
    pub kind: String,
    pub at: String,
}

pub async fn scene(
    pool: &PgPool,
    whiteboard_id: Uuid,
    since: Option<i64>,
) -> Result<SceneView, AppError> {
    // One snapshot, so the counter and the elements agree.
    let mut transaction = crate::db::scoped_snapshot(pool).await?;
    let scene_seq = sqlx::query_scalar::<_, i64>("SELECT scene_seq FROM whiteboards WHERE id = $1")
        .bind(whiteboard_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("whiteboard"))?;
    let since = since.filter(|since| *since > 0);
    let elements = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT data FROM whiteboard_elements
        WHERE whiteboard_id = $1
          AND CASE WHEN $2::bigint IS NULL THEN NOT is_deleted ELSE seq > $2 END
        ORDER BY sort_index COLLATE "C" NULLS LAST, element_id
        "#,
    )
    .bind(whiteboard_id)
    .bind(since)
    .fetch_all(&mut *transaction)
    .await?;
    let files = sqlx::query_as::<_, FileSummary>(
        r#"
        SELECT id, mime_type FROM (
            SELECT external_key AS id, content_type AS mime_type, created_at FROM files
            WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND removed_at IS NULL
            UNION ALL
            SELECT file_id, mime_type, created_at FROM whiteboard_files WHERE whiteboard_id = $1
        ) image
        ORDER BY created_at, id
        "#,
    )
    .bind(whiteboard_id)
    .fetch_all(&mut *transaction)
    .await?;
    let recent_editors = recent_editors(&mut transaction, whiteboard_id).await?;
    transaction.commit().await?;
    Ok(SceneView {
        whiteboard_id,
        scene_seq,
        elements,
        files,
        recent_editors,
    })
}

pub async fn recent_editors(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
) -> Result<Vec<Editor>, AppError> {
    Ok(sqlx::query_as::<_, Editor>(
        r#"
        SELECT editor.id AS actor_id,
               COALESCE(profile.display_name, account.display_name, 'Someone') AS name,
               editor.kind, max(element.updated_at)::text AS at
        FROM whiteboard_elements element
        JOIN whiteboards board ON board.id = element.whiteboard_id
        JOIN actors editor ON editor.id = element.updated_by_actor_id
        LEFT JOIN organization_actor_profiles profile
          ON profile.organization_id = board.organization_id AND profile.actor_id = editor.id
        LEFT JOIN users account ON account.actor_id = editor.id
        WHERE element.whiteboard_id = $1 AND element.updated_at > now() - interval '5 minutes'
        GROUP BY editor.id, profile.display_name, account.display_name, editor.kind
        ORDER BY max(element.updated_at) DESC
        LIMIT 10
        "#,
    )
    .bind(whiteboard_id)
    .fetch_all(&mut **transaction)
    .await?)
}

// ---------------------------------------------------------------------------
// Files

/// What a file's bytes must start with for the type it claims.
fn content_matches(mime_type: &str, data: &[u8]) -> bool {
    match mime_type {
        "image/png" => data.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
        "image/webp" => data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP",
        "image/svg+xml" => std::str::from_utf8(data).is_ok_and(|text| {
            let text = text.trim_start_matches('\u{feff}').trim_start();
            text.starts_with('<') && text.to_ascii_lowercase().contains("<svg")
        }),
        _ => false,
    }
}

/// Reads an Excalidraw data URL ("data:image/png;base64,…").
pub fn decode_data_url(data_url: &str, mime_type: &str) -> Result<Vec<u8>, AppError> {
    let prefix = format!("data:{mime_type};base64,");
    let encoded = data_url
        .strip_prefix(&prefix)
        .ok_or(AppError::Validation("dataURL"))?;
    let data = decode_base64(encoded).ok_or(AppError::Validation("dataURL"))??;
    if !content_matches(mime_type, &data) {
        return Err(AppError::Validation("mimeType"));
    }
    Ok(data)
}

/// Up to `MAX_FILE_BYTES` of base64. None when it isn't base64.
fn decode_base64(encoded: &str) -> Option<Result<Vec<u8>, AppError>> {
    if encoded.len() > MAX_FILE_BYTES / 3 * 4 + 8 {
        return Some(Err(AppError::PayloadTooLarge));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if data.is_empty() || data.len() > MAX_FILE_BYTES {
        return Some(Err(AppError::PayloadTooLarge));
    }
    Some(Ok(data))
}

/// An image an agent sent as base64, with its type (given, or recognized
/// from its bytes) and the id Excalidraw would give it: named by content,
/// so the same image stored twice is stored once.
pub fn decode_image(
    encoded: &str,
    mime_type: Option<&str>,
) -> Result<(String, String, Vec<u8>), AppError> {
    let data = decode_base64(encoded.trim()).ok_or(AppError::Validation("dataBase64"))??;
    let mime_type = match mime_type {
        Some(mime_type) => mime_type.to_owned(),
        None => [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/svg+xml",
        ]
        .into_iter()
        .find(|mime_type| content_matches(mime_type, &data))
        .ok_or(AppError::Validation("mimeType"))?
        .to_owned(),
    };
    if !allowed_mime(&mime_type) || !content_matches(&mime_type, &data) {
        return Err(AppError::Validation("mimeType"));
    }
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&data);
    let id = digest
        .iter()
        .take(20)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((id, mime_type, data))
}

/// Which of these files the board has: in `files` (C1.29), or still in the
/// old table until the move job reaches them.
pub async fn stored_files(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
    file_ids: &[String],
) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT external_key FROM files
        WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND external_key = ANY($2)
          AND removed_at IS NULL
        UNION
        SELECT file_id FROM whiteboard_files WHERE whiteboard_id = $1 AND file_id = ANY($2)
        "#,
    )
    .bind(whiteboard_id)
    .bind(file_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

pub fn allowed_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml"
    )
}

/// Stores an image for a board the caller may edit, as a file of the board
/// in the file store (C1.29). Excalidraw names files by content, so a
/// second upload of the same id changes nothing.
pub(crate) async fn put_file(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    file_id: &str,
    mime_type: &str,
    data: &[u8],
    staged: &files::StagedContent,
) -> Result<bool, AppError> {
    if !elements::valid_id(file_id) {
        return Err(AppError::Validation("id"));
    }
    if !allowed_mime(mime_type) {
        return Err(AppError::Validation("mimeType"));
    }
    let (used, project_id) = sqlx::query_as::<_, (i64, Uuid)>(
        "SELECT file_bytes, project_id FROM whiteboards WHERE id = $1 FOR UPDATE",
    )
    .bind(whiteboard_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !stored_files(transaction, whiteboard_id, &[file_id.to_owned()])
        .await?
        .is_empty()
    {
        return Ok(false);
    }
    let size = i64::try_from(data.len()).map_err(|_| AppError::PayloadTooLarge)?;
    if used + size > MAX_BOARD_FILE_BYTES {
        return Err(AppError::Conflict(
            "whiteboard_files_full",
            "This board's images use all the space a board has. Remove some images first.",
        ));
    }
    let name = format!("{file_id}.{}", extension(mime_type));
    files::store_content(
        transaction,
        caller.actor_id,
        files::ContentFile {
            project_id,
            thing: files::ThingKind::Whiteboard,
            thing_id: whiteboard_id,
            purpose: files::Purpose::Inline,
            external_key: file_id,
            name: &name,
            content_type: mime_type,
            bytes: data,
        },
        staged,
    )
    .await?;
    sqlx::query("UPDATE whiteboards SET file_bytes = file_bytes + $2 WHERE id = $1")
        .bind(whiteboard_id)
        .bind(size)
        .execute(&mut **transaction)
        .await?;
    Ok(true)
}

fn extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub id: String,
    pub mime_type: String,
    #[serde(rename = "dataURL")]
    pub data_url: String,
    pub created: i64,
}

/// One stored image of a board: its id, type, when it was added, and where
/// its bytes are.
struct StoredImage {
    id: String,
    mime_type: String,
    created: chrono::DateTime<chrono::Utc>,
    bytes: ImageBytes,
}

enum ImageBytes {
    /// In a file store: which one, and its key there.
    Stored { store_id: String, key: String },
    /// Still in the old table, until the move job reaches it.
    Inline(Vec<u8>),
}

/// The board's images with these ids, from `files` first, then from the
/// old table for any the move job hasn't reached.
async fn images(
    pool: &PgPool,
    whiteboard_id: Uuid,
    file_ids: &[String],
) -> Result<Vec<StoredImage>, AppError> {
    let mut found = sqlx::query_as::<
        _,
        (
            String,
            String,
            chrono::DateTime<chrono::Utc>,
            String,
            String,
        ),
    >(
        r#"
        SELECT external_key, content_type, created_at, store_id, storage_key FROM files
        WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND external_key = ANY($2)
          AND removed_at IS NULL
        "#,
    )
    .bind(whiteboard_id)
    .bind(file_ids)
    .fetch_all(crate::db::scoped(pool))
    .await?
    .into_iter()
    .map(|(id, mime_type, created, store_id, key)| StoredImage {
        id,
        mime_type,
        created,
        bytes: ImageBytes::Stored { store_id, key },
    })
    .collect::<Vec<_>>();
    let missing = file_ids
        .iter()
        .filter(|id| !found.iter().any(|image| &image.id == *id))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        found.extend(
            sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>, Vec<u8>)>(
                r#"
                SELECT file_id, mime_type, created_at, data FROM whiteboard_files
                WHERE whiteboard_id = $1 AND file_id = ANY($2)
                "#,
            )
            .bind(whiteboard_id)
            .bind(&missing)
            .fetch_all(crate::db::scoped(pool))
            .await?
            .into_iter()
            .map(|(id, mime_type, created, data)| StoredImage {
                id,
                mime_type,
                created,
                bytes: ImageBytes::Inline(data),
            }),
        );
    }
    Ok(found)
}

async fn image_bytes(
    state: &AppState,
    image: StoredImage,
) -> Result<(StoredImage, Vec<u8>), AppError> {
    let bytes = match &image.bytes {
        ImageBytes::Inline(data) => data.clone(),
        // The row's own store: a move between stores changes it apart
        // from the configured one.
        ImageBytes::Stored { store_id, key } => {
            files::FileStore::resolve(state, store_id)
                .await?
                .read_all(key, MAX_FILE_BYTES as u64)
                .await?
        }
    };
    Ok((image, bytes))
}

pub async fn get_file(
    state: &AppState,
    whiteboard_id: Uuid,
    file_id: &str,
) -> Result<FileView, AppError> {
    let image = images(&state.pool, whiteboard_id, &[file_id.to_owned()])
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound("whiteboard file"))?;
    let (image, data) = image_bytes(state, image).await?;
    Ok(FileView {
        id: image.id,
        data_url: format!(
            "data:{};base64,{}",
            image.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&data)
        ),
        mime_type: image.mime_type,
        created: image.created.timestamp_millis(),
    })
}

/// The images the given elements show, for rendering.
pub async fn files_for(
    state: &AppState,
    whiteboard_id: Uuid,
    elements: &[Object],
) -> Result<HashMap<String, super::render::File>, AppError> {
    let mut wanted = elements
        .iter()
        .filter(|element| !scene::is_deleted(element))
        .filter_map(|element| scene::text_field(element, "fileId"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    wanted.sort();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let mut found = HashMap::new();
    for image in images(&state.pool, whiteboard_id, &wanted).await? {
        // An image whose bytes can't be read is left out of the drawing
        // rather than failing it.
        if let Ok((image, data)) = image_bytes(state, image).await {
            found.insert(
                image.id,
                super::render::File {
                    mime_type: image.mime_type,
                    data,
                },
            );
        }
    }
    Ok(found)
}

/// Copies a board's images to its duplicate: new rows of the copy over
/// the same stored bytes in the same store, so nothing is copied (C1.29). Images still in the
/// old table are copied there, and move with the rest.
pub async fn copy_files(
    transaction: &mut Transaction<'_, Postgres>,
    from: Uuid,
    to: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO files (
            id, organization_id, project_id, uploaded_by_actor_id, name, content_type,
            byte_size, sha256, store_id, storage_key, image_width, image_height, attached_at,
            thing_kind, thing_id, purpose, external_key
        )
        SELECT gen_random_uuid(), copy.organization_id, copy.project_id, $3, file.name,
               file.content_type, file.byte_size, file.sha256, file.store_id, file.storage_key,
               file.image_width, file.image_height, now(), 'whiteboard', copy.id,
               file.purpose, file.external_key
        FROM files file
        JOIN whiteboards copy ON copy.id = $2
        WHERE file.thing_kind = 'whiteboard' AND file.thing_id = $1
          AND file.removed_at IS NULL AND file.thumbnail_key IS NULL
          AND file.organization_id = copy.organization_id
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(actor)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO whiteboard_files (whiteboard_id, file_id, mime_type, data, created_by_actor_id)
        SELECT $2, file_id, mime_type, data, $3 FROM whiteboard_files WHERE whiteboard_id = $1
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(actor)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Marks a purged board's images removed, so the file cleanup takes their
/// bytes (unless another board still shows them).
pub async fn remove_files(
    transaction: &mut Transaction<'_, Postgres>,
    whiteboard_id: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE files SET removed_at = now(), removed_by_actor_id = $2
        WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND removed_at IS NULL
        "#,
    )
    .bind(whiteboard_id)
    .bind(actor)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Moving images out of the old table (C1.29)

pub const MOVE_IMAGES_KIND: &str = "whiteboards.move_images";
/// Images moved per transaction.
const MOVE_BATCH: i64 = 20;
/// A run stops after this long and queues the next.
const MOVE_RUN: std::time::Duration = std::time::Duration::from_secs(240);

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register_with_timeout(
        MOVE_IMAGES_KIND,
        std::time::Duration::from_secs(300),
        |context: crate::jobs::JobContext| async move {
            let more = move_images(&context.state, MOVE_RUN)
                .await
                .map_err(|error| crate::jobs::JobError::retry(error.log_category()))?;
            if more {
                crate::jobs::enqueue(
                    crate::db::scoped(&context.state.pool),
                    crate::jobs::NewJob::new(MOVE_IMAGES_KIND)
                        .unique(format!("{MOVE_IMAGES_KIND}:{}", Uuid::now_v7()))
                        .max_attempts(50),
                )
                .await
                .map_err(|error| crate::jobs::JobError::retry(error.log_category()))?;
            }
            Ok(())
        },
    );
}

/// Moves board images from `whiteboard_files` into the file store, a batch
/// per transaction, for up to `budget`. True when some are left.
pub async fn move_images(state: &AppState, budget: std::time::Duration) -> Result<bool, AppError> {
    let started = std::time::Instant::now();
    loop {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                String,
                Vec<u8>,
                Uuid,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            r#"
            SELECT old.whiteboard_id, board.project_id, old.file_id, old.mime_type, old.data,
                   old.created_by_actor_id, old.created_at
            FROM whiteboard_files old
            JOIN whiteboards board ON board.id = old.whiteboard_id
            ORDER BY old.whiteboard_id, old.file_id
            LIMIT $1
            FOR UPDATE OF old SKIP LOCKED
            "#,
        )
        .bind(MOVE_BATCH)
        .fetch_all(&mut *transaction)
        .await?;
        if rows.is_empty() {
            transaction.commit().await?;
            return Ok(false);
        }
        for (board, project_id, file_id, mime_type, data, actor, created) in rows {
            let name = format!("{file_id}.{}", extension(&mime_type));
            files::move_content(
                state,
                &mut transaction,
                actor,
                files::ContentFile {
                    project_id,
                    thing: files::ThingKind::Whiteboard,
                    thing_id: board,
                    purpose: files::Purpose::Inline,
                    external_key: &file_id,
                    name: &name,
                    content_type: &mime_type,
                    bytes: &data,
                },
                created,
            )
            .await?;
            // The old row goes once its file is in `files`, in the same
            // transaction: the move, not a removal (rule 16).
            sqlx::query("DELETE FROM whiteboard_files WHERE whiteboard_id = $1 AND file_id = $2")
                .bind(board)
                .bind(&file_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        if started.elapsed() >= budget {
            return Ok(true);
        }
    }
}

/// The live elements, for reading and rendering outside a write.
pub async fn live_elements(pool: &PgPool, whiteboard_id: Uuid) -> Result<Vec<Object>, AppError> {
    Ok(sqlx::query_scalar::<_, Value>(
        r#"
        SELECT data FROM whiteboard_elements WHERE whiteboard_id = $1 AND NOT is_deleted
        ORDER BY sort_index COLLATE "C" NULLS LAST, element_id
        "#,
    )
    .bind(whiteboard_id)
    .fetch_all(crate::db::scoped(pool))
    .await?
    .into_iter()
    .filter_map(|value| match value {
        Value::Object(object) => Some(object),
        _ => None,
    })
    .collect())
}

#[cfg(test)]
mod tests {
    use super::content_matches;

    #[test]
    fn files_must_be_what_they_claim() {
        assert!(content_matches("image/png", b"\x89PNG\r\n\x1a\nrest"));
        assert!(!content_matches("image/png", b"<svg onload=alert(1)>"));
        assert!(content_matches(
            "image/svg+xml",
            b"<?xml version=\"1.0\"?><svg></svg>"
        ));
        assert!(!content_matches("image/svg+xml", b"<html><script>"));
        assert!(!content_matches("text/html", b"<html>"));
    }
}
