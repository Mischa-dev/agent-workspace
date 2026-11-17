//! Built modules (M1.6): a named bundle of existing parts (collections,
//! forms, automations, pages and navigation entries) that a
//! workspace or one space switches on and off as one. Off hides every part
//! and refuses operations on them; the data stays, and switching it on
//! brings everything back. Archiving archives the parts with it, or keeps
//! them as plain objects.
//!
//! A part's owner is a row in `module_parts`; nothing is added to the parts'
//! own tables. Readers of a part kind add [`object_on_sql`] (the SQL
//! function `module_part_on`, migration `20261116220030_built_modules`).
//! The operations are in [`ops`].

pub mod ops;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

/// Who reads a thing that may live in a space: the account, the connection
/// it came through (an agent's connection may reach fewer spaces), and the
/// one space an older credential is held to. Membership alone never widens
/// what a narrower connection reaches.
#[derive(Clone, Copy, Debug)]
pub struct Reader {
    pub account: Uuid,
    pub connection: Option<Uuid>,
    pub held: Option<Uuid>,
}

impl Reader {
    pub fn of(caller: &crate::access::Caller) -> Self {
        Self {
            account: caller.account_id,
            connection: caller.connection_id,
            held: caller.confined_space,
        }
    }

    /// The SQL that says this reader reaches `space` (an expression), with
    /// the account, connection and held space bound at `$first`, `$first+1`
    /// and `$first+2`.
    pub fn reaches_sql(space: &str, first: usize) -> String {
        let (account, connection, held) = (first, first + 1, first + 2);
        format!(
            "(actor_can_access_project(${account}, ${connection}::uuid, {space}) \
             AND (${held}::uuid IS NULL OR {space} = ${held}::uuid))"
        )
    }
}

/// Built modules' keys are `local.<name>`: a connected install's own kinds
/// are `<install>.<name>` (ruling R54), and `local` is never an install.
pub const KEY_PREFIX: &str = "local";

/// The part kinds a module may own and `module_part_link` takes.
/// (Channels wait for K3's link store: a members-only channel's name must
/// stay with its members.)
pub const LINKABLE: &[&str] = &["collection", "form", "automation", "page"];

/// Nav entries a module holds, at most.
pub const MAX_NAV: usize = 20;
/// Parts a module holds, at most.
pub const MAX_PARTS: usize = 100;

/// SQL that is true when the object in `id_column` is on as far as built
/// modules go: not a part of a module switched off (in its workspace or its
/// space) or archived with its parts. An object in no module is always on.
pub fn object_on_sql(part_kind: &'static str, id_column: &str) -> String {
    debug_assert!(LINKABLE.contains(&part_kind) || part_kind == "nav");
    format!("module_part_on('{part_kind}', {id_column})")
}

