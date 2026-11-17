//! What an input acts on, and where that lives. Each input names its
//! target; dispatch locates it (workspace and space) before checking
//! access. A thing the caller can't see is `not_found`, the same as a
//! missing one. This replaces choosing a project by argument name.
//!
//! Until K3's `things::locate` lands, `locate` is a table here with one SQL
//! per kind; the conversation lookup goes through `conversation_visible_to`.

use sqlx::PgConnection;
use uuid::Uuid;

use super::OpError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The caller's own things (notifications, Later).
    Account,
    /// The workspace the call names (its `workspace` argument or path) when
    /// `None`.
    Workspace(Option<Uuid>),
    Space(Uuid),
    Thing {
        kind: &'static str,
        id: Uuid,
        expected_version: Option<i64>,
    },
}

impl Target {
    pub fn thing(kind: &'static str, id: Uuid) -> Self {
        Self::Thing {
            kind,
            id,
            expected_version: None,
        }
    }

    /// The id a request id is scoped to: the thing or space acted on.
    pub fn scope_id(&self, located: &Located) -> Uuid {
        match self {
            Self::Thing { id, .. } => *id,
            Self::Space(id) => *id,
            Self::Workspace(_) | Self::Account => located.workspace_id,
        }
    }

    /// The audit row's target kind and id when the operation names none.
    pub fn audit(&self, located: &Located) -> (&'static str, Uuid) {
        match self {
            Self::Thing { kind, id, .. } => (kind, *id),
            Self::Space(id) => ("project", *id),
            Self::Workspace(_) | Self::Account => ("organization", located.workspace_id),
        }
    }
}

/// An input says what it acts on.
pub trait Targeted {
    fn target(&self) -> Target;
}

/// Where a target lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Located {
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
}

/// Kinds `locate` finds, with the query that returns `(workspace, space)`
/// for an id the caller (`$2`) may see.
const KINDS: &[(&str, &str)] = &[
    (
        "project",
        "SELECT organization_id, id FROM projects WHERE id = $1 AND archived_at IS NULL AND $2::uuid IS NOT NULL",
    ),
    (
        "conversation",
        "SELECT project.organization_id, project.id FROM conversations conversation \
         JOIN projects project ON project.id = conversation.project_id \
         WHERE conversation.id = $1 AND conversation_visible_to(conversation.id, $2) \
           AND EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = project.id AND membership.actor_id = $2 \
               AND membership.enabled)",
    ),
    (
        "message",
        "SELECT project.organization_id, project.id FROM messages message \
         JOIN projects project ON project.id = message.project_id \
         WHERE message.id = $1 AND message.removed_at IS NULL \
           AND conversation_visible_to(message.conversation_id, $2) \
           AND EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = project.id AND membership.actor_id = $2 \
               AND membership.enabled)",
    ),
    // K2.7: a message found after it was removed too (its history, a
    // removal retried); still only in a conversation the caller may see.
    (
        "message_record",
        "SELECT project.organization_id, project.id FROM messages message \
         JOIN projects project ON project.id = message.project_id \
         WHERE message.id = $1 AND conversation_visible_to(message.conversation_id, $2) \
           AND EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = project.id AND membership.actor_id = $2 \
               AND membership.enabled)",
    ),
    // K2.7: a scheduled message is found only by its author.
    (
        "scheduled_message",
        "SELECT organization_id, project_id FROM scheduled_messages \
         WHERE id = $1 AND author_actor_id = $2",
    ),
    (
        "decision",
        "SELECT project.organization_id, project.id FROM project_decisions decision \
         JOIN projects project ON project.id = decision.project_id \
         WHERE decision.id = $1 AND (decision.source_conversation_id IS NULL \
           OR conversation_visible_to(decision.source_conversation_id, $2))",
    ),
    (
        "task",
        "SELECT project.organization_id, project.id FROM task_rows item \
         JOIN projects project ON project.id = item.project_id \
         WHERE item.id = $1 AND $2::uuid IS NOT NULL",
    ),
    // Any thing by id alone (K3.19), from the registry: a conversation or
    // anything that follows one (a message, a decision from it, a file in
    // it) only when the viewer may see that conversation. Those are checked
    // against their own row's conversation, not the registry's
    // `access_parent_id`, which is empty until the conversation itself is
    // registered (a backfill still running). The operation then checks the
    // thing through its kind's own resolver. A workspace board has no space.
    (
        "thing",
        "SELECT thing.workspace_id, thing.space_id FROM things thing \
         WHERE thing.id = $1 AND thing.deleted_at IS NULL AND $2::uuid IS NOT NULL \
           AND (thing.kind NOT IN ('channel', 'thread', 'conversation') \
                OR conversation_visible_to(thing.id, $2)) \
           AND (thing.kind <> 'message' OR EXISTS ( \
                SELECT 1 FROM messages message WHERE message.id = thing.id \
                  AND conversation_visible_to(message.conversation_id, $2))) \
           AND (thing.kind <> 'decision' OR EXISTS ( \
                SELECT 1 FROM project_decisions decision WHERE decision.id = thing.id \
                  AND (decision.source_conversation_id IS NULL \
                       OR conversation_visible_to(decision.source_conversation_id, $2)))) \
           AND (thing.kind <> 'file' OR EXISTS ( \
                SELECT 1 FROM files file WHERE file.id = thing.id \
                  AND (file.conversation_id IS NULL \
                       OR conversation_visible_to(file.conversation_id, $2)))) \
           AND (thing.access_parent_id IS NULL \
                OR NOT EXISTS (SELECT 1 FROM conversations parent \
                               WHERE parent.id = thing.access_parent_id) \
                OR conversation_visible_to(thing.access_parent_id, $2))",
    ),
    // A1.3: the run service keeps its SQL (guard G1).
    ("run", crate::runs::store::LOCATE_SQL),
    // M1.6: a built module, in its space or (space NULL) the workspace.
    // Archived ones are found too, so they can be restored.
    (
        "module",
        "SELECT workspace_id, space_id FROM modules WHERE id = $1 AND $2::uuid IS NOT NULL",
    ),
    // K2.12: a whiteboard, in its space or (space NULL) the workspace; one
    // in the trash too, so it can be restored. Its own rules (a space
    // board for the space's members) are checked again by `access`.
    (
        "whiteboard",
        "SELECT organization_id, project_id FROM whiteboards WHERE id = $1 AND $2::uuid IS NOT NULL",
    ),
    // M1.9: a package install, in its space or the workspace.
    (
        "package_install",
        "SELECT workspace_id, space_id FROM package_installs WHERE id = $1 AND $2::uuid IS NOT NULL",
    ),
];

