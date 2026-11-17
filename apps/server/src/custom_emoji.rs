//! A workspace's own emoji (C2.19): a small PNG, GIF or WebP under a
//! `:name:`, for reactions and the picker. Anyone in the workspace adds one;
//! whoever added it, or someone who manages the workspace's settings,
//! retires it. A retired emoji leaves the picker and stays on the reactions
//! that use it, so old messages keep reading the same.
//!
//! `custom_emoji_create`, `custom_emoji_list` and `custom_emoji_update` (in
//! `ops.rs`) are the ways in; `GET /api/v1/custom-emoji/{id}` serves the
//! image to members of its workspace only.

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
};

pub mod ops;

/// Tells every space of `workspace_id` that its emoji changed
/// (`custom_emoji.changed`), in the change's transaction, so open reaction
/// pickers read the list again.
pub(crate) async fn announce(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    emoji_id: Uuid,
) -> Result<(), AppError> {
    let spaces = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 AND archived_at IS NULL \
         ORDER BY created_at, id LIMIT 500",
    )
    .bind(workspace_id)
    .fetch_all(&mut **transaction)
    .await?;
    for space in spaces {
        crate::outbox::enqueue_typed(
            transaction,
            space,
            crate::outbox::OutboxEventPayload::CustomEmojiChanged(
                crate::outbox::CustomEmojiChangedPayload {
                    workspace_id,
                    project_id: space,
                    emoji_id,
                },
            ),
        )
        .await?;
    }
    Ok(())
}

/// Charges the events a change in `workspace_id` will send (one to each
/// live space, as [`announce`] sends them) before it's made, so adding and
/// retiring emoji can't flood a large workspace's members.
pub(crate) async fn charge_announcement(
    state: &crate::AppState,
    workspace_id: Uuid,
) -> Result<(), AppError> {
    let spaces = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE organization_id = $1 AND archived_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;
    let events = u32::try_from(spaces.clamp(1, 500)).unwrap_or(500);
    state
        .rate_limits
        .check_custom_emoji_events(workspace_id, events)
        .await
}

/// The most live custom emoji one workspace keeps (so the list shows all).
pub const MAX_EMOJI: i64 = MAX_LISTED;
/// The most bytes one workspace's custom emoji images take, retired ones
/// included.
pub const MAX_TOTAL_BYTES: i64 = 16 * 1024 * 1024;
/// The largest image kept.
pub const MAX_IMAGE_BYTES: usize = 65_536;
/// The most custom emoji one workspace lists.
pub const MAX_LISTED: i64 = 500;

/// A custom emoji as the picker and tools show it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomEmojiView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Written as :name: in a reaction.
    pub name: String,
    /// Where the image is, for members of this workspace.
    pub image_url: String,
    #[schemars(with = "String")]
    pub created_by_account_id: Uuid,
    #[schemars(with = "String")]
    pub created_at: DateTime<Utc>,
    /// Retired: out of the picker, still on old reactions.
    pub retired: bool,
}

pub(crate) const VIEW_COLUMNS: &str = "id, name, '/api/v1/custom-emoji/' || id AS image_url, \
     created_by_account_id, created_at, retired_at IS NOT NULL AS retired";

/// The image type its first bytes say, of the three kept.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/custom-emoji/{id}", get(image))
}

/// The image, for someone in the emoji's workspace; anyone else gets not
/// found.
async fn image(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let (content_type, bytes) = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (String, Vec<u8>)>(
            r#"
        SELECT emoji.content_type, emoji.image
        FROM custom_emojis emoji
        JOIN organization_memberships membership
          ON membership.organization_id = emoji.workspace_id
         AND membership.actor_id = $2
         AND membership.enabled
        WHERE emoji.id = $1
        "#,
        )
        .bind(id)
        .bind(principal.actor_id)
        .fetch_optional(&mut *connection)
        .await
    })
    .await?
    .ok_or(AppError::NotFound("custom_emoji"))?;
    let content_type = match image_type(&bytes) {
        Some(found) if found == content_type => found,
        _ => return Err(AppError::NotFound("custom_emoji")),
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::image_type;

    #[test]
    fn only_png_gif_and_webp_are_images() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_type(b"GIF89a..."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"<svg onload=alert(1)>"), None);
        assert_eq!(image_type(b""), None);
    }
}
