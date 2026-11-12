//! AWD-111: a project's invitations through MCP, for people connected
//! through an app as themselves. Owners and admins of the workspace who
//! manage a project's invitations list them, send new join links, revoke
//! them and replace them, as the project's Team page does on the web. Agents
//! never invite anyone. Everything goes through `crate::invitations`, the
//! code the web app uses.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, project_role},
    writes::{WriteAccess, check_access_for},
};
use crate::{
    AppState,
    error::AppError,
    invitations::{self, InvitationView},
    policy::{Action, ActorKind},
};

const PEOPLE_INVITE: &str = "Only people invite others to a project: a workspace owner or admin who manages its invitations, on the web or through an app connected as themselves.";
const DEFAULT_EXPIRY_HOURS: i32 = 72;

/// An invitation, without its link: the link is shown once, when it's made.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct InvitationLine {
    pub id: String,
    /// The role it gives: admin, member or viewer, or a custom role (see
    /// roleId).
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    /// The start of the link's token, to tell links apart.
    pub token_prefix: String,
    /// active, expired, accepted, revoked or replaced.
    pub status: String,
    /// Pass as expectedVersion to revoke or replace it.
    pub version: i64,
    pub issued_by: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
    pub created_at: String,
    /// A workspace invitation's note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<InvitationView> for InvitationLine {
    fn from(view: InvitationView) -> Self {
        Self {
            id: view.id.to_string(),
            role: view.role,
            role_id: view.role_id.map(|id| id.to_string()),
            token_prefix: view.token_prefix,
            status: view.status,
            version: view.version,
            issued_by: view.issued_by_display_name,
            expires_at: view.expires_at,
            accepted_at: view.accepted_at,
            created_at: view.created_at,
            note: None,
        }
    }
}

impl From<crate::workspace::InvitationView> for InvitationLine {
    fn from(view: crate::workspace::InvitationView) -> Self {
        Self {
            id: view.id.to_string(),
            role: view.role_name.unwrap_or(view.role),
            role_id: view.role_id.map(|id| id.to_string()),
            token_prefix: view.token_prefix,
            status: view.status,
            version: view.version,
            issued_by: view.issued_by_display_name,
            expires_at: view.expires_at.to_rfc3339(),
            accepted_at: view.accepted_at.map(|at| at.to_rfc3339()),
            created_at: view.created_at.to_rfc3339(),
            note: view.note,
        }
    }
}

// ---------------------------------------------------------------------------
// invitation_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InvitationListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// List the invitations to the project's workspace instead.
    pub workspace: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct InvitationListValue {
    /// Newest first, at most 200.
    pub invitations: Vec<InvitationLine>,
}

pub(super) async fn invitation_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: InvitationListArguments,
) -> ToolResult<InvitationListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_INVITE);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    if arguments.workspace.unwrap_or(false) {
        let listed = async {
            let mut connection = state.pool.acquire().await?;
            let organization_id = organization_of(&mut connection, principal.project_id).await?;
            crate::workspace::workspace_invitations_for(
                &mut connection,
                principal.actor_id,
                organization_id,
            )
            .await
        }
        .await;
        return match listed {
            Ok(views) => ToolResult::ok(InvitationListValue {
                invitations: views.into_iter().map(Into::into).collect(),
            }),
            Err(error) => map_error(error, "invitation_list"),
        };
    }
    match invitations::invitations_for(state, principal.actor_id, principal.project_id).await {
        Ok(views) => ToolResult::ok(InvitationListValue {
            invitations: views.into_iter().map(Into::into).collect(),
        }),
        Err(error) => map_error(error, "invitation_list"),
    }
}

// ---------------------------------------------------------------------------
// invitation_write

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InvitationWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Invite to (or revoke an invitation to) the project's workspace
    /// instead: role admin or member, or a custom role. Workspace
    /// invitations can't be replaced; revoke and send a new one.
    pub workspace: Option<bool>,
    /// A note for a new workspace invitation, up to 200 characters.
    pub note: Option<String>,
    /// An invitation to revoke or replace, by ID (invitation_list). Leave it
    /// out to send a new one.
    pub invitation: Option<String>,
    /// For a new invitation: admin, member (the default), viewer, or a
    /// custom role's name (role_list). Never above what you hold; only
    /// owners invite admins.
    pub role: Option<String>,
    /// Revoke the invitation: its link stops working.
    pub revoke: Option<bool>,
    /// Replace the invitation with a new link for the same role; the old
    /// link stops working.
    pub replace: Option<bool>,
    /// How long a new or replacement link works: 1 to 168 hours. Defaults
    /// to 72.
    pub expires_in_hours: Option<i32>,
    /// The version you last read (invitation_list), to revoke or replace.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct InvitationWriteValue {
    /// "sent", "revoked" or "replaced".
    pub done: String,
    pub invitation: InvitationLine,
    /// The join link for a new or replacement invitation. It is shown only
    /// now: pass it on privately to the person you're inviting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_url: Option<String>,
}

