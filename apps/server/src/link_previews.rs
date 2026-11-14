//! W20: link previews. When a new message links to web pages, a background
//! job fetches each page's title, description and picture (Open Graph tags,
//! else `<title>`) once per URL per workspace, and the message shows them.
//!
//! The fetch is the server reaching out on someone's say-so, so it is
//! guarded (`outbound::PagePolicy`): http and https on the standard ports
//! only, every address the name resolves to must be public (no loopback,
//! private, link-local or other special ranges), the checked addresses are
//! pinned so a
//! second DNS answer can't change it, each redirect is checked again, and
//! there are limits on time, redirects and bytes. The page's picture is
//! fetched the same way and stored, so viewers' browsers only ever load
//! images from this server.
//!
//! The author (or a moderator) can hide a preview from their message; the
//! workspace setting `messages.link_previews` turns new previews off.

mod page;

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::MessageCommandActor,
    error::{ApiPath, AppError},
    files::{FileStore, image},
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    message_edits::{Change, lock_message_for_change},
    outbox::{MessageUpdatedPayload, OutboxEventPayload, enqueue_typed},
    settings::{self, SettingsContext},
};

/// Previews per message, in the order the links appear.
pub const MAX_PREVIEWS_PER_MESSAGE: usize = 3;
const FETCH_KIND: &str = "link_previews.fetch";
/// The picture shown with a preview is at most this many pixels across.
const IMAGE_SIDE: u32 = 600;
/// A failed fetch is tried again for a new message after this long.
const RETRY_FAILED_AFTER_HOURS: i32 = 24;

/// A preview as a message shows it. The picture, when there is one, is at
/// `/api/v1/link-previews/{id}/image`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreviewView {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_height: Option<i32>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/link-previews/{id}/image", get(image_http))
        .route(
            "/api/v1/conversations/{id}/messages/{message_id}/link-previews/{preview_id}",
            delete(hide_http),
        )
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(FETCH_KIND, Duration::from_secs(60), fetch_job);
}

// ---------------------------------------------------------------------------
// Finding links

/// The web links in a message's text, normalized (no fragment), each once,
/// in order, at most [`MAX_PREVIEWS_PER_MESSAGE`]. Links with credentials or
/// to anything but http and https are skipped.
pub fn links_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = find_scheme(rest) {
        let candidate = &rest[start..];
        let end = candidate
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '<' | '>' | '"' | '`' | '\'')
            })
            .unwrap_or(candidate.len());
        let mut link = &candidate[..end];
        // Trailing punctuation belongs to the sentence, and an unbalanced
        // closing bracket to the Markdown or prose around the link.
        loop {
            let trimmed = link.trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_', '~']);
            let unbalanced = trimmed.matches(')').count() > trimmed.matches('(').count();
            let trimmed = match trimmed.strip_suffix(')') {
                Some(shorter) if unbalanced => shorter,
                _ => trimmed.strip_suffix(']').unwrap_or(trimmed),
            };
            if trimmed.len() == link.len() {
                break;
            }
            link = trimmed;
        }
        if let Some(url) = normalize(link)
            && !found.contains(&url)
        {
            found.push(url);
            if found.len() == MAX_PREVIEWS_PER_MESSAGE {
                break;
            }
        }
        rest = &candidate[end.max(1)..];
    }
    found
}

fn find_scheme(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    match (lower.find("https://"), lower.find("http://")) {
        (Some(secure), Some(plain)) => Some(secure.min(plain)),
        (found, None) | (None, found) => found,
    }
}

fn normalize(link: &str) -> Option<String> {
    let mut url = url::Url::parse(link).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    let url = url.to_string();
    (url.len() <= 2048).then_some(url)
}

// ---------------------------------------------------------------------------
// Attaching previews to new messages

