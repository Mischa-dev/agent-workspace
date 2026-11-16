//! Keyword alerts (C2.13): words a person wants to hear about. A message
//! naming one, in a conversation the person can read and hasn't muted, tells
//! them (`notifications::keyword_messages`). Each account keeps its own list
//! per workspace; nobody else reads it.

pub mod ops;

use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::AppError;

/// Words one person may keep per workspace.
pub const MAX_KEYWORDS: usize = 50;

/// A keyword as kept: trimmed, 2 to 60 characters, no control characters.
pub fn normalize(keyword: &str) -> Option<String> {
    let keyword = keyword.split_whitespace().collect::<Vec<_>>().join(" ");
    ((2..=60).contains(&keyword.chars().count()) && !keyword.chars().any(char::is_control))
        .then_some(keyword)
}

/// One account's keywords in a workspace, in the order they were added.
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT keyword FROM notification_keywords \
         WHERE workspace_id = $1 AND account_id = $2 \
         ORDER BY created_at, lower(keyword)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// Replaces one account's keywords in a workspace with `keywords` (already
/// normalized, at most `MAX_KEYWORDS`). Words kept keep when they were added.
pub async fn replace(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
    keywords: &[String],
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM notification_keywords \
         WHERE workspace_id = $1 AND account_id = $2 \
           AND lower(keyword) <> ALL(SELECT lower(word) FROM UNNEST($3::text[]) AS word)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(keywords)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO notification_keywords (workspace_id, account_id, keyword, created_at) \
         SELECT $1, $2, word, now() + make_interval(secs => position / 1000000.0) \
         FROM UNNEST($3::text[]) WITH ORDINALITY AS given(word, position) \
         ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(keywords)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn keywords_are_trimmed_words() {
        assert_eq!(
            normalize("  launch   date "),
            Some("launch date".to_owned())
        );
        assert_eq!(normalize("x"), None);
        assert_eq!(normalize(&"a".repeat(61)), None);
        assert_eq!(normalize("bad\u{7}"), None);
    }
}