pub(super) async fn invitation_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: InvitationWriteArguments,
) -> ToolResult<InvitationWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_INVITE);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let revoke = arguments.revoke.unwrap_or(false);
    let replace = arguments.replace.unwrap_or(false);
    let existing = match arguments.invitation.as_deref() {
        None => None,
        Some(text) => match parse_id(text) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("invitation", "Pass the invitation's ID."),
        },
    };
    match (existing, revoke, replace) {
        (None, false, false) => {
            if arguments.expected_version.is_some() {
                return ToolResult::invalid(
                    "expectedVersion",
                    "Leave out expectedVersion when sending a new invitation.",
                );
            }
        }
        (Some(_), true, false) | (Some(_), false, true) => {
            if arguments.role.is_some() {
                return ToolResult::invalid(
                    "role",
                    "An invitation keeps its role. Revoke it and send a new one instead.",
                );
            }
            if revoke && arguments.expires_in_hours.is_some() {
                return ToolResult::invalid(
                    "expiresInHours",
                    "Leave out expiresInHours when revoking.",
                );
            }
        }
        (None, _, _) => {
            return ToolResult::invalid("invitation", "Name the invitation to revoke or replace.");
        }
        (Some(_), true, true) => {
            return ToolResult::invalid("replace", "Either revoke or replace, not both.");
        }
        (Some(_), false, false) => {
            return ToolResult::invalid(
                "revoke",
                "Say what to do with the invitation: revoke: true or replace: true.",
            );
        }
    }
    let expires_in_hours = arguments.expires_in_hours.unwrap_or(DEFAULT_EXPIRY_HOURS);
    if arguments.workspace.unwrap_or(false) {
        if replace {
            return ToolResult::invalid(
                "replace",
                "Workspace invitations can't be replaced. Revoke it and send a new one.",
            );
        }
        let result =
            workspace_invitation_write(state, principal, &arguments, existing, expires_in_hours)
                .await;
        return result.unwrap_or_else(|error| map_error(error, "invitation_write"));
    }
    if arguments.note.is_some() {
        return ToolResult::invalid("note", "Only workspace invitations carry a note.");
    }
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        match check_access_for(&mut transaction, principal, Action::InvitationManage).await? {
            WriteAccess::Allowed => {}
            WriteAccess::ReadOnly => return Ok(ToolResult::read_only()),
            WriteAccess::NotFound => return Ok(ToolResult::not_found("That project")),
            WriteAccess::ProjectArchived => {
                return Ok(ToolResult::conflict("That project is archived."));
            }
            WriteAccess::Denied => return Ok(ToolResult::denied(PEOPLE_INVITE)),
        }
        let (done, invitation_id, token) = match existing {
            None => {
                let (role, role_id) = match arguments.role.as_deref().map(str::trim) {
                    None => ("member".to_owned(), None),
                    Some(role @ ("admin" | "member" | "viewer")) => (role.to_owned(), None),
                    Some(text) => {
                        let organization_id = sqlx::query_scalar::<_, Uuid>(
                            "SELECT organization_id FROM projects WHERE id = $1",
                        )
                        .bind(project_id)
                        .fetch_one(&mut *transaction)
                        .await?;
                        match crate::roles::find_role(&mut transaction, organization_id, text)
                            .await?
                        {
                            // The custom role's own base replaces this one.
                            Some(role_id) => ("member".to_owned(), Some(role_id)),
                            None => {
                                return Ok(ToolResult::invalid(
                                    "role",
                                    "Use admin, member, viewer, or a custom role's name from role_list.",
                                ));
                            }
                        }
                    }
                };
                let issued = invitations::issue_invitation_in(
                    &mut transaction,
                    principal.actor_id,
                    project_id,
                    &role,
                    role_id,
                    expires_in_hours,
                )
                .await?;
                insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    "project.invitation.issue",
                    "project_invitation",
                    issued.invitation_id,
                    issued.metadata,
                )
                .await?;
                ("sent", issued.invitation_id, Some(issued.token))
            }
            Some(invitation_id) => {
                // Only an invitation of the project this call names.
                let in_project = sqlx::query_scalar::<_, bool>(
                    "SELECT project_id = $2 FROM project_invitations WHERE id = $1",
                )
                .bind(invitation_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
                if !in_project {
                    return Ok(ToolResult::not_found("That invitation"));
                }
                let current = sqlx::query_scalar::<_, i64>(
                    "SELECT version FROM project_invitations WHERE id = $1",
                )
                .bind(invitation_id)
                .fetch_one(&mut *transaction)
                .await?;
                let expected_version = arguments.expected_version.unwrap_or(current);
                if revoke {
                    let revoked = invitations::revoke_invitation_in(
                        &mut transaction,
                        principal.actor_id,
                        invitation_id,
                        expected_version,
                    )
                    .await?;
                    if let Some(role) = revoked.newly_revoked_role {
                        insert_audit(
                            &mut transaction,
                            principal,
                            project_id,
                            "project.invitation.revoke",
                            "project_invitation",
                            invitation_id,
                            serde_json::json!({ "role": role }),
                        )
                        .await?;
                    }
                    ("revoked", invitation_id, None)
                } else {
                    let issued = invitations::replace_invitation_in(
                        &mut transaction,
                        principal.actor_id,
                        invitation_id,
                        expected_version,
                        expires_in_hours,
                    )
                    .await?;
                    insert_audit(
                        &mut transaction,
                        principal,
                        project_id,
                        "project.invitation.replace",
                        "project_invitation",
                        invitation_id,
                        issued.metadata,
                    )
                    .await?;
                    ("replaced", issued.invitation_id, Some(issued.token))
                }
            }
        };
        transaction.commit().await?;
        let view = invitations::fetch_invitation(state, project_id, invitation_id).await?;
        Ok::<_, AppError>(ToolResult::ok(InvitationWriteValue {
            done: done.to_owned(),
            invitation: view.into(),
            join_url: token.map(|token| invitations::join_url(state, &token)),
        }))
    }
    .await;
    result.unwrap_or_else(|error| map_error(error, "invitation_write"))
}

