//! Personal spaces (K4.9, K4 §3.9): one per account per workspace, made the
//! first time something needs it ("My space" opened, a page saved there),
//! never eagerly. Its owner is its only member, as `owner`; it is
//! `members`-visible, so nobody else reads it, workspace admins included.
//! It starts with Pages and a "Notes" page; its kind's starting modules
//! (`SpaceKind::starting_modules`) hold what it shows.
//!
//! Two first calls at once make one space: the insert yields on the unique
//! index `projects_one_personal_space` and the loser reads the winner's.
//! Leaving the workspace hides it with the workspace membership; coming
//! back shows it again. Retiring an agent archives its space.

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{SpaceKind, SpaceVisibility, manage};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    memory::Writer,
};

/// What a space key may be suffixed with when the first choice is taken.
const KEY_SUFFIXES: &str = "0123456789";

/// The account's personal space in the workspace, made on first use, or
/// `None` when the account isn't an active, enabled member of the workspace.
/// An archived one (a retired agent's) is returned as it is: the caller's
/// access check refuses it.
pub async fn personal(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    account: Uuid,
) -> Result<Option<Uuid>, AppError> {
    if let Some(space) = existing(transaction, workspace, account).await? {
        return Ok(Some(space));
    }
    let Some((display_name, handle, actor_kind)) = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT profile.display_name, profile.handle, actor.kind
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        JOIN organization_actor_profiles profile
          ON profile.organization_id = membership.organization_id
         AND profile.actor_id = membership.actor_id
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled
        "#,
    )
    .bind(workspace)
    .bind(account)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    crate::workspace::lifecycle::ensure_open(transaction, workspace, "space_personal_open").await?;
    let base = key_for(account);
    for suffix in std::iter::once(None).chain(KEY_SUFFIXES.chars().map(Some)) {
        let key = match suffix {
            Some(character) => format!("{base}{character}"),
            None => base.clone(),
        };
        let space_id = Uuid::now_v7();
        // Yields on either unique index: one personal space per account per
        // workspace, and one key per workspace.
        let made = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO projects (
                id, organization_id, project_key, name, kind, description, visibility,
                owner_actor_id, created_by_actor_id
            )
            VALUES ($1, $2, $3, $4, $5, '', $6, $7, $7)
            ON CONFLICT DO NOTHING
            RETURNING id
            "#,
        )
        .bind(space_id)
        .bind(workspace)
        .bind(&key)
        .bind(&display_name)
        .bind(SpaceKind::Personal.stored())
        .bind(SpaceVisibility::Members.stored())
        .bind(account)
        .fetch_optional(&mut **transaction)
        .await?;
        if made.is_none() {
            // Someone else made it first, or the key is taken.
            if let Some(space) = existing(transaction, workspace, account).await? {
                return Ok(Some(space));
            }
            continue;
        }
        manage::furnish(
            transaction,
            space_id,
            workspace,
            SpaceKind::Personal,
            &manage::Owner {
                account_id: account,
                display_name: &display_name,
                login_name: &handle,
            },
        )
        .await?;
        let writer = Writer {
            actor_id: account,
            actor_kind: crate::accounts::actor_kind(&actor_kind)?,
            via_grant_id: None,
            confined_project: None,
        };
        // K4.20: its starting shape, a "Notes" page.
        super::shape::apply(
            transaction,
            writer,
            space_id,
            &super::shape::builtin(SpaceKind::Personal),
        )
        .await?;
        return Ok(Some(space_id));
    }
    Err(AppError::ProjectKeyConflict)
}

/// The account's personal space in the workspace, if it was made.
pub async fn existing(
    connection: &mut PgConnection,
    workspace: Uuid,
    account: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 AND owner_actor_id = $2 AND kind = $3",
    )
    .bind(workspace)
    .bind(account)
    .bind(SpaceKind::Personal.stored())
    .fetch_optional(&mut *connection)
    .await?)
}

/// Archives the account's personal space in the workspace, when it has
/// one: an agent retired from the workspace keeps its notes and pages, read
/// by nobody until it's restored.
pub async fn archive_for(
    connection: &mut PgConnection,
    workspace: Uuid,
    account: Uuid,
    by: Uuid,
) -> Result<(), AppError> {
    let archived = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        UPDATE projects
        SET archived_at = now(), archived_by_actor_id = $4, version = version + 1,
            updated_at = now()
        WHERE organization_id = $1 AND owner_actor_id = $2 AND kind = $3
          AND archived_at IS NULL
        RETURNING id, version
        "#,
    )
    .bind(workspace)
    .bind(account)
    .bind(SpaceKind::Personal.stored())
    .bind(by)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some((space, version)) = archived {
        events::emit(
            &mut *connection,
            NewEvent::thing(&catalog::SPACE_ARCHIVED, space, "space", space)
                .workspace(workspace)
                .actor(by)
                .version(version),
        )
        .await?;
    }
    Ok(())
}

/// A personal space's key: `U` and the last six hex digits of the account
/// id (the random end of a v7 id), in capitals.
fn key_for(account: Uuid) -> String {
    let hex = account.simple().to_string().to_ascii_uppercase();
    format!("U{}", &hex[hex.len() - 6..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_space_keys() {
        let account = Uuid::now_v7();
        let key = key_for(account);
        assert_eq!(key.len(), 7);
        assert_eq!(manage::normalize_key(&key).ok(), Some(key.clone()));
        assert_eq!(
            manage::normalize_key(&format!("{key}9")).ok(),
            Some(format!("{key}9"))
        );
    }
}
