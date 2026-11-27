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
    /// The project or team space it is made inside, if any.
    pub inside: Option<Inside>,
}

/// Where a space sits: inside another one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inside {
    pub parent: Uuid,
    /// Its people are its parent's: everyone there holds a seat here with
    /// the same role, kept in step by the database (`space_group_seat_sync`).
    pub people_from_parent: bool,
}

/// How many levels a space and what is under it may be (the database's
/// `space_tree_max_depth`).
pub const MAX_TREE_DEPTH: i64 = 3;

/// One workspace's moves take turns, as the database's own check does, so
/// what is read here is still true when the row is written.
pub async fn lock_tree(connection: &mut PgConnection, workspace_id: Uuid) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('space-tree:' || $1::text, 0))")
        .bind(workspace_id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// Whether `space` (a new one when `None`) may go inside `parent`: a
/// project or team space of the same workspace that isn't archived, isn't
/// the space or something under it, and leaves the tree within its depth.
/// The database refuses the same things; this says why in words.
pub async fn check_parent(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space: Option<Uuid>,
    parent: Uuid,
) -> Result<(), AppError> {
    lock_tree(connection, workspace_id).await?;
    let found = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT workspace_id, kind, archived_at IS NOT NULL FROM spaces WHERE id = $1",
    )
    .bind(parent)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((parent_workspace, kind, archived)) = found else {
        return Err(AppError::NotFound("project"));
    };
    if parent_workspace != workspace_id {
        return Err(AppError::NotFound("project"));
    }
    if !matches!(
        SpaceKind::from_stored(&kind),
        Some(SpaceKind::Project | SpaceKind::Team)
    ) {
        return Err(AppError::Refused {
            field: "parentId",
            message: "A project goes inside a project or a team space.",
        });
    }
    if archived {
        return Err(AppError::Refused {
            field: "parentId",
            message: "That project is archived. Restore it first, or pick another.",
        });
    }
    let (above, loops) = sqlx::query_as::<_, (i64, bool)>(
        r#"
        WITH RECURSIVE up AS (
            SELECT space.id, space.parent_space_id, 1 AS level
            FROM spaces space WHERE space.id = $1
            UNION ALL
            SELECT space.id, space.parent_space_id, up.level + 1
            FROM spaces space JOIN up ON space.id = up.parent_space_id
            WHERE up.level < 16
        )
        SELECT COALESCE(max(level), 0)::bigint, COALESCE(bool_or(id = $2), false) FROM up
        "#,
    )
    .bind(parent)
    .bind(space)
    .fetch_one(&mut *connection)
    .await?;
    if loops {
        return Err(AppError::Refused {
            field: "parentId",
            message: "A project can't go inside itself or inside something under it.",
        });
    }
    let below = match space {
        Some(space) => {
            sqlx::query_scalar::<_, i64>(
                r#"
                WITH RECURSIVE down AS (
                    SELECT space.id, 1 AS level FROM spaces space WHERE space.parent_space_id = $1
                    UNION ALL
                    SELECT space.id, down.level + 1
                    FROM spaces space JOIN down ON space.parent_space_id = down.id
                    WHERE down.level < 16
                )
                SELECT COALESCE(max(level), 0)::bigint FROM down
                "#,
            )
            .bind(space)
            .fetch_one(&mut *connection)
            .await?
        }
        None => 0,
    };
    if above + 1 + below > MAX_TREE_DEPTH {
        return Err(AppError::Refused {
            field: "parentId",
            message: "Projects nest three levels deep at most. Put it one level higher.",
        });
    }
    Ok(())
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
    crate::workspace::lifecycle::ensure_open(transaction, new.workspace_id, "space_create").await?;
    if let Some(inside) = new.inside {
        check_parent(transaction, new.workspace_id, None, inside.parent).await?;
    }
    let space_id = Uuid::now_v7();
    let insert = sqlx::query(
        r#"
        INSERT INTO spaces (
            id, workspace_id, key, name, kind, description, visibility,
            created_by_actor_id, icon, parent_space_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
    .bind(new.inside.map(|inside| inside.parent))
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
    // Its owner has their own seat by now, so the parent's people join
    // around it.
    if new.inside.is_some_and(|inside| inside.people_from_parent) {
        sqlx::query("UPDATE spaces SET people_from_parent = true WHERE id = $1")
            .bind(space_id)
            .execute(&mut **transaction)
            .await?;
    }
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
        transaction,
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
    /// `Some(None)` moves it to the top; `Some(Some(parent))` inside
    /// another space.
    pub parent: Option<Option<Uuid>>,
    /// Whether its people are its parent's.
    pub people_from_parent: Option<bool>,
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
    pub parent_space_id: Option<Uuid>,
    pub people_from_parent: bool,
    /// When it was archived: what went with it carries the same moment.
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
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
               archived_at IS NOT NULL AS archived, version,
               parent_space_id, people_from_parent, archived_at
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
    let parent = changes.parent.unwrap_or(current.parent_space_id);
    if parent != current.parent_space_id {
        if !matches!(current.kind(), Some(SpaceKind::Project | SpaceKind::Team)) {
            return Err(AppError::Refused {
                field: "parentId",
                message: "Only a project or team space can be moved.",
            });
        }
        if let Some(parent) = parent {
            check_parent(connection, current.workspace_id, Some(space_id), parent).await?;
        }
        changed.push("parent");
    }
    // At the top a space has its own people. One that had its parent's
    // people and moves under another takes the new parent's.
    let people_from_parent = parent.is_some()
        && changes
            .people_from_parent
            .unwrap_or(current.people_from_parent);
    if changes.people_from_parent == Some(true) && parent.is_none() {
        return Err(AppError::Refused {
            field: "people",
            message: "Only a project inside another can have that one's people. Give it a \
                parent first.",
        });
    }
    if people_from_parent != current.people_from_parent {
        changed.push("people");
    }
    if changed.is_empty() {
        return Ok((changed, current.version));
    }
    if current.people_from_parent && !people_from_parent {
        // Its own people from here on: everyone the parent gave a seat
        // keeps it as their own, so nobody loses the space by the change.
        sqlx::query(
            "UPDATE project_memberships SET via_group = false \
             WHERE project_id = $1 AND via_parent AND enabled",
        )
        .bind(space_id)
        .execute(&mut *connection)
        .await?;
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE spaces
        SET name = COALESCE($2, name),
            description = COALESCE($3, description),
            icon = CASE WHEN $4 THEN $5 ELSE icon END,
            visibility = COALESCE($6, visibility),
            parent_space_id = $7,
            people_from_parent = $8,
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
    .bind(parent)
    .bind(people_from_parent)
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

/// The active spaces under `space_id`, nearest first.
async fn active_below(
    connection: &mut PgConnection,
    space_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        WITH RECURSIVE down AS (
            SELECT space.id, space.version, 1 AS level
            FROM spaces space
            WHERE space.parent_space_id = $1 AND space.archived_at IS NULL
            UNION ALL
            SELECT space.id, space.version, down.level + 1
            FROM spaces space JOIN down ON space.parent_space_id = down.id
            WHERE space.archived_at IS NULL AND down.level < 16
        )
        SELECT id, version FROM down ORDER BY level, id
        "#,
    )
    .bind(space_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// Archives a locked, active space and every space under it: nothing in
/// them can change until it is brought back. A space with a run still
/// going, here or under it, is not archived. The workspace's own space and
/// personal spaces stay. Answers the space's new version.
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
    lock_tree(connection, current.workspace_id).await?;
    let below = active_below(connection, space_id).await?;
    let mut all = vec![space_id];
    all.extend(below.iter().map(|(id, _)| *id));
    for id in &all {
        if crate::runs::store::active_under_space(&mut *connection, *id).await? {
            return Err(AppError::ProjectHasActiveRuns);
        }
    }
    // One moment for the whole tree (the transaction's), which is how
    // restoring finds what went with it.
    let archived = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        UPDATE spaces
        SET archived_at = now(), archived_by_actor_id = $2,
            version = version + 1, updated_at = now()
        WHERE id = ANY($1) AND archived_at IS NULL
        RETURNING id, version
        "#,
    )
    .bind(&all)
    .bind(actor)
    .fetch_all(&mut *connection)
    .await?;
    let mut version = current.version;
    for (id, new_version) in archived {
        if id == space_id {
            version = new_version;
        }
        events::emit_on(
            &mut *connection,
            NewEvent::thing(&catalog::SPACE_ARCHIVED, id, "space", id)
                .workspace(current.workspace_id)
                .actor(actor)
                .version(new_version),
        )
        .await?;
    }
    Ok(version)
}

/// Brings an archived space back as it was, with the spaces under it that
/// were archived with it. One inside an archived space waits for that one.
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
    lock_tree(connection, current.workspace_id).await?;
    if let Some(parent) = current.parent_space_id {
        let parent_archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM spaces WHERE id = $1",
        )
        .bind(parent)
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(false);
        if parent_archived {
            return Err(AppError::Refused {
                field: "space",
                message: "It is inside an archived project. Restore that one, and this comes \
                    back with it.",
            });
        }
    }
    let restored = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        WITH RECURSIVE down AS (
            SELECT space.id FROM spaces space WHERE space.id = $1
            UNION ALL
            SELECT space.id
            FROM spaces space JOIN down ON space.parent_space_id = down.id
            WHERE space.archived_at = $2
        ),
        back AS (
            UPDATE spaces
            SET archived_at = NULL, archived_by_actor_id = NULL,
                version = version + 1, updated_at = now()
            WHERE id IN (SELECT id FROM down) AND archived_at IS NOT NULL
            RETURNING id, version
        )
        SELECT id, version FROM back
        "#,
    )
    .bind(space_id)
    .bind(current.archived_at)
    .fetch_all(&mut *connection)
    .await?;
    let mut version = current.version;
    for (id, new_version) in restored {
        if id == space_id {
            version = new_version;
        }
        events::emit_on(
            &mut *connection,
            NewEvent::thing(&catalog::SPACE_RESTORED, id, "space", id)
                .workspace(current.workspace_id)
                .actor(actor)
                .version(new_version),
        )
        .await?;
    }
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
        assert_eq!(normalize_key(" web ").ok().as_deref(), Some("WEB"));
        assert!(normalize_key("1AB").is_err());
        assert!(normalize_key("A").is_err());
        assert!(normalize_name("  ").is_err());
        assert_eq!(normalize_icon("🚀").ok().flatten().as_deref(), Some("🚀"));
        assert_eq!(normalize_icon(" ").ok(), Some(None));
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
