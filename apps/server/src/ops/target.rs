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
    /// The DM or group the target lives in, which is in no space (K4.17):
    /// access is decided by the conversation (`access::Target::Conversation`).
    pub direct: Option<Uuid>,
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
        "SELECT conversation.workspace_id, conversation.project_id FROM conversations conversation \
         WHERE conversation.id = $1 AND conversation_visible_to(conversation.id, $2) \
           AND (conversation.project_id IS NULL OR EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = conversation.project_id AND membership.actor_id = $2 \
               AND membership.enabled))",
    ),
    (
        "message",
        "SELECT message.workspace_id, message.project_id FROM messages message \
         WHERE message.id = $1 AND message.removed_at IS NULL \
           AND conversation_visible_to(message.conversation_id, $2) \
           AND (message.project_id IS NULL OR EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = message.project_id AND membership.actor_id = $2 \
               AND membership.enabled))",
    ),
    // K2.7: a message found after it was removed too (its history, a
    // removal retried); still only in a conversation the caller may see.
    (
        "message_record",
        "SELECT message.workspace_id, message.project_id FROM messages message \
         WHERE message.id = $1 AND conversation_visible_to(message.conversation_id, $2) \
           AND (message.project_id IS NULL OR EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = message.project_id AND membership.actor_id = $2 \
               AND membership.enabled))",
    ),
    // K2.21: a long call's task is found only by its caller.
    (
        "operation_task",
        "SELECT workspace_id, space_id FROM operation_tasks WHERE id = $1 AND account_id = $2",
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
    (
        "milestone",
        "SELECT project.organization_id, project.id FROM project_milestones milestone \
         JOIN projects project ON project.id = milestone.project_id \
         WHERE milestone.id = $1 AND $2::uuid IS NOT NULL",
    ),
    (
        "collection",
        "SELECT project.organization_id, project.id FROM collections collection \
         JOIN projects project ON project.id = collection.project_id \
         WHERE collection.id = $1 AND $2::uuid IS NOT NULL AND doc_open_to(collection.id, $2)",
    ),
    // AWD-223: a folder or document of a space's Docs tree, whatever its
    // kind, unless a restriction hides it from the caller.
    (
        "doc",
        "SELECT doc.workspace_id, doc.space_id FROM ( \
             SELECT id, workspace_id, space_id FROM doc_folders WHERE archived_at IS NULL \
             UNION ALL SELECT id, workspace_id, project_id FROM project_pages \
               WHERE memory_for_kind IS DISTINCT FROM 'conversation' \
             UNION ALL SELECT id, workspace_id, project_id FROM sheets \
             UNION ALL SELECT id, organization_id, project_id FROM whiteboards \
               WHERE deleted_at IS NULL \
             UNION ALL SELECT id, workspace_id, project_id FROM collections \
             UNION ALL SELECT id, workspace_id, space_id FROM dashboards \
               WHERE owner_actor_id IS NULL OR owner_actor_id = $2) doc \
         WHERE doc.id = $1 AND $2::uuid IS NOT NULL AND doc_open_to(doc.id, $2)",
    ),
    // A view: shared, or the caller's own (another's personal view is
    // not found).
    (
        "view",
        "SELECT project.organization_id, project.id FROM collection_views view \
         JOIN projects project ON project.id = view.project_id \
         WHERE view.id = $1 AND (view.owner_actor_id IS NULL OR view.owner_actor_id = $2) \
           AND doc_open_to(view.collection_id, $2)",
    ),
    // A dashboard: shared, or the caller's own (anyone else's is not found).
    (
        "dashboard",
        "SELECT dashboard.workspace_id, dashboard.space_id FROM dashboards dashboard \
         WHERE dashboard.id = $1 \
           AND (dashboard.owner_actor_id IS NULL OR dashboard.owner_actor_id = $2) \
           AND doc_open_to(dashboard.id, $2)",
    ),
    // Outside data (T1.34): a connection, and a saved query. Who sees which
    // is the operation's to say (data.connect, or a shared query).
    (
        "data_connection",
        "SELECT connection.workspace_id, connection.space_id FROM data_connections connection \
         WHERE connection.id = $1 AND $2::uuid IS NOT NULL",
    ),
    (
        "data_query",
        "SELECT query.workspace_id, query.space_id FROM data_queries query \
         WHERE query.id = $1 AND $2::uuid IS NOT NULL",
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
           AND doc_open_to(thing.id, $2) \
           AND (thing.kind <> 'record' OR thing.parent_id IS NULL \
                OR doc_open_to(thing.parent_id, $2)) \
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
    // K5.20: a direct upload, by its uploader alone, while they may still
    // see its conversation.
    (
        "file_upload",
        "SELECT upload.workspace_id, upload.space_id FROM file_uploads upload \
         WHERE upload.id = $1 AND upload.uploaded_by_actor_id = $2 \
           AND (upload.conversation_id IS NULL \
                OR conversation_visible_to(upload.conversation_id, $2))",
    ),
    // K4.7: a space its member may bring back, archived or not. Archived
    // spaces are closed to everything else, so only the workspace comes
    // back; `space_restore` checks the space as it was.
    (
        "member_space",
        "SELECT space.organization_id, NULL::uuid FROM projects space \
         WHERE space.id = $1 AND EXISTS ( \
             SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = space.id AND membership.actor_id = $2 \
               AND membership.enabled)",
    ),
    // K4.8: a space the caller may read, to join it: only its workspace
    // comes back, since they aren't in it yet.
    (
        "readable_space",
        "SELECT space.organization_id, NULL::uuid FROM projects space \
         WHERE space.id = $1 AND space_readable_by(space.id, $2)",
    ),
    // A1.3: the run service keeps its SQL (guard G1).
    ("run", crate::runs::store::LOCATE_SQL),
    // M1.15: a connected module's frame, only ever its viewer's own.
    (
        "frame_launch",
        "SELECT launch.workspace_id, launch.space_id FROM module_frame_launches launch \
         WHERE launch.id = $1 AND launch.account_id = $2",
    ),
    // K2.19: a queued launch, in its space; the command checks who may
    // cancel it.
    (
        "launch",
        "SELECT project.organization_id, project.id FROM launches launch \
         JOIN projects project ON project.id = launch.project_id \
         WHERE launch.id = $1 AND $2::uuid IS NOT NULL",
    ),
    // C1.3: things with a discussion, by the same rule their comments follow.
    // A whiteboard is the one below (K2.12).
    (
        "page",
        "SELECT project.organization_id, project.id FROM project_pages page \
         JOIN projects project ON project.id = page.project_id \
         WHERE page.id = $1 AND page_visible_to(page.id, $2)",
    ),
    (
        "change_set",
        "SELECT change.workspace_id, change.space_id FROM document_change_sets change \
         WHERE change.id = $1 \
           AND subject_visible_to(change.thing_kind, change.thing_id, $2)",
    ),
    (
        "proposal",
        "SELECT proposal.workspace_id, proposal.space_id FROM proposals proposal \
         WHERE proposal.id = $1 \
           AND subject_visible_to(proposal.thing_kind, proposal.thing_id, $2)",
    ),
    (
        "sheet",
        "SELECT project.organization_id, project.id FROM sheets sheet \
         JOIN projects project ON project.id = sheet.project_id \
         WHERE sheet.id = $1 AND subject_visible_to('sheet', sheet.id, $2)",
    ),
    // C1.16: a sheet's view, for whoever sees the sheet and, for a
    // personal one, only its owner.
    (
        "sheet_view",
        "SELECT view.workspace_id, view.space_id FROM sheet_views view \
         WHERE view.id = $1 AND subject_visible_to('sheet', view.sheet_id, $2) \
           AND (view.owner_actor_id IS NULL OR view.owner_actor_id = $2)",
    ),
    (
        "record_template",
        "SELECT template.workspace_id, template.space_id FROM record_templates template \
         WHERE template.id = $1 AND $2::uuid IS NOT NULL",
    ),
    (
        "recurring_record",
        "SELECT recurring.workspace_id, recurring.space_id FROM recurring_records recurring \
         WHERE recurring.id = $1 AND $2::uuid IS NOT NULL",
    ),
    (
        "form",
        "SELECT form.workspace_id, form.space_id FROM forms form \
         WHERE form.id = $1 AND $2::uuid IS NOT NULL",
    ),
    (
        "record",
        "SELECT project.organization_id, project.id FROM collection_records record \
         JOIN projects project ON project.id = record.project_id \
         WHERE record.id = $1 AND subject_visible_to('record', record.id, $2)",
    ),
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
    // K2.9: a reply request, for the agent it asks (claim, reply, pass,
    // release); another agent's is not found. One in a DM or group has no
    // space (K4.15) and is located at its conversation.
    (
        "reply_request",
        "SELECT COALESCE(project.organization_id, conversation.workspace_id), request.project_id \
         FROM reply_requests request \
         JOIN conversations conversation ON conversation.id = request.conversation_id \
         LEFT JOIN projects project ON project.id = request.project_id \
         WHERE request.id = $1 AND request.agent_actor_id = $2 \
           AND conversation_visible_to(request.conversation_id, $2)",
    ),
    // K2.9: a reply request for anyone who sees its conversation (stop it,
    // ask again).
    (
        "reply_request_seen",
        "SELECT COALESCE(project.organization_id, conversation.workspace_id), request.project_id \
         FROM reply_requests request \
         JOIN conversations conversation ON conversation.id = request.conversation_id \
         LEFT JOIN projects project ON project.id = request.project_id \
         WHERE request.id = $1 AND conversation_visible_to(request.conversation_id, $2) \
           AND (request.project_id IS NULL OR EXISTS (SELECT 1 FROM project_memberships membership \
             WHERE membership.project_id = project.id AND membership.actor_id = $2 \
               AND membership.enabled))",
    ),
    // K2.18: a repository, archived ones too, in its space; the command
    // checks who may see or manage it.
    (
        "repository",
        "SELECT project.organization_id, project.id FROM repositories repository \
         JOIN projects project ON project.id = repository.project_id \
         WHERE repository.id = $1 AND $2::uuid IS NOT NULL",
    ),
    // K2.18: a pull request goes by pull_request_visible_to (one opened
    // from a private thread stays private).
    (
        "pull_request",
        "SELECT project.organization_id, project.id FROM github_pull_requests pull \
         JOIN projects project ON project.id = pull.project_id \
         WHERE pull.id = $1 AND pull_request_visible_to(pull.id, $2)",
    ),
    // A2.12: a file, in its space, or in the DM or group it was sent in
    // (K4.15), which is found as the conversation is; who may read it is
    // `files::readable_file`.
    (
        "file",
        "SELECT COALESCE(conversation.workspace_id, file.organization_id), file.project_id \
         FROM files file \
         LEFT JOIN conversations conversation \
           ON conversation.id = file.conversation_id AND file.project_id IS NULL \
         WHERE file.id = $1 AND $2::uuid IS NOT NULL \
           AND (conversation.id IS NULL OR conversation_visible_to(conversation.id, $2))",
    ),
    // A1.8: a request goes by request_visible_to, which holds its
    // conversation's and run's visibility.
    (
        "request",
        // A request to let an account in (A2.18) or add an agent a host
        // made (A2.19) is about the workspace, though it is asked in the
        // workspace's own space.
        "SELECT request.workspace_id, \
                CASE WHEN request.subject_kind IN ('account_join', 'agent_register') THEN NULL \
                     ELSE request.space_id END \
         FROM requests request \
         WHERE request.id = $1 AND request_visible_to(request.id, $2)",
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
            direct: None,
        },
        Target::Workspace(Some(id)) => Located {
            workspace_id: *id,
            space_id: None,
            direct: None,
        },
        Target::Space(id) => thing(connection, "project", *id, viewer).await?,
        Target::Thing { kind, id, .. } => thing(connection, kind, *id, viewer).await?,
    };
    // A target in another workspace than the one named is not found here.
    // A DM or group is in no workspace (the direct scope), so it is found
    // from whichever workspace the caller is working in: the web app calls
    // every operation under its current workspace's address.
    if named.is_some_and(|named| named != located.workspace_id)
        && located.workspace_id != crate::spaces::DIRECT
    {
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
    // (a workspace-wide built module, a registry thing such as a workspace
    // board, or a workspace-level request).
    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(sql)
        .bind(id)
        .bind(viewer)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?;
    let (workspace_id, space_id) = row.ok_or(OpError::not_found(what_of(kind)))?;
    // A conversation or message with no space is in a DM or group.
    let direct = match (kind, space_id) {
        ("conversation", None) => Some(id),
        ("message" | "message_record", None) => sqlx::query_scalar::<_, Uuid>(
            "-- visibility: the message was just found through conversation_visible_to\n\
                 SELECT conversation_id FROM messages WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?,
        ("file", None) => sqlx::query_scalar::<_, Option<Uuid>>(
            "-- visibility: the file's conversation was just checked through conversation_visible_to\n\
                 SELECT conversation_id FROM files WHERE id = $1 AND project_id IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?
        .flatten(),
        ("file_upload", None) => sqlx::query_scalar::<_, Option<Uuid>>(
            "-- visibility: the upload's conversation was just checked through conversation_visible_to\n\
                 SELECT conversation_id FROM file_uploads WHERE id = $1 AND space_id IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?
        .flatten(),
        ("reply_request" | "reply_request_seen", None) => sqlx::query_scalar::<_, Uuid>(
            "-- visibility: the request was just found through its conversation\n\
                 SELECT conversation_id FROM reply_requests WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?,
        // A registry thing with no space: a DM's or group's conversation,
        // message or file is located at it.
        ("thing", None) => sqlx::query_scalar::<_, Option<Uuid>>(
            "-- visibility: the thing was just found through conversation_visible_to\n\
             SELECT COALESCE( \
                 (SELECT id FROM conversations WHERE id = $1 AND project_id IS NULL), \
                 (SELECT conversation_id FROM messages WHERE id = $1 AND project_id IS NULL), \
                 (SELECT conversation_id FROM files WHERE id = $1 AND project_id IS NULL))",
        )
        .bind(id)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| OpError::logged("locate", &error.into()))?,
        _ => None,
    };
    Ok(Located {
        workspace_id,
        space_id,
        direct,
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
        "project" | "member_space" | "readable_space" => "That space",
        "conversation" => "That conversation",
        "message" | "message_record" => "That message",
        "scheduled_message" => "That scheduled message",
        "decision" => "That decision",
        "task" => "That task",
        "milestone" => "That milestone",
        "run" => "That run",
        "page" => "That page",
        "proposal" => "That proposal",
        "change_set" => "That change",
        "sheet" => "That sheet",
        "record" => "That record",
        "form" => "That form",
        "record_template" => "That template",
        "recurring_record" => "That recurring record",
        "module" => "That module",
        "package_install" => "That install",
        "whiteboard" => "That whiteboard",
        "request" => "That request",
        "file_upload" => "That upload",
        "repository" => "That repository",
        "pull_request" => "That pull request",
        "file" => "That file",
        _ => "That",
    }
}
