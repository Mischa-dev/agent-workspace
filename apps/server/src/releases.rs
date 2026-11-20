//! Numbered releases and the update check (H1.15). Releases are numbered
//! `0.N.0` until the public release, with notes in `CHANGELOG.md`. Once a
//! day a `work` process reads the release feed the operator configured
//! (`AGENT_WORKSPACE_UPDATE_CHECK_URL`), sending nothing but the running
//! version in its user agent, and keeps the newest version it names; the
//! Server page and `preflight` say when it is newer than this build.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::{error::AppError, jobs::JobRegistry};

pub const CHECK_KIND: &str = "server.release_check";
/// This build's version.
pub const RUNNING: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = concat!("agent-workspace/", env!("CARGO_PKG_VERSION"));
/// The most of a feed read.
const FEED_BYTES: usize = 64 * 1024;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(CHECK_KIND, |context| async move {
        check(&context.state).await?;
        Ok(())
    });
    registry.every(CHECK_KIND, Duration::from_secs(24 * 60 * 60));
}

/// What a release feed says.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    latest: String,
    #[serde(default)]
    notes_url: Option<String>,
}

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewerRelease {
    pub version: String,
    pub notes_url: Option<String>,
}

/// `major.minor.patch` as numbers; None for anything else.
fn parse(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version
        .trim()
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

fn newer_than_running(version: &str) -> bool {
    matches!((parse(version), parse(RUNNING)), (Some(latest), Some(running)) if latest > running)
}

/// Reads the feed, when one is configured, and keeps what it names.
async fn check(state: &crate::AppState) -> Result<(), AppError> {
    let Some(url) = state.config.update_check_url.clone() else {
        return Ok(());
    };
    let client = crate::outbound::client_for(
        &url,
        false,
        crate::outbound::Timeouts {
            connect: Duration::from_secs(5),
            total: Some(Duration::from_secs(15)),
        },
        Some(USER_AGENT),
    )
    .await
    .map_err(|_| AppError::Internal("the release feed's address was refused"))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::Internal("the release feed didn't answer"))?;
    if !response.status().is_success() {
        return Err(AppError::Internal("the release feed refused"));
    }
    // Read up to the limit and no further: a feed that sends more is
    // refused before it is held.
    let too_large = AppError::Internal("the release feed is too large");
    if response
        .content_length()
        .is_some_and(|length| length > FEED_BYTES as u64)
    {
        return Err(too_large);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::Internal("the release feed didn't answer"))?
    {
        if bytes.len() + chunk.len() > FEED_BYTES {
            return Err(too_large);
        }
        bytes.extend_from_slice(&chunk);
    }
    let feed = serde_json::from_slice::<Feed>(&bytes)
        .map_err(|_| AppError::Internal("the release feed isn't one"))?;
    let Some((major, minor, patch)) = parse(&feed.latest) else {
        return Err(AppError::Internal("the release feed names no version"));
    };
    let notes_url = feed
        .notes_url
        .filter(|notes| notes.len() <= 500 && notes.starts_with("https://"));
    record(
        &state.pool,
        &format!("{major}.{minor}.{patch}"),
        notes_url.as_deref(),
    )
    .await
}

/// Keeps the newest version a feed named.
pub async fn record(pool: &PgPool, latest: &str, notes_url: Option<&str>) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO server_release_checks (id, latest, notes_url, checked_at) \
         VALUES (1, $1, $2, now()) \
         ON CONFLICT (id) DO UPDATE SET latest = EXCLUDED.latest, \
             notes_url = EXCLUDED.notes_url, checked_at = EXCLUDED.checked_at",
    )
    .bind(latest)
    .bind(notes_url)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(())
}

/// The newest release the feed named, when it is newer than this build.
pub async fn newer(pool: &PgPool) -> Result<Option<NewerRelease>, AppError> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT latest, notes_url FROM server_release_checks WHERE id = 1",
    )
    .fetch_optional(crate::db::scoped(pool))
    .await?;
    Ok(row
        .filter(|(latest, _)| newer_than_running(latest))
        .map(|(version, notes_url)| NewerRelease { version, notes_url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse("0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse("1.2"), None);
        assert_eq!(parse("1.2.3.4"), None);
        assert_eq!(parse("v1.2.3"), None);
        assert!(newer_than_running("999.0.0"));
        assert!(!newer_than_running(RUNNING));
        assert!(!newer_than_running("0.0.0"));
        assert!(!newer_than_running("not a version"));
    }
}