/// Shows previews for the links in a new message, fetching the ones this
/// workspace hasn't seen (or that failed a while ago) in the background.
/// Does nothing when the workspace or project turned previews off.
pub(crate) async fn add_for_message(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    message_id: Uuid,
    plain_text: &str,
) -> Result<(), AppError> {
    let links = links_in(plain_text);
    if links.is_empty() {
        return Ok(());
    }
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    let effective = settings::resolve(
        transaction,
        SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    if !effective.bool("messages.link_previews").unwrap_or(true) {
        return Ok(());
    }
    for (position, url) in links.iter().enumerate() {
        let (preview_id, needs_fetch) = sqlx::query_as::<_, (Uuid, bool)>(
            r#"
            INSERT INTO link_previews (id, organization_id, url)
            VALUES ($1, $2, $3)
            ON CONFLICT (organization_id, url) DO UPDATE
            SET status = CASE
                    WHEN link_previews.status = 'failed'
                     AND link_previews.fetched_at < now() - make_interval(hours => $4)
                    THEN 'pending' ELSE link_previews.status END
            RETURNING id, status = 'pending'
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(organization_id)
        .bind(url)
        .bind(RETRY_FAILED_AFTER_HOURS)
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO message_link_previews (project_id, message_id, link_preview_id, position)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(message_id)
        .bind(preview_id)
        .bind(i16::try_from(position).unwrap_or(0))
        .execute(&mut **transaction)
        .await?;
        if needs_fetch {
            crate::jobs::enqueue(
                &mut **transaction,
                NewJob::new(FETCH_KIND)
                    .payload(FetchPayload { preview_id })
                    .workspace(organization_id)
                    .unique(format!("link_preview:{preview_id}"))
                    .max_attempts(2),
            )
            .await?;
        }
    }
    Ok(())
}

/// After an edit: previews follow the message's links as they are now. A
/// link edited out takes its preview with it (title, description and
/// picture), a link added gets one, and one kept stays as it was, hidden or
/// not.
pub(crate) async fn refresh_for_message(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    message_id: Uuid,
    plain_text: &str,
) -> Result<(), AppError> {
    let links = links_in(plain_text);
    sqlx::query(
        r#"
        DELETE FROM message_link_previews shown
        USING link_previews preview
        WHERE shown.link_preview_id = preview.id AND shown.message_id = $1
          AND NOT (preview.url = ANY($2))
        "#,
    )
    .bind(message_id)
    .bind(&links)
    .execute(&mut **transaction)
    .await?;
    add_for_message(transaction, project_id, message_id, plain_text).await?;
    sqlx::query(
        r#"
        UPDATE message_link_previews shown
        SET position = array_position($2, preview.url) - 1
        FROM link_previews preview
        WHERE shown.link_preview_id = preview.id AND shown.message_id = $1
        "#,
    )
    .bind(message_id)
    .bind(&links)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchPayload {
    preview_id: Uuid,
}

async fn fetch_job(context: JobContext) -> Result<(), JobError> {
    let payload = context.job.payload_as::<FetchPayload>()?;
    let state = &context.state;
    let Some((organization_id, url, status)) = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT organization_id, url, status FROM link_previews WHERE id = $1",
    )
    .bind(payload.preview_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(());
    };
    if status != "pending" {
        return Ok(());
    }
    let policy = crate::outbound::PagePolicy::public();
    let permit = crate::outbound::permit(organization_id).await;
    let outcome = match page::fetch(&policy, &url).await {
        Ok(found) => {
            let picture = match &found.image_url {
                Some(image_url) => preview_image(&policy, image_url).await,
                None => None,
            };
            Ok((found, picture))
        }
        Err(reason) => Err(reason),
    };
    drop(permit);
    let store = FileStore::from_config(&state.config);
    let result = match outcome {
        Ok((found, picture)) => {
            let room = match &picture {
                Some((bytes, _, _, _)) => crate::files::require_room(
                    state,
                    organization_id,
                    u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                )
                .await
                .is_ok(),
                None => false,
            };
            // A full workspace or disk keeps the preview's words, not its picture.
            let image_key = match picture.as_ref().filter(|_| room) {
                Some((bytes, _, _, _)) => {
                    let key = format!(
                        "{}/{}.preview",
                        organization_id.simple(),
                        payload.preview_id.simple()
                    );
                    match store.put(&key, bytes).await {
                        Ok(()) => Some(key),
                        Err(_) => None,
                    }
                }
                None => None,
            };
            let picture = picture.filter(|_| image_key.is_some());
            sqlx::query(
                r#"
                UPDATE link_previews
                SET status = 'ready', title = $2, description = $3, site_name = $4,
                    image_key = $5, image_bytes = $6, image_width = $7, image_height = $8,
                    image_type = $9, failure = NULL, fetched_at = now()
                WHERE id = $1 AND status = 'pending'
                "#,
            )
            .bind(payload.preview_id)
            .bind(&found.title)
            .bind(&found.description)
            .bind(&found.site_name)
            .bind(&image_key)
            .bind(
                picture
                    .as_ref()
                    .map(|(bytes, ..)| i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            )
            .bind(
                picture
                    .as_ref()
                    .map(|(_, _, width, _)| i32::try_from(*width).unwrap_or(i32::MAX)),
            )
            .bind(
                picture
                    .as_ref()
                    .map(|(_, _, _, height)| i32::try_from(*height).unwrap_or(i32::MAX)),
            )
            .bind(picture.as_ref().map(|(_, content_type, ..)| *content_type))
            .execute(&state.pool)
            .await
        }
        Err(reason) => {
            sqlx::query(
                r#"
                UPDATE link_previews
                SET status = 'failed', failure = $2, fetched_at = now()
                WHERE id = $1 AND status = 'pending'
                "#,
            )
            .bind(payload.preview_id)
            .bind(reason)
            .execute(&state.pool)
            .await
        }
    };
    result?;
    refresh_messages(state, payload.preview_id).await?;
    Ok(())
}

/// The page's picture, shrunk for the preview: (bytes, content type,
/// width, height).
async fn preview_image(
    policy: &crate::outbound::PagePolicy,
    url: &str,
) -> Option<(Vec<u8>, &'static str, u32, u32)> {
    let bytes = page::fetch_image(policy, url).await.ok()?;
    image::one_at_a_time(move || image::shrink(bytes, IMAGE_SIDE))
        .await
        .flatten()
}

/// Open timelines refetch the messages that show this preview.
async fn refresh_messages(state: &AppState, preview_id: Uuid) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    let messages = sqlx::query_as::<_, (Uuid, Uuid, Uuid, i32)>(
        r#"
        SELECT message.project_id, message.conversation_id, message.id, message.revision
        FROM message_link_previews shown
        JOIN messages message ON message.id = shown.message_id
        WHERE shown.link_preview_id = $1
          AND shown.hidden_at IS NULL
          AND message.removed_at IS NULL
          AND message.created_at > now() - interval '1 day'
        ORDER BY message.created_at DESC
        LIMIT 50
        "#,
    )
    .bind(preview_id)
    .fetch_all(&mut *transaction)
    .await?;
    for (project_id, conversation_id, message_id, revision) in messages {
        enqueue_typed(
            &mut transaction,
            project_id,
            OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
                conversation_id,
                message_id,
                revision,
                removed: false,
            }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Deletes stored pictures of previews no message shows any more.
pub(crate) async fn cleanup(state: &AppState, store: &FileStore) -> Result<u64, AppError> {
    let orphans = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT preview.id, preview.image_key
        FROM link_previews preview
        WHERE preview.image_key IS NOT NULL
          AND preview.fetched_at < now() - interval '1 day'
          AND NOT EXISTS (
              SELECT 1 FROM message_link_previews shown
              WHERE shown.link_preview_id = preview.id
          )
        LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    let mut deleted = 0;
    for (id, key) in orphans {
        if let Err(error) = store.delete(&key).await {
            tracing::warn!(preview_id = %id, category = error.log_category(), "preview picture not deleted");
            continue;
        }
        // Marked failed long enough ago to retry: a later message linking
        // the same page fetches it again, picture and all, rather than
        // showing it without one for good.
        sqlx::query(
            r#"
            UPDATE link_previews
            SET image_key = NULL, image_bytes = NULL, image_width = NULL, image_height = NULL,
                image_type = NULL, status = 'failed', failure = 'expired',
                fetched_at = now() - make_interval(hours => $2 + 1)
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(RETRY_FAILED_AFTER_HOURS)
        .execute(&state.pool)
        .await?;
        deleted += 1;
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Routes

/// A preview's picture, for someone who can see a message showing it.
async fn image_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(preview_id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let (key, content_type) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT preview.image_key, preview.image_type
        FROM link_previews preview
        WHERE preview.id = $1
          AND preview.image_key IS NOT NULL
          AND preview.image_type IS NOT NULL
          AND EXISTS (
              SELECT 1
              FROM message_link_previews shown
              JOIN messages message ON message.id = shown.message_id
              JOIN projects project ON project.id = message.project_id
              JOIN project_memberships membership
                ON membership.project_id = message.project_id
               AND membership.actor_id = $2
               AND membership.enabled
              JOIN organization_memberships organization_membership
                ON organization_membership.organization_id = project.organization_id
               AND organization_membership.actor_id = $2
               AND organization_membership.enabled
              WHERE shown.link_preview_id = preview.id
                AND shown.hidden_at IS NULL
                AND message.removed_at IS NULL
                AND conversation_visible_to(message.conversation_id, $2)
          )
        "#,
    )
    .bind(preview_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("link_preview"))?;
    crate::files::http::serve(&state, &key, &content_type, "preview", false).await
}

/// Hides one preview from a message: its author, or a moderator.
async fn hide_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id, preview_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let actor = MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
    };
    let mut transaction = state.pool.begin().await?;
    let locked = lock_message_for_change(
        &mut transaction,
        &actor,
        Some(conversation_id),
        message_id,
        Change::Remove,
    )
    .await?;
    hide_in(
        &mut transaction,
        &locked,
        principal.actor_id,
        Hide::Preview(preview_id),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Which of a message's link previews to hide.
pub(crate) enum Hide<'a> {
    /// One, by its ID (the web's remove button).
    Preview(Uuid),
    /// The one for this link, or all of them.
    Url(Option<&'a str>),
}

/// Hides link previews on a message the caller may change (locked with
/// `Change::Remove`), for everyone, and tells open pages. Returns how many
/// were newly hidden; not found when none match.
pub(crate) async fn hide_in(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    locked: &crate::message_edits::LockedMessage,
    actor_id: Uuid,
    which: Hide<'_>,
) -> Result<u64, AppError> {
    if locked.removed {
        return Err(AppError::NotFound("link_preview"));
    }
    let (preview_id, url, all) = match which {
        Hide::Preview(id) => (Some(id), None, false),
        // Stored links are normalized (a root slash added, no fragment):
        // match what someone typed the same way.
        Hide::Url(Some(url)) => (
            None,
            Some(normalize(url.trim()).unwrap_or_else(|| url.trim().to_owned())),
            false,
        ),
        Hide::Url(None) => (None, None, true),
    };
    let matching = r#"
        message.message_id = $1
        AND ($2::uuid IS NULL OR message.link_preview_id = $2)
        AND ($3::text IS NULL OR EXISTS (
            SELECT 1 FROM link_previews preview
            WHERE preview.id = message.link_preview_id AND preview.url = $3))
        AND ($2::uuid IS NOT NULL OR $3::text IS NOT NULL OR $5)
    "#;
    let hidden = sqlx::query(&format!(
        r#"
        UPDATE message_link_previews message
        SET hidden_at = now(), hidden_by_actor_id = $4
        WHERE {matching} AND message.hidden_at IS NULL
        "#
    ))
    .bind(locked.id)
    .bind(preview_id)
    .bind(&url)
    .bind(actor_id)
    .bind(all)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if hidden == 0 {
        let exists = sqlx::query_scalar::<_, bool>(&format!(
            "SELECT EXISTS (SELECT 1 FROM message_link_previews message WHERE {matching} AND $4::uuid IS NOT NULL)"
        ))
        .bind(locked.id)
        .bind(preview_id)
        .bind(&url)
        .bind(actor_id)
        .bind(all)
        .fetch_one(&mut **transaction)
        .await?;
        if !exists {
            return Err(AppError::NotFound("link_preview"));
        }
    } else {
        enqueue_typed(
            transaction,
            locked.project_id,
            OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
                conversation_id: locked.conversation.id,
                message_id: locked.id,
                revision: locked.revision,
                removed: false,
            }),
        )
        .await?;
    }
    Ok(hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_found_in_prose_and_markdown() {
        assert_eq!(
            links_in("see https://example.com/a?b=1#top, and (http://example.org/x)."),
            vec!["https://example.com/a?b=1", "http://example.org/x"]
        );
        assert_eq!(
            links_in("[docs](https://docs.rs/axum) and https://docs.rs/axum again"),
            vec!["https://docs.rs/axum"]
        );
        assert_eq!(
            links_in("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
        assert!(links_in("ftp://example.com and https://user:pw@example.com").is_empty());
        assert_eq!(
            links_in("https://a.example https://b.example https://c.example https://d.example")
                .len(),
            MAX_PREVIEWS_PER_MESSAGE
        );
    }
}