async fn organization_of(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound("project"))
}

/// Sends or revokes an invitation to the workspace of the project the call
/// names. The caller needs only to belong to that project; what they may
/// invite is their workspace role's business, as on the web.
async fn workspace_invitation_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: &InvitationWriteArguments,
    existing: Option<Uuid>,
    expires_in_hours: i32,
) -> Result<ToolResult<InvitationWriteValue>, AppError> {
    if project_role(state, principal, principal.project_id)
        .await?
        .is_none()
    {
        return Ok(ToolResult::not_found("That project"));
    }
    let project_id = principal.project_id;
    let mut transaction = state.pool.begin().await?;
    mark_via(&mut transaction, principal).await?;
    let organization_id = organization_of(&mut transaction, project_id).await?;
    let (done, invitation_id, token, action, metadata) = match existing {
        None => {
            let (role, role_id) = match arguments.role.as_deref().map(str::trim) {
                None => ("member".to_owned(), None),
                Some(role @ ("admin" | "member")) => (role.to_owned(), None),
                Some(text) => {
                    match crate::roles::find_role(&mut transaction, organization_id, text).await? {
                        // The custom role's own base replaces this one.
                        Some(role_id) => ("member".to_owned(), Some(role_id)),
                        None => {
                            return Ok(ToolResult::invalid(
                                "role",
                                "Use admin, member, or a custom role's name from role_list.",
                            ));
                        }
                    }
                }
            };
            let issued = crate::workspace::issue_workspace_invitation_in(
                &mut transaction,
                principal.actor_id,
                organization_id,
                &role,
                role_id,
                expires_in_hours,
                arguments.note.as_deref(),
            )
            .await?;
            (
                "sent",
                issued.invitation_id,
                Some(issued.token),
                "workspace.invitation.issue",
                issued.metadata,
            )
        }
        Some(invitation_id) => {
            if arguments.note.is_some() {
                return Ok(ToolResult::invalid("note", "Leave out note when revoking."));
            }
            // Only an invitation to this project's workspace.
            let Some(current) = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM workspace_invitations WHERE id = $1 AND organization_id = $2",
            )
            .bind(invitation_id)
            .bind(organization_id)
            .fetch_optional(&mut *transaction)
            .await?
            else {
                return Ok(ToolResult::not_found("That invitation"));
            };
            let (_, role) = crate::workspace::revoke_workspace_invitation_in(
                &mut transaction,
                principal.actor_id,
                invitation_id,
                arguments.expected_version.unwrap_or(current),
            )
            .await?;
            (
                "revoked",
                invitation_id,
                None,
                "workspace.invitation.revoke",
                serde_json::json!({ "role": role }),
            )
        }
    };
    let mut metadata = metadata;
    metadata["organizationId"] = serde_json::json!(organization_id);
    insert_audit(
        &mut transaction,
        principal,
        project_id,
        action,
        "workspace_invitation",
        invitation_id,
        metadata,
    )
    .await?;
    let view = crate::workspace::fetch_invitation(&mut transaction, invitation_id).await?;
    transaction.commit().await?;
    Ok(ToolResult::ok(InvitationWriteValue {
        done: done.to_owned(),
        invitation: view.into(),
        join_url: token.map(|token| invitations::join_url(state, &token)),
    }))
}

fn map_error<T: Serialize>(error: AppError, operation: &'static str) -> ToolResult<T> {
    match error {
        AppError::Forbidden => ToolResult::denied(PEOPLE_INVITE),
        AppError::NotFound("project" | "workspace") => ToolResult::not_found("That project"),
        AppError::NotFound(_) => ToolResult::not_found("That invitation"),
        AppError::Validation("invitation expiry") => {
            ToolResult::invalid("expiresInHours", "Use 1 to 168 hours (a week at most).")
        }
        AppError::VersionConflict {
            current_version, ..
        } => ToolResult::conflict(format!(
            "The invitation changed since you read it; it's at version {current_version} now. Read it again and retry."
        )),
        error => ToolResult::from_error(error, operation),
    }
}
