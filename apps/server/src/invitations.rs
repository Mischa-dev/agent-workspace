use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
};

const INVITATION_TOKEN_BYTES: usize = 32;
const INVITATION_PREFIX_LENGTH: usize = 16;
const MAX_EXPIRY_HOURS: i32 = 7 * 24;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/invitations",
            get(list_invitations).post(issue_invitation),
        )
        .route(
            "/api/v1/project-invitations/{invitation_id}/revoke",
            post(revoke_invitation),
        )
        .route(
            "/api/v1/project-invitations/{invitation_id}/replace",
            post(replace_invitation),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IssueInvitationRequest {
    role: String,
    expires_in_hours: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevokeInvitationRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceInvitationRequest {
    expected_version: i64,
    expires_in_hours: i32,
}

#[derive(Debug, FromRow)]
struct InvitationAccess {
    organization_id: Uuid,
    project_archived: bool,
    organization_role: String,
    organization_membership_enabled: bool,
    project_membership_enabled: bool,
}

#[derive(Debug, FromRow)]
struct InvitationRow {
    id: Uuid,
    project_id: Uuid,
    role: String,
    token_prefix: String,
    status: String,
    version: i64,
    issued_by_actor_id: Uuid,
    issued_by_display_name: String,
    expires_at: String,
    accepted_by_actor_id: Option<Uuid>,
    accepted_at: Option<String>,
    revoked_at: Option<String>,
    created_at: String,
}

#[derive(Debug, FromRow)]
struct InvitationMutationRow {
    project_id: Uuid,
    role: String,
    status: String,
    version: i64,
    organization_id: Uuid,
    project_archived: bool,
    organization_role: String,
    organization_membership_enabled: bool,
    project_membership_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InvitationView {
    id: Uuid,
    project_id: Uuid,
    role: String,
    token_prefix: String,
    status: String,
    version: i64,
    issued_by_actor_id: Uuid,
    issued_by_display_name: String,
    expires_at: String,
    accepted_by_actor_id: Option<Uuid>,
    accepted_at: Option<String>,
    revoked_at: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedInvitation {
    #[serde(flatten)]
    invitation: InvitationView,
    invitation_url: String,
}

async fn list_invitations(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<Vec<InvitationView>>, AppError> {
    let access = invitation_access(&state, principal.actor_id, project_id).await?;
    authorize_manage(&access, None)?;
    let rows = sqlx::query_as::<_, InvitationRow>(
        r#"
        SELECT invitation.id, invitation.project_id, invitation.role,
               invitation.token_prefix,
               CASE WHEN invitation.status = 'active' AND invitation.expires_at <= now()
                    THEN 'expired' ELSE invitation.status END AS status,
               invitation.version, invitation.issued_by_actor_id,
               issuer.display_name AS issued_by_display_name,
               invitation.expires_at::text AS expires_at,
               invitation.accepted_by_actor_id,
               invitation.accepted_at::text AS accepted_at,
               invitation.revoked_at::text AS revoked_at,
               invitation.created_at::text AS created_at
        FROM project_invitations invitation
        JOIN actors issuer ON issuer.id = invitation.issued_by_actor_id
        WHERE invitation.project_id = $1
        ORDER BY invitation.created_at DESC, invitation.id DESC
        LIMIT 200
        "#,
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(InvitationView::from).collect()))
}

async fn issue_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<IssueInvitationRequest>,
) -> Result<(StatusCode, Json<IssuedInvitation>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    validate_invitation_role(&request.role)?;
    if !(1..=MAX_EXPIRY_HOURS).contains(&request.expires_in_hours) {
        return Err(AppError::Validation("invitation expiry"));
    }

    let (token, token_prefix, token_digest) = generate_invitation_token();
    let invitation_id = Uuid::now_v7();
    let mut transaction = state.pool.begin().await?;
    let access = lock_invitation_access(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_manage(&access, Some(&request.role))?;
    sqlx::query(
        r#"
        INSERT INTO project_invitations (
            id, organization_id, project_id, role, token_prefix, token_digest,
            issued_by_actor_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7,
                  now() + make_interval(hours => $8))
        "#,
    )
    .bind(invitation_id)
    .bind(access.organization_id)
    .bind(project_id)
    .bind(&request.role)
    .bind(&token_prefix)
    .bind(token_digest.to_vec())
    .bind(principal.actor_id)
    .bind(request.expires_in_hours)
    .execute(&mut *transaction)
    .await?;
    insert_audit(
        &mut transaction,
        &principal,
        project_id,
        invitation_id,
        "project.invitation.issue",
        serde_json::json!({
            "role": request.role,
            "expiresInHours": request.expires_in_hours,
        }),
    )
    .await?;
    transaction.commit().await?;

    let invitation = fetch_invitation(&state, project_id, invitation_id).await?;
    let origin = format!("http://{}", state.config.server_addr);
    Ok((
        StatusCode::CREATED,
        Json(IssuedInvitation {
            invitation,
            invitation_url: format!("{origin}/join#token={token}"),
        }),
    ))
}

async fn revoke_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(invitation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RevokeInvitationRequest>,
) -> Result<Json<InvitationView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("invitation version"));
    }
    let mut transaction = state.pool.begin().await?;
    let row = lock_invitation_for_mutation(&mut transaction, principal.actor_id, invitation_id)
        .await?
        .ok_or(AppError::NotFound("project_invitation"))?;
    let access = InvitationAccess {
        organization_id: row.organization_id,
        project_archived: row.project_archived,
        organization_role: row.organization_role,
        organization_membership_enabled: row.organization_membership_enabled,
        project_membership_enabled: row.project_membership_enabled,
    };
    authorize_manage(&access, Some(&row.role))?;
    if row.status == "revoked" {
        transaction.commit().await?;
        return Ok(Json(
            fetch_invitation(&state, row.project_id, invitation_id).await?,
        ));
    }
    if row.status != "active" {
        return Err(AppError::Conflict(
            "project_invitation",
            "invitation is no longer active",
        ));
    }
    if row.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_invitation",
            current_version: row.version,
        });
    }
    sqlx::query(
        r#"
        UPDATE project_invitations
        SET status = 'revoked', revoked_at = now(), version = version + 1,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(invitation_id)
    .execute(&mut *transaction)
    .await?;
    insert_audit(
        &mut transaction,
        &principal,
        row.project_id,
        invitation_id,
        "project.invitation.revoke",
        serde_json::json!({"role": row.role}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(
        fetch_invitation(&state, row.project_id, invitation_id).await?,
    ))
}

async fn replace_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(invitation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ReplaceInvitationRequest>,
) -> Result<(StatusCode, Json<IssuedInvitation>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("invitation version"));
    }
    if !(1..=MAX_EXPIRY_HOURS).contains(&request.expires_in_hours) {
        return Err(AppError::Validation("invitation expiry"));
    }

    let (token, token_prefix, token_digest) = generate_invitation_token();
    let replacement_id = Uuid::now_v7();
    let mut transaction = state.pool.begin().await?;
    let row = lock_invitation_for_mutation(&mut transaction, principal.actor_id, invitation_id)
        .await?
        .ok_or(AppError::NotFound("project_invitation"))?;
    let access = InvitationAccess {
        organization_id: row.organization_id,
        project_archived: row.project_archived,
        organization_role: row.organization_role,
        organization_membership_enabled: row.organization_membership_enabled,
        project_membership_enabled: row.project_membership_enabled,
    };
    authorize_manage(&access, Some(&row.role))?;
    if row.status != "active" {
        return Err(AppError::Conflict(
            "project_invitation",
            "invitation is no longer active",
        ));
    }
    if row.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_invitation",
            current_version: row.version,
        });
    }

    sqlx::query(
        r#"
        INSERT INTO project_invitations (
            id, organization_id, project_id, role, token_prefix, token_digest,
            issued_by_actor_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7,
                  now() + make_interval(hours => $8))
        "#,
    )
    .bind(replacement_id)
    .bind(row.organization_id)
    .bind(row.project_id)
    .bind(&row.role)
    .bind(&token_prefix)
    .bind(token_digest.to_vec())
    .bind(principal.actor_id)
    .bind(request.expires_in_hours)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE project_invitations
        SET status = 'replaced', revoked_at = now(),
            replaced_by_invitation_id = $2, version = version + 1,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(invitation_id)
    .bind(replacement_id)
    .execute(&mut *transaction)
    .await?;
    insert_audit(
        &mut transaction,
        &principal,
        row.project_id,
        invitation_id,
        "project.invitation.replace",
        serde_json::json!({
            "replacementInvitationId": replacement_id,
            "role": row.role,
            "expiresInHours": request.expires_in_hours,
        }),
    )
    .await?;
    transaction.commit().await?;

    let invitation = fetch_invitation(&state, row.project_id, replacement_id).await?;
    let origin = format!("http://{}", state.config.server_addr);
    Ok((
        StatusCode::CREATED,
        Json(IssuedInvitation {
            invitation,
            invitation_url: format!("{origin}/join#token={token}"),
        }),
    ))
}

