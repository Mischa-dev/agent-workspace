//! Making and changing spaces (K4.7): the one place a space is made,
//! renamed, given an icon, archived and brought back. The space operations
//! (`spaces::ops`), the web app's project routes and workspace templates
//! all call these after checking the caller may; each writes its event
//! (`space.created`, `space.updated`, `space.archived`, `space.restored`)
//! in the same transaction.

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{SpaceKind, SpaceVisibility};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
};

/// Longest space name, in bytes.
pub const MAX_NAME_BYTES: usize = 120;
/// Longest description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// Longest icon, in characters (an emoji with its modifiers).
pub const MAX_ICON_CHARS: usize = 16;

/// Who a new space is made for: they become its owner.
pub struct Owner<'a> {
    pub account_id: Uuid,
    pub display_name: &'a str,
    pub login_name: &'a str,
}

/// A space to make.
pub struct NewSpace<'a> {
    pub workspace_id: Uuid,
    pub kind: SpaceKind,
    /// Already normalised (`normalize_key`).
    pub key: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub visibility: SpaceVisibility,
    pub icon: Option<&'a str>,
    pub owner: Owner<'a>,
}

/// A space's key: 2 to 10 letters or digits, starting with a letter, in
/// capitals.
pub fn normalize_key(value: &str) -> Result<String, AppError> {
    let key = value.trim().to_ascii_uppercase();
    let mut characters = key.chars();
    let Some(first) = characters.next() else {
        return Err(AppError::Validation("Project key"));
    };
    if !(2..=10).contains(&key.len())
        || !first.is_ascii_uppercase()
        || !characters.all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
    {
        return Err(AppError::Validation("Project key"));
    }
    Ok(key)
}

/// A space's name: 1 to 120 bytes on one line.
pub fn normalize_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(AppError::Validation("Project name"));
    }
    Ok(name.to_owned())
}

/// A description: up to 500 characters on one line.
pub fn normalize_description(value: &str) -> Result<String, AppError> {
    let description = value.trim();
    if description.chars().count() > MAX_DESCRIPTION_CHARS
        || description.chars().any(char::is_control)
    {
        return Err(AppError::Validation("description"));
    }
    Ok(description.to_owned())
}

/// An icon: one emoji (up to 16 characters with its modifiers, no spaces),
/// or empty for none.
pub fn normalize_icon(value: &str) -> Result<Option<String>, AppError> {
    let icon = value.trim();
    if icon.is_empty() {
        return Ok(None);
    }
    if icon.chars().count() > MAX_ICON_CHARS
        || icon
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || icon
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return Err(AppError::Validation("icon"));
    }
    Ok(Some(icon.to_owned()))
}

/// A member's handle in a space, from their login name.
pub fn membership_handle(login_name: &str) -> String {
    let mut handle = login_name
        .chars()
        .filter_map(|character| {
            character
                .is_ascii_alphanumeric()
                .then_some(character.to_ascii_lowercase())
                .or_else(|| matches!(character, '_' | '-').then_some(character))
                .or_else(|| (character == '.').then_some('_'))
        })
        .take(32)
        .collect::<String>();
    if handle.len() < 2 {
        handle.push_str("_user");
    }
    handle
}