/// Whether one object is on (see [`object_on_sql`]).
pub async fn object_on(
    connection: &mut PgConnection,
    part_kind: &'static str,
    object_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT module_part_on($1, $2)")
            .bind(part_kind)
            .bind(object_id)
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// The full key for a name given with or without its prefix: `playtest`
/// and `local.playtest` are both `local.playtest`. None when it can't be a
/// key (lowercase letters, digits and `_`, starting with a letter, 1 to 40).
pub fn full_key(given: &str) -> Option<String> {
    let given = given.trim();
    let name = given
        .strip_prefix(KEY_PREFIX)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(given);
    let valid = (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("{KEY_PREFIX}.{name}"))
}

#[derive(Clone, Debug, FromRow)]
pub struct ModuleRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub module_key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub revision: i64,
    pub archived_parts: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

const MODULE_COLUMNS: &str = "id, workspace_id, space_id, module_key, name, description, icon, \
     revision, archived_parts, created_at, updated_at, archived_at";

/// A module by id, in a workspace.
pub async fn get(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<Option<ModuleRow>, AppError> {
    Ok(sqlx::query_as::<_, ModuleRow>(&format!(
        "SELECT {MODULE_COLUMNS} FROM modules WHERE workspace_id = $1 AND id = $2"
    ))
    .bind(workspace_id)
    .bind(module_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A module by id, locked for a change.
pub async fn lock(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<Option<ModuleRow>, AppError> {
    Ok(sqlx::query_as::<_, ModuleRow>(&format!(
        "SELECT {MODULE_COLUMNS} FROM modules WHERE workspace_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(workspace_id)
    .bind(module_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The workspace-wide modules, live ones first; with `space`, that space's
/// own too. (A space's modules are listed only to those who name it.)
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space: Option<Uuid>,
    include_archived: bool,
) -> Result<Vec<ModuleRow>, AppError> {
    Ok(sqlx::query_as::<_, ModuleRow>(&format!(
        "SELECT {MODULE_COLUMNS} FROM modules \
         WHERE workspace_id = $1 AND (space_id IS NULL OR space_id = $2) \
           AND ($3 OR archived_at IS NULL) \
         ORDER BY archived_at IS NOT NULL, name, id LIMIT 200"
    ))
    .bind(workspace_id)
    .bind(space)
    .bind(include_archived)
    .fetch_all(&mut *connection)
    .await?)
}

/// Whether a module is switched on where it lives.
pub async fn is_on(connection: &mut PgConnection, module: &ModuleRow) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS (SELECT 1 FROM module_installs \
         WHERE workspace_id = $1 AND module_key = $2 AND state = 'off' \
           AND (space_id IS NULL OR space_id = $3))",
    )
    .bind(module.workspace_id)
    .bind(&module.module_key)
    .bind(module.space_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// A new module row. Refuses a key a live module has.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    module_key: &str,
    name: &str,
    description: &str,
    icon: Option<&str>,
    actor_id: Uuid,
) -> Result<Uuid, AppError> {
    let id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO modules (
            id, workspace_id, space_id, module_key, name, description, icon,
            created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (workspace_id, module_key) WHERE archived_at IS NULL DO NOTHING
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(module_key)
    .bind(name)
    .bind(description)
    .bind(icon)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?;
    inserted.ok_or(AppError::Conflict(
        "module",
        "a module with that key already exists here",
    ))
}

/// Bumps a module's revision after a change.
pub async fn touch(connection: &mut PgConnection, module_id: Uuid) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "UPDATE modules SET revision = revision + 1, updated_at = now() WHERE id = $1 \
         RETURNING revision",
    )
    .bind(module_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// Where an object of a linkable kind lives: its workspace and space.
pub async fn object_place(
    connection: &mut PgConnection,
    part_kind: &str,
    object_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let sql = match part_kind {
        "collection" => {
            "SELECT project.organization_id, project.id FROM collections object \
             JOIN projects project ON project.id = object.project_id \
             WHERE object.id = $1 AND object.builtin IS NULL"
        }
        "form" => {
            "SELECT object.organization_id, object.project_id FROM bug_report_links object \
             WHERE object.id = $1 AND object.collection_id IS NOT NULL"
        }
        "automation" => {
            "SELECT object.organization_id, object.project_id FROM automation_rules object \
             WHERE object.id = $1"
        }
        "page" => {
            "SELECT project.organization_id, project.id FROM project_pages object \
             JOIN projects project ON project.id = object.project_id WHERE object.id = $1"
        }
        _ => return Ok(None),
    };
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(sql)
        .bind(object_id)
        .fetch_optional(&mut *connection)
        .await?)
}

/// Makes an object a part of a module. The object must be in the module's
/// workspace (and space, for a space's module), in a space `actor` is a
/// member of (and, for a connection held to one space, in that space), and
/// in no other module. Returns false when it was already this module's.
pub async fn link_part(
    connection: &mut PgConnection,
    module: &ModuleRow,
    part_kind: &'static str,
    object_id: Uuid,
    actor: Uuid,
    confined: Option<Uuid>,
) -> Result<bool, AppError> {
    let Some((workspace_id, space_id)) = object_place(connection, part_kind, object_id).await?
    else {
        return Err(AppError::NotFound("part"));
    };
    if workspace_id != module.workspace_id || module.space_id.is_some_and(|space| space != space_id)
    {
        // Another workspace's object is not found here, the same as a
        // missing one.
        return Err(AppError::NotFound("part"));
    }
    // A workspace-wide module may gather parts from several spaces, but only
    // from spaces the caller is in; another space's object is not found.
    let member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM project_memberships \
         WHERE project_id = $1 AND actor_id = $2 AND enabled)",
    )
    .bind(space_id)
    .bind(actor)
    .fetch_one(&mut *connection)
    .await?;
    if !member || confined.is_some_and(|confined| confined != space_id) {
        return Err(AppError::NotFound("part"));
    }
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT module_id FROM module_parts WHERE part_kind = $1 AND object_id = $2",
    )
    .bind(part_kind)
    .bind(object_id)
    .fetch_optional(&mut *connection)
    .await?;
    match owner {
        Some(owner) if owner == module.id => return Ok(false),
        Some(_) => {
            return Err(AppError::Conflict(
                "module_part",
                "that part already belongs to another module",
            ));
        }
        None => {}
    }
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM module_parts WHERE module_id = $1")
            .bind(module.id)
            .fetch_one(&mut *connection)
            .await?;
    if count >= MAX_PARTS as i64 {
        return Err(AppError::Validation("parts"));
    }
    sqlx::query(
        r#"
        INSERT INTO module_parts (module_id, workspace_id, space_id, part_kind, object_id, position)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(module.id)
    .bind(module.workspace_id)
    .bind(space_id)
    .bind(part_kind)
    .bind(object_id)
    .bind(i32::try_from(count).unwrap_or(i32::MAX))
    .execute(&mut *connection)
    .await?;
    Ok(true)
}

/// Detaches a part: the object stays, as a plain one. Returns whether it
/// was this module's part. A nav entry's row goes with it.
pub async fn unlink_part(
    connection: &mut PgConnection,
    module: &ModuleRow,
    part_kind: &str,
    object_id: Uuid,
) -> Result<bool, AppError> {
    let removed = sqlx::query(
        "DELETE FROM module_parts WHERE module_id = $1 AND part_kind = $2 AND object_id = $3",
    )
    .bind(module.id)
    .bind(part_kind)
    .bind(object_id)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        > 0;
    if removed && part_kind == "nav" {
        sqlx::query("DELETE FROM module_nav WHERE module_id = $1 AND id = $2")
            .bind(module.id)
            .bind(object_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(removed)
}

/// A navigation entry, as written and as read.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavEntry {
    /// What the sidebar shows: 1 to 40 characters.
    pub label: String,
    /// An icon name (lowercase letters, digits and dashes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Where it goes: {"kind": "collection", "collection": "<id or inline
    /// name>"}, {"kind": "page", "page": "<id>"} or {"kind": "form",
    /// "form": "<id or inline name>"}.
    pub target: serde_json::Value,
}

/// Adds a nav entry (and its part row).
pub async fn add_nav(
    connection: &mut PgConnection,
    module: &ModuleRow,
    label: &str,
    icon: Option<&str>,
    target: &serde_json::Value,
) -> Result<Uuid, AppError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 40 || label.chars().any(char::is_control) {
        return Err(AppError::Validation("nav.label"));
    }
    if icon.is_some_and(|icon| !valid_icon(icon)) {
        return Err(AppError::Validation("nav.icon"));
    }
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM module_nav WHERE module_id = $1")
            .bind(module.id)
            .fetch_one(&mut *connection)
            .await?;
    if count >= MAX_NAV as i64 {
        return Err(AppError::Validation("nav"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO module_nav (id, module_id, workspace_id, label, icon, target, position)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(module.id)
    .bind(module.workspace_id)
    .bind(label)
    .bind(icon)
    .bind(target)
    .bind(i32::try_from(count).unwrap_or(i32::MAX))
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO module_parts (module_id, workspace_id, space_id, part_kind, object_id, position)
        VALUES ($1, $2, $3, 'nav', $4, $5)
        "#,
    )
    .bind(module.id)
    .bind(module.workspace_id)
    .bind(module.space_id)
    .bind(id)
    .bind(i32::try_from(count).unwrap_or(i32::MAX))
    .execute(&mut *connection)
    .await?;
    Ok(id)
}

pub fn valid_icon(icon: &str) -> bool {
    (1..=40).contains(&icon.len())
        && icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One part as `module_open` shows it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
    /// collection, form, automation, page or nav.
    pub kind: String,
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Its name, as its own module shows it.
    pub name: Option<String>,
}

#[derive(FromRow)]
struct PartRow {
    part_kind: String,
    object_id: Uuid,
    name: Option<String>,
}

/// Whether `reader` may know of a part (`part` aliases `module_parts`): one
/// in a space only to a reader who reaches that space, a channel only to
/// those who can see it. A workspace-wide module may gather parts from
/// several spaces, and anyone in the workspace can open it.
fn part_visible() -> String {
    format!(
        "(part.space_id IS NULL OR {}) \
         AND (part.part_kind <> 'channel' OR conversation_visible_to(part.object_id, $2))",
        Reader::reaches_sql("part.space_id", 2)
    )
}

/// A module's parts that `reader` may know of, in order, each with its name.
pub async fn parts(
    connection: &mut PgConnection,
    module_id: Uuid,
    reader: Reader,
) -> Result<Vec<PartView>, AppError> {
    let visible = part_visible();
    let rows = sqlx::query_as::<_, PartRow>(&format!(
        r#"
        SELECT part.part_kind, part.object_id,
               CASE part.part_kind
                   WHEN 'collection' THEN (SELECT name FROM collections WHERE id = part.object_id)
                   WHEN 'form' THEN (SELECT name FROM bug_report_links WHERE id = part.object_id)
                   WHEN 'automation' THEN (SELECT name FROM automation_rules WHERE id = part.object_id)
                   WHEN 'page' THEN (SELECT title FROM project_pages WHERE id = part.object_id)
                   WHEN 'nav' THEN (SELECT label FROM module_nav WHERE id = part.object_id)
               END AS name
        FROM module_parts part
        WHERE part.module_id = $1 AND {visible}
        ORDER BY part.part_kind, part.position, part.object_id
        "#
    ))
    .bind(module_id)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PartView {
            kind: row.part_kind,
            id: row.object_id,
            name: row.name,
        })
        .collect())
}

#[derive(FromRow)]
struct NavRow {
    id: Uuid,
    label: String,
    icon: Option<String>,
    target: serde_json::Value,
}

/// A nav entry as read.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NavView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub label: String,
    pub icon: Option<String>,
    pub target: serde_json::Value,
}

/// A module's nav entries `reader` may follow: an entry whose collection,
/// page or form is in a space they don't reach is left out.
pub async fn nav(
    connection: &mut PgConnection,
    module_id: Uuid,
    reader: Reader,
) -> Result<Vec<NavView>, AppError> {
    Ok(sqlx::query_as::<_, NavRow>(&format!(
        r#"
        SELECT nav.id, nav.label, nav.icon, nav.target FROM module_nav nav
        WHERE nav.module_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM (
                  SELECT project_id FROM collections
                  WHERE id::text = nav.target->>'collection'
                  UNION ALL
                  SELECT project_id FROM project_pages WHERE id::text = nav.target->>'page'
                  UNION ALL
                  SELECT project_id FROM bug_report_links WHERE id::text = nav.target->>'form'
              ) goes
              WHERE NOT {reaches})
        ORDER BY nav.position, nav.id
        "#,
        reaches = Reader::reaches_sql("goes.project_id", 2)
    ))
    .bind(module_id)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| NavView {
        id: row.id,
        label: row.label,
        icon: row.icon,
        target: row.target,
    })
    .collect())
}

/// Counts of the parts `reader` may know of, per module, for lists.
pub async fn part_counts(
    connection: &mut PgConnection,
    module_ids: &[Uuid],
    reader: Reader,
) -> Result<BTreeMap<Uuid, i64>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, i64)>(&format!(
        "SELECT part.module_id, count(*) FROM module_parts part \
         WHERE part.module_id = ANY($1) AND {} GROUP BY part.module_id",
        part_visible()
    ))
    .bind(module_ids)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect())
}

/// The collection parts of a module, for events.
pub async fn collection_parts(
    connection: &mut PgConnection,
    module_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT collection.project_id, collection.id FROM module_parts part \
         JOIN collections collection ON collection.id = part.object_id \
         WHERE part.module_id = $1 AND part.part_kind = 'collection'",
    )
    .bind(module_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// Archives a module. With `keep`, its parts are detached and live on as
/// plain objects; with `archive`, they stay its parts and are hidden with
/// it until `restore`.
pub async fn archive(
    connection: &mut PgConnection,
    module: &ModuleRow,
    keep: bool,
) -> Result<(), AppError> {
    if keep {
        sqlx::query("DELETE FROM module_parts WHERE module_id = $1 AND part_kind <> 'nav'")
            .bind(module.id)
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query(
        "UPDATE modules SET archived_at = now(), archived_parts = $2, revision = revision + 1, \
         updated_at = now() WHERE id = $1",
    )
    .bind(module.id)
    .bind(if keep { "keep" } else { "archive" })
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Brings an archived module back. Refused when another live module took
/// its key meanwhile.
pub async fn restore(connection: &mut PgConnection, module: &ModuleRow) -> Result<(), AppError> {
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM modules WHERE workspace_id = $1 AND module_key = $2 \
         AND archived_at IS NULL)",
    )
    .bind(module.workspace_id)
    .bind(&module.module_key)
    .fetch_one(&mut *connection)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            "module",
            "another module has this key now; archive it first",
        ));
    }
    sqlx::query(
        "UPDATE modules SET archived_at = NULL, archived_parts = NULL, revision = revision + 1, \
         updated_at = now() WHERE id = $1",
    )
    .bind(module.id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_take_the_local_prefix() {
        assert_eq!(full_key("playtest").as_deref(), Some("local.playtest"));
        assert_eq!(full_key("local.play_2").as_deref(), Some("local.play_2"));
        assert_eq!(full_key("Playtest"), None);
        assert_eq!(full_key("acme.build"), None);
        assert_eq!(full_key(""), None);
        assert_eq!(
            object_on_sql("collection", "collection.id"),
            "module_part_on('collection', collection.id)"
        );
    }
}