async fn lock_invitation_for_mutation(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    invitation_id: Uuid,
) -> Result<Option<InvitationMutationRow>, AppError> {
    Ok(sqlx::query_as::<_, InvitationMutationRow>(
        r#"
        SELECT invitation.project_id, invitation.role, invitation.status,
               invitation.version, project.organization_id,
               project.archived_at IS NOT NULL AS project_archived,
               organization_membership.role AS organization_role,
               organization_membership.enabled AS organization_membership_enabled,
               project_membership.enabled AS project_membership_enabled
        FROM project_invitations invitation
        JOIN projects project ON project.id = invitation.project_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
        JOIN project_memberships project_membership
          ON project_membership.project_id = invitation.project_id
         AND project_membership.actor_id = $2
        WHERE invitation.id = $1
        FOR UPDATE OF invitation, project, organization_membership,
                      project_membership
        "#,
    )
    .bind(invitation_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn invitation_access(
    state: &AppState,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<InvitationAccess, AppError> {
    sqlx::query_as::<_, InvitationAccess>(
        r#"
        SELECT project.organization_id,
               project.archived_at IS NOT NULL AS project_archived,
               organization_membership.role AS organization_role,
               organization_membership.enabled AS organization_membership_enabled,
               project_membership.enabled AS project_membership_enabled
        FROM projects project
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
        JOIN project_memberships project_membership
          ON project_membership.project_id = project.id
         AND project_membership.actor_id = $2
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("project"))
}

async fn lock_invitation_access(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<InvitationAccess>, AppError> {
    Ok(sqlx::query_as::<_, InvitationAccess>(
        r#"
        SELECT project.organization_id,
               project.archived_at IS NOT NULL AS project_archived,
               organization_membership.role AS organization_role,
               organization_membership.enabled AS organization_membership_enabled,
               project_membership.enabled AS project_membership_enabled
        FROM projects project
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
        JOIN project_memberships project_membership
          ON project_membership.project_id = project.id
         AND project_membership.actor_id = $2
        WHERE project.id = $1
        FOR SHARE OF project, organization_membership, project_membership
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn authorize_manage(
    access: &InvitationAccess,
    invitation_role: Option<&str>,
) -> Result<(), AppError> {
    if access.project_archived
        || !access.organization_membership_enabled
        || !access.project_membership_enabled
    {
        return Err(AppError::NotFound("project"));
    }
    let manager = matches!(access.organization_role.as_str(), "owner" | "admin");
    let admin_invite_requires_owner =
        invitation_role == Some("admin") && access.organization_role != "owner";
    if !manager || admin_invite_requires_owner {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn validate_invitation_role(role: &str) -> Result<(), AppError> {
    matches!(role, "admin" | "member" | "viewer")
        .then_some(())
        .ok_or(AppError::Validation("invitation role"))
}

fn generate_invitation_token() -> (String, String, [u8; 32]) {
    let mut bytes = [0_u8; INVITATION_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("aw_inv_{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = token[..INVITATION_PREFIX_LENGTH].to_owned();
    let digest = Sha256::digest(token.as_bytes()).into();
    (token, prefix, digest)
}

async fn fetch_invitation(
    state: &AppState,
    project_id: Uuid,
    invitation_id: Uuid,
) -> Result<InvitationView, AppError> {
    let row = sqlx::query_as::<_, InvitationRow>(
        r#"
        SELECT invitation.id, invitation.project_id, invitation.role,
               invitation.token_prefix,
               CASE WHEN invitation.status = 'active' AND invitation.expires_at <= now()
                    THEN 'expired' ELSE invitation.status END AS status,
               invitation.version, invitation.issued_by_actor_id,
               issuer.display_name AS issued_by_display_name,
               invitation.expires_at::text AS expires_at,
               invitation.accepted_by_actor_id,
               invitation.accepted_at::text AS accepted_at,
               invitation.revoked_at::text AS revoked_at,
               invitation.created_at::text AS created_at
        FROM project_invitations invitation
        JOIN actors issuer ON issuer.id = invitation.issued_by_actor_id
        WHERE invitation.project_id = $1 AND invitation.id = $2
        "#,
    )
    .bind(project_id)
    .bind(invitation_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("project_invitation"))?;
    Ok(row.into())
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    invitation_id: Uuid,
    action: &'static str,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, channel
        ) VALUES ($1, $2, $3, $4, 'password_session', $5,
                  'project_invitation', $6, 'succeeded', $7, 'browser')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(invitation_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

impl From<InvitationRow> for InvitationView {
    fn from(row: InvitationRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            role: row.role,
            token_prefix: row.token_prefix,
            status: row.status,
            version: row.version,
            issued_by_actor_id: row.issued_by_actor_id,
            issued_by_display_name: row.issued_by_display_name,
            expires_at: row.expires_at,
            accepted_by_actor_id: row.accepted_by_actor_id,
            accepted_at: row.accepted_at,
            revoked_at: row.revoked_at,
            created_at: row.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        InvitationAccess, authorize_manage, generate_invitation_token, validate_invitation_role,
    };

    #[test]
    fn invitation_tokens_are_high_entropy_and_only_roles_are_allowed() {
        let (token, prefix, digest) = generate_invitation_token();
        assert!(token.starts_with("aw_inv_"));
        assert_eq!(token.len(), 50);
        assert_eq!(prefix.len(), 16);
        assert_eq!(digest.len(), 32);
        assert!(validate_invitation_role("admin").is_ok());
        assert!(validate_invitation_role("member").is_ok());
        assert!(validate_invitation_role("viewer").is_ok());
        assert!(validate_invitation_role("owner").is_err());
        assert!(validate_invitation_role("worker").is_err());
    }

    #[test]
    fn organization_role_owns_invitation_authority() {
        let mut access = InvitationAccess {
            organization_id: Uuid::now_v7(),
            project_archived: false,
            organization_role: "owner".to_owned(),
            organization_membership_enabled: true,
            project_membership_enabled: true,
        };
        assert!(authorize_manage(&access, Some("admin")).is_ok());

        access.organization_role = "admin".to_owned();
        assert!(authorize_manage(&access, Some("member")).is_ok());
        assert!(authorize_manage(&access, Some("admin")).is_err());

        access.organization_role = "member".to_owned();
        assert!(authorize_manage(&access, Some("viewer")).is_err());

        access.organization_role = "owner".to_owned();
        access.organization_membership_enabled = false;
        assert!(authorize_manage(&access, Some("viewer")).is_err());
        access.organization_membership_enabled = true;
        access.project_membership_enabled = false;
        assert!(authorize_manage(&access, Some("viewer")).is_err());
        access.project_membership_enabled = true;
        access.project_archived = true;
        assert!(authorize_manage(&access, Some("viewer")).is_err());
    }
}