/// Makes a space with everything it starts with: its owner, its task
/// counter, the default workflow, empty instructions and the owner's place
/// in it. Projects and team spaces are made here; the workspace's own space
/// is made with the workspace, and personal spaces by their account.
pub async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    new: NewSpace<'_>,
) -> Result<Uuid, AppError> {
    if !matches!(new.kind, SpaceKind::Project | SpaceKind::Team) {
        return Err(AppError::Refused {
            field: "kind",
            message: "Only projects and team spaces are made this way.",
        });
    }
    // H1.24: no new space in a suspended or deleting workspace, whichever
    // edge asked.
    crate::workspace::lifecycle::ensure_open(&mut **transaction, new.workspace_id, "space_create")
        .await?;
    let space_id = Uuid::now_v7();
    let insert = sqlx::query(
        r#"
        INSERT INTO spaces (
            id, workspace_id, key, name, kind, description, visibility,
            created_by_actor_id, icon
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(space_id)
    .bind(new.workspace_id)
    .bind(new.key)
    .bind(new.name)
    .bind(new.kind.stored())
    .bind(new.description)
    .bind(new.visibility.stored())
    .bind(new.owner.account_id)
    .bind(new.icon)
    .execute(&mut **transaction)
    .await;
    if let Err(error) = insert {
        if constraint(&error) == Some("projects_organization_key_unique") {
            return Err(AppError::ProjectKeyConflict);
        }
        return Err(error.into());
    }
    furnish(
        transaction,
        space_id,
        new.workspace_id,
        new.kind,
        &new.owner,
    )
    .await?;
    Ok(space_id)
}

/// Everything a new space starts with, and its `space.created` event: its
/// owner's membership, its task counter, the default workflow, empty
/// instructions and the owner's place in it.
pub(super) async fn furnish(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    workspace_id: Uuid,
    kind: SpaceKind,
    owner: &Owner<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(space_id)
    .bind(owner.account_id)
    .bind(owner.display_name)
    .bind(membership_handle(owner.login_name))
    .execute(&mut **transaction)
    .await?;
    sqlx::query("INSERT INTO project_counters (project_id, next_work_item_number) VALUES ($1, 1)")
        .bind(space_id)
        .execute(&mut **transaction)
        .await?;
    crate::work_items::ensure_default_workflow(transaction, space_id).await?;
    sqlx::query("INSERT INTO project_instructions (project_id, content) VALUES ($1, '')")
        .bind(space_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, context_drawer_open,
            filters, last_opened_at, version
        )
        VALUES ($1, $2, 'project_overview', false, '{}'::jsonb, now(), 1)
        "#,
    )
    .bind(space_id)
    .bind(owner.account_id)
    .execute(&mut **transaction)
    .await?;
    events::emit(
        &mut **transaction,
        NewEvent::thing(&catalog::SPACE_CREATED, space_id, "space", space_id)
            .workspace(workspace_id)
            .actor(owner.account_id)
            .version(1)
            .payload(serde_json::json!({ "kind": kind })),
    )
    .await?;
    Ok(())
}

/// What a change sets; `None` leaves a field as it is. `icon: Some(None)`
/// takes the icon away.
#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<Option<String>>,
    pub visibility: Option<SpaceVisibility>,
}

/// A space as a change sees it, locked.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Locked {
    pub workspace_id: Uuid,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub visibility: String,
    pub archived: bool,
    pub version: i64,
}

impl Locked {
    pub fn kind(&self) -> Option<SpaceKind> {
        SpaceKind::from_stored(&self.kind)
    }
}

/// The space, locked for a change, or `None` when there is no such space
/// (or it is the hidden Messages project, which is not a space).
pub async fn lock(
    connection: &mut PgConnection,
    space_id: Uuid,
) -> Result<Option<Locked>, AppError> {
    let row = sqlx::query_as::<_, Locked>(
        r#"
        SELECT workspace_id, kind, name, description, icon, visibility,
               archived_at IS NOT NULL AS archived, version
        FROM spaces WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(space_id)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.filter(|row| row.kind().is_some()))
}

/// Applies `changes` to a locked, active space: what changed, by key, and
/// the new version (unchanged when nothing did). The workspace's own space
/// is open to the whole workspace, so its visibility stays.
pub async fn update(
    connection: &mut PgConnection,
    actor: Uuid,
    space_id: Uuid,
    current: &Locked,
    changes: Changes,
) -> Result<(Vec<&'static str>, i64), AppError> {
    if current.archived {
        return Err(AppError::ProjectArchived);
    }
    let mut changed = Vec::new();
    if changes
        .name
        .as_ref()
        .is_some_and(|name| *name != current.name)
    {
        changed.push("name");
    }
    if changes
        .description
        .as_ref()
        .is_some_and(|description| *description != current.description)
    {
        changed.push("description");
    }
    if changes
        .icon
        .as_ref()
        .is_some_and(|icon| *icon != current.icon)
    {
        changed.push("icon");
    }
    if let Some(visibility) = changes.visibility
        && visibility.stored() != current.visibility
    {
        if current.kind() != Some(SpaceKind::Project) && current.kind() != Some(SpaceKind::Team) {
            return Err(AppError::Refused {
                field: "visibility",
                message: "Only a project or team space can change who may find it.",
            });
        }
        changed.push("visibility");
    }
    if changed.is_empty() {
        return Ok((changed, current.version));
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE spaces
        SET name = COALESCE($2, name),
            description = COALESCE($3, description),
            icon = CASE WHEN $4 THEN $5 ELSE icon END,
            visibility = COALESCE($6, visibility),
            version = version + 1, updated_at = now()
        WHERE id = $1
        RETURNING version
        "#,
    )
    .bind(space_id)
    .bind(changes.name)
    .bind(changes.description)
    .bind(changes.icon.is_some())
    .bind(changes.icon.flatten())
    .bind(changes.visibility.map(SpaceVisibility::stored))
    .fetch_one(&mut *connection)
    .await?;
    events::emit_on(
        &mut *connection,
        NewEvent::thing(&catalog::SPACE_UPDATED, space_id, "space", space_id)
            .workspace(current.workspace_id)
            .actor(actor)
            .version(version)
            .changes(&changed),
    )
    .await?;
    Ok((changed, version))
}

