//! Profiles (C2.17): a title, pronouns, a time zone and an avatar for every
//! account, people and agents alike. One row an account in
//! `account_profiles`, global to it (R19): it shows in every workspace the
//! account is in, and only to people who share one of them. "The spaces you
//! share" lists only spaces of the workspace the reader is in.
//!
//! `profile_get`, `profile_update` and `profile_avatar_list` (in `ops.rs`)
//! are the ways in; `GET /api/v1/accounts/{id}/avatar` serves the image.

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
};

pub mod ops;

pub const MAX_TITLE: usize = 80;
pub const MAX_PRONOUNS: usize = 40;
/// The largest avatar image kept, in bytes.
pub const MAX_AVATAR_BYTES: usize = 256 * 1024;

/// The image type its first bytes say, of the four kept.
pub fn avatar_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else {
        crate::custom_emoji::image_type(bytes)
    }
}

/// Where an account's avatar is served, with its version so a new image
/// gets a new address.
pub fn avatar_url(account_id: Uuid, version: i32) -> String {
    format!("/api/v1/accounts/{account_id}/avatar?v={version}")
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/accounts/{id}/avatar", get(avatar))
}

/// The avatar, for someone who shares a workspace with the account (or
/// the account itself); anyone else gets not found.
async fn avatar(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let (content_type, bytes) = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (String, Vec<u8>)>(
            r#"
            SELECT profile.avatar_type, profile.avatar
            FROM account_profiles profile
            WHERE profile.account_id = $1
              AND profile.avatar IS NOT NULL
              AND ($1 = $2 OR EXISTS (
                  SELECT 1 FROM organization_memberships theirs
                  JOIN organization_memberships mine
                    ON mine.organization_id = theirs.organization_id
                   AND mine.actor_id = $2 AND mine.enabled
                  WHERE theirs.actor_id = $1 AND theirs.enabled
              ))
            "#,
        )
        .bind(id)
        .bind(principal.actor_id)
        .fetch_optional(&mut *connection)
        .await
    })
    .await?
    .ok_or(AppError::NotFound("avatar"))?;
    let content_type = match avatar_type(&bytes) {
        Some(found) if found == content_type => found,
        _ => return Err(AppError::NotFound("avatar")),
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    // The address carries the version, so a cached copy is never stale.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=604800, immutable"),
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
    use super::avatar_type;

    #[test]
    fn avatars_are_png_jpeg_gif_or_webp() {
        assert_eq!(avatar_type(b"\xFF\xD8\xFF\xE0rest"), Some("image/jpeg"));
        assert_eq!(avatar_type(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(avatar_type(b"<svg onload=alert(1)>"), None);
    }
}