/// The kinds `locate` knows; a test checks every operation's thing kind is
/// one of them.
pub fn known_kinds() -> impl Iterator<Item = &'static str> {
    KINDS.iter().map(|(kind, _)| *kind)
}

/// Where `target` lives, as `viewer` may see it. `named` is the workspace
/// the call named (path, argument), which a target elsewhere contradicts.
pub async fn locate(
    connection: &mut PgConnection,
    target: &Target,
    viewer: Uuid,
    named: Option<Uuid>,
) -> Result<Located, OpError> {
    let located = match target {
        Target::Account | Target::Workspace(None) => Located {
            workspace_id: named.ok_or(OpError::Invalid {
                field: "workspace".into(),
                message: "Say which workspace (workspace: its id or slug).".into(),
            })?,
            space_id: None,
        },
        Target::Workspace(Some(id)) => Located {
            workspace_id: *id,
            space_id: None,
        },
        Target::Space(id) => thing(connection, "project", *id, viewer).await?,
        Target::Thing { kind, id, .. } => thing(connection, kind, *id, viewer).await?,
    };
    // A target in another workspace than the one named is not found here.
    if named.is_some_and(|named| named != located.workspace_id) {
        return Err(OpError::not_found(not_found_what(target)));
    }
    Ok(located)
}

/// The workspace a path or `workspace` argument names: its id, or its slug.
/// A name that matches nothing is `not_found`, the same as one the caller
/// can't reach (access checks that later).
pub async fn workspace_named(connection: &mut PgConnection, name: &str) -> Result<Uuid, OpError> {
    let name = name.trim();
    if let Ok(id) = Uuid::parse_str(name) {
        return Ok(id);
    }
    if name.is_empty() || name.len() > 100 {
        return Err(OpError::not_found("That workspace"));
    }
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM organizations WHERE slug = $1")
        .bind(name.to_ascii_lowercase())
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?
        .ok_or(OpError::not_found("That workspace"))
}

async fn thing(
    connection: &mut PgConnection,
    kind: &str,
    id: Uuid,
    viewer: Uuid,
) -> Result<Located, OpError> {
    let Some((_, sql)) = KINDS.iter().find(|(known, _)| *known == kind) else {
        return Err(OpError::Unavailable);
    };
    // The space is NULL only for a thing that lives in the workspace itself
    // (a workspace-wide built module, or a registry thing such as a
    // workspace board).
    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(sql)
        .bind(id)
        .bind(viewer)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?;
    let (workspace_id, space_id) = row.ok_or(OpError::not_found(what_of(kind)))?;
    Ok(Located {
        workspace_id,
        space_id,
    })
}

fn not_found_what(target: &Target) -> &'static str {
    match target {
        Target::Thing { kind, .. } => what_of(kind),
        Target::Space(_) => "That space",
        Target::Workspace(_) | Target::Account => "That workspace",
    }
}

fn what_of(kind: &str) -> &'static str {
    match kind {
        "project" => "That space",
        "conversation" => "That conversation",
        "message" | "message_record" => "That message",
        "scheduled_message" => "That scheduled message",
        "decision" => "That decision",
        "task" => "That task",
        "run" => "That run",
        "module" => "That module",
        "package_install" => "That install",
        "whiteboard" => "That whiteboard",
        _ => "That",
    }
}