/// Archives a locked, active space: nothing in it can change until it is
/// brought back. A space with a run still going is not archived. The
/// workspace's own space and personal spaces stay.
pub async fn archive(
    connection: &mut PgConnection,
    actor: Uuid,
    space_id: Uuid,
    current: &Locked,
) -> Result<i64, AppError> {
    if !matches!(current.kind(), Some(SpaceKind::Project | SpaceKind::Team)) {
        return Err(AppError::Refused {
            field: "space",
            message: "Only a project or team space can be archived.",
        });
    }
    if current.archived {
        return Ok(current.version);
    }
    if crate::runs::store::active_under_space(&mut *connection, space_id).await? {
        return Err(AppError::ProjectHasActiveRuns);
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE spaces
        SET archived_at = now(), archived_by_actor_id = $2,
            version = version + 1, updated_at = now()
        WHERE id = $1
        RETURNING version
        "#,
    )
    .bind(space_id)
    .bind(actor)
    .fetch_one(&mut *connection)
    .await?;
    events::emit_on(
        &mut *connection,
        NewEvent::thing(&catalog::SPACE_ARCHIVED, space_id, "space", space_id)
            .workspace(current.workspace_id)
            .actor(actor)
            .version(version),
    )
    .await?;
    Ok(version)
}

/// Brings an archived space back as it was.
pub async fn restore(
    connection: &mut PgConnection,
    actor: Uuid,
    space_id: Uuid,
    current: &Locked,
) -> Result<i64, AppError> {
    if !matches!(current.kind(), Some(SpaceKind::Project | SpaceKind::Team)) {
        return Err(AppError::Refused {
            field: "space",
            message: "Only a project or team space can be archived or restored.",
        });
    }
    if !current.archived {
        return Ok(current.version);
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE spaces
        SET archived_at = NULL, archived_by_actor_id = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1
        RETURNING version
        "#,
    )
    .bind(space_id)
    .fetch_one(&mut *connection)
    .await?;
    events::emit_on(
        &mut *connection,
        NewEvent::thing(&catalog::SPACE_RESTORED, space_id, "space", space_id)
            .workspace(current.workspace_id)
            .actor(actor)
            .version(version),
    )
    .await?;
    Ok(version)
}

fn constraint(error: &sqlx::Error) -> Option<&str> {
    match error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_names_and_icons_are_checked() {
        assert_eq!(normalize_key(" web ").unwrap(), "WEB");
        assert!(normalize_key("1AB").is_err());
        assert!(normalize_key("A").is_err());
        assert!(normalize_name("  ").is_err());
        assert_eq!(normalize_icon("🚀").unwrap().as_deref(), Some("🚀"));
        assert_eq!(normalize_icon(" ").unwrap(), None);
        assert!(normalize_icon("ab").is_err());
        assert!(normalize_icon("🚀 🚀").is_err());
        assert!(normalize_key("TOO-LONG-KEY").is_err());
        assert!(normalize_name("bad\nname").is_err());
        assert!(normalize_name(&"x".repeat(121)).is_err());
        assert_eq!(membership_handle("Ada.Lovelace"), "ada_lovelace");
        assert!(membership_handle(&"a".repeat(64)).len() <= 32);
        assert_eq!(membership_handle("x"), "x_user");
    }
}
