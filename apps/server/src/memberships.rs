use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    idempotency::{IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve},
    outbox::{OutboxEventPayload, ProjectMembershipUpdatedPayload, enqueue_typed},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/members",
            get(list_members).post(add_existing_member),
        )
        .route(
            "/api/v1/projects/{project_id}/member-candidates",
            get(member_candidates),
        )
        .route(
            "/api/v1/projects/{project_id}/members/{actor_id}",
            axum::routing::patch(update_member),
        )
}

#[derive(Debug, FromRow)]
struct AccessRow {
    project_archived: bool,
    organization_role: String,
    organization_enabled: bool,
    project_role: String,
    project_enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct MemberRow {
    id: Uuid,
    project_id: Uuid,
    organization_id: Uuid,
    actor_id: Uuid,
    organization_role: String,
    actor_kind: String,
    role: String,
    display_name: String,
    handle: String,
    enabled: bool,
    version: i64,
    invited_by_actor_id: Option<Uuid>,
    invited_by_display_name: Option<String>,
    accepted_invitation_id: Option<Uuid>,
    accepted_at: Option<String>,
    disabled_by_actor_id: Option<Uuid>,
    disabled_at: Option<String>,
    created_at: String,
    profile_readiness: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateMemberRequest {
    expected_version: i64,
    role: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MembersResponse {
    members: Vec<MemberView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    role: String,
    display_name: String,
    handle: String,
    enabled: bool,
    version: i64,
    invited_by: Option<MemberProvenance>,
    accepted_invitation_id: Option<Uuid>,
    accepted_at: Option<String>,
    disabled_by_actor_id: Option<Uuid>,
    disabled_at: Option<String>,
    created_at: String,
    execution_profile_readiness: String,
    capabilities: MemberCapabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberProvenance {
    actor_id: Uuid,
    display_name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberCapabilities {
    change_role: bool,
    disable: bool,
    enable: bool,
}

async fn list_members(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<MembersResponse>, AppError> {
    let access = load_access(&state, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_access(&principal, &access, project_id, Action::MembershipView)?;
    let rows = sqlx::query_as::<_, MemberRow>(member_list_sql())
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(MembersResponse {
        members: rows
            .into_iter()
            .map(|row| member_view(row, &principal, &access))
            .collect(),
    }))
}

async fn update_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, actor_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateMemberRequest>,
) -> Result<Json<MemberView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    validate_update(&request)?;
    if actor_id == principal.actor_id {
        return Err(AppError::Forbidden);
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = format!(
        "project_membership.{}.{}.update",
        project_id.simple(),
        actor_id.simple()
    );

    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("m7-membership:{project_id}"))
        .execute(&mut *transaction)
        .await?;
    let access = load_access_for_update(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_access(&principal, &access, project_id, Action::MembershipManage)?;
    if access.project_archived {
        return Err(AppError::ProjectArchived);
    }

    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(Json(replay.decode()?));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    let target = load_member_for_update(&mut transaction, project_id, actor_id)
        .await?
        .ok_or(AppError::NotFound("project_member"))?;
    let next_role = request.role.as_deref().unwrap_or(&target.role);
    authorize_target(&access, &target, next_role)?;
    if target.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_membership",
            current_version: target.version,
        });
    }

    let next_enabled = request.enabled.unwrap_or(target.enabled);
    if target.role == "owner" && (next_role != "owner" || !next_enabled) {
        lock_and_require_other_owners(
            &mut transaction,
            target.organization_id,
            project_id,
            actor_id,
        )
        .await?;
    }

    if next_role == target.role && next_enabled == target.enabled {
        let view = member_view(target, &principal, &access);
        complete(
            &mut transaction,
            &reservation,
            axum::http::StatusCode::OK,
            &view,
        )
        .await?;
        transaction.commit().await?;
        return Ok(Json(view));
    }

    let updated = sqlx::query_as::<_, MemberRow>(
        r#"
        UPDATE project_memberships
        SET role = $3, enabled = $4,
            disabled_by_actor_id = CASE WHEN $4 THEN NULL ELSE $5 END,
            disabled_at = CASE WHEN $4 THEN NULL ELSE now() END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND actor_id = $2 AND version = $6
        RETURNING id, project_id,
          (SELECT organization_id FROM projects WHERE id = $1) AS organization_id,
          actor_id, (SELECT kind FROM actors WHERE id = $2) AS actor_kind,
          (SELECT role FROM organization_memberships
            WHERE organization_id = (SELECT organization_id FROM projects WHERE id = $1)
              AND actor_id = $2) AS organization_role,
          role, display_name, handle, enabled, version,
          invited_by_actor_id,
          (SELECT display_name FROM project_memberships inviter
            WHERE inviter.project_id = $1
              AND inviter.actor_id = project_memberships.invited_by_actor_id)
            AS invited_by_display_name,
          accepted_invitation_id, accepted_at::text,
          disabled_by_actor_id, disabled_at::text, created_at::text,
          CASE
            WHEN EXISTS (SELECT 1 FROM execution_profiles profile
              WHERE profile.project_id = $1 AND profile.owner_actor_id = $2
                AND profile.status = 'available') THEN 'ready'
            WHEN EXISTS (SELECT 1 FROM execution_profiles profile
              WHERE profile.project_id = $1 AND profile.owner_actor_id = $2
                AND profile.status <> 'disabled') THEN 'setup_required'
            WHEN EXISTS (SELECT 1 FROM execution_profiles profile
              WHERE profile.project_id = $1 AND profile.owner_actor_id = $2)
              THEN 'disabled'
            ELSE 'not_configured'
          END AS profile_readiness
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .bind(next_role)
    .bind(next_enabled)
    .bind(principal.actor_id)
    .bind(request.expected_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_membership_database_error)?
    .ok_or(AppError::VersionConflict {
        resource: "project_membership",
        current_version: target.version,
    })?;

    if target.enabled && !next_enabled {
        revoke_project_credentials(&mut transaction, project_id, actor_id).await?;
    }
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        actor_id,
        &target,
        &updated,
    )
    .await?;
    enqueue_typed(
        &mut transaction,
        project_id,
        OutboxEventPayload::ProjectMembershipUpdated(ProjectMembershipUpdatedPayload {
            project_id,
            actor_id,
            version: updated.version,
        }),
    )
    .await?;
    let view = member_view(updated, &principal, &access);
    complete(
        &mut transaction,
        &reservation,
        axum::http::StatusCode::OK,
        &view,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(view))
}

fn validate_update(request: &UpdateMemberRequest) -> Result<(), AppError> {
    if request.expected_version < 1 || (request.role.is_none() && request.enabled.is_none()) {
        return Err(AppError::Validation("project membership update"));
    }
    if request
        .role
        .as_deref()
        .is_some_and(|role| !matches!(role, "admin" | "member" | "viewer"))
    {
        return Err(AppError::Validation("project membership role"));
    }
    Ok(())
}

fn authorize_access(
    principal: &AuthenticatedActor,
    access: &AccessRow,
    project_id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    if !access.organization_enabled
        || !matches!(access.organization_role.as_str(), "owner" | "admin")
    {
        return Err(AppError::NotFound("project"));
    }
    let subject = Subject::new(
        principal.actor_id,
        ActorKind::Human,
        true,
        Some(Membership::project(
            project_id,
            parse_role(&access.project_role)?,
            access.project_enabled,
        )),
        None,
    );
    match authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(project_id)),
    ) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(AppError::NotFound("project")),
    }
}

fn authorize_target(
    access: &AccessRow,
    target: &MemberRow,
    next_role: &str,
) -> Result<(), AppError> {
    if target.actor_kind == "external_agent" && !matches!(next_role, "member" | "viewer") {
        return Err(AppError::Forbidden);
    }
    match access.organization_role.as_str() {
        "owner" => Ok(()),
        "admin"
            if target.organization_role == "member"
                && matches!(target.role.as_str(), "member" | "viewer")
                && matches!(next_role, "member" | "viewer") =>
        {
            Ok(())
        }
        _ => Err(AppError::Forbidden),
    }
}

fn member_view(row: MemberRow, principal: &AuthenticatedActor, access: &AccessRow) -> MemberView {
    let manageable = row.actor_id != principal.actor_id
        && (access.organization_role == "owner"
            || (access.organization_role == "admin"
                && row.organization_role == "member"
                && matches!(row.role.as_str(), "member" | "viewer")));
    MemberView {
        id: row.id,
        project_id: row.project_id,
        actor_id: row.actor_id,
        actor_kind: row.actor_kind,
        role: row.role,
        display_name: row.display_name,
        handle: row.handle,
        enabled: row.enabled,
        version: row.version,
        invited_by: row
            .invited_by_actor_id
            .zip(row.invited_by_display_name)
            .map(|(actor_id, display_name)| MemberProvenance {
                actor_id,
                display_name,
            }),
        accepted_invitation_id: row.accepted_invitation_id,
        accepted_at: row.accepted_at,
        disabled_by_actor_id: row.disabled_by_actor_id,
        disabled_at: row.disabled_at,
        created_at: row.created_at,
        execution_profile_readiness: row.profile_readiness,
        capabilities: MemberCapabilities {
            change_role: manageable,
            disable: manageable && row.enabled,
            enable: manageable && !row.enabled,
        },
    }
}

async fn load_access(
    state: &AppState,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<AccessRow>, AppError> {
    Ok(sqlx::query_as::<_, AccessRow>(access_sql(false))
        .bind(actor_id)
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?)
}

async fn load_access_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<AccessRow>, AppError> {
    Ok(sqlx::query_as::<_, AccessRow>(access_sql(true))
        .bind(actor_id)
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?)
}

fn access_sql(lock: bool) -> &'static str {
    if lock {
        r#"
        SELECT project.archived_at IS NOT NULL AS project_archived,
               organization_membership.role AS organization_role,
               organization_membership.enabled AS organization_enabled,
               project_membership.role AS project_role,
               project_membership.enabled AS project_enabled
        FROM projects project
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $1
        JOIN project_memberships project_membership
          ON project_membership.project_id = project.id
         AND project_membership.actor_id = $1
        WHERE project.id = $2
        FOR UPDATE OF project, organization_membership, project_membership
        "#
    } else {
        r#"
        SELECT project.archived_at IS NOT NULL AS project_archived,
               organization_membership.role AS organization_role,
               organization_membership.enabled AS organization_enabled,
               project_membership.role AS project_role,
               project_membership.enabled AS project_enabled
        FROM projects project
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $1
        JOIN project_memberships project_membership
          ON project_membership.project_id = project.id
         AND project_membership.actor_id = $1
        WHERE project.id = $2
        "#
    }
}

async fn load_member_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<MemberRow>, AppError> {
    Ok(sqlx::query_as::<_, MemberRow>(&format!(
        "{} FOR UPDATE OF membership, actor, organization_membership",
        member_query_sql()
    ))
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn member_list_sql() -> &'static str {
    concat!(
        "SELECT * FROM (",
        // The reusable projection is bounded to human Users and safe metadata.
        "SELECT membership.id, membership.project_id, project.organization_id, membership.actor_id, actor.kind AS actor_kind, ",
        "organization_membership.role AS organization_role, membership.role, membership.display_name, ",
        "membership.handle, membership.enabled, membership.version, membership.invited_by_actor_id, ",
        "inviter.display_name AS invited_by_display_name, membership.accepted_invitation_id, ",
        "membership.accepted_at::text, membership.disabled_by_actor_id, membership.disabled_at::text, ",
        "membership.created_at::text, CASE ",
        "WHEN EXISTS (SELECT 1 FROM execution_profiles profile WHERE profile.project_id = membership.project_id AND profile.owner_actor_id = membership.actor_id AND profile.status = 'available') THEN 'ready' ",
        "WHEN EXISTS (SELECT 1 FROM execution_profiles profile WHERE profile.project_id = membership.project_id AND profile.owner_actor_id = membership.actor_id AND profile.status <> 'disabled') THEN 'setup_required' ",
        "WHEN EXISTS (SELECT 1 FROM execution_profiles profile WHERE profile.project_id = membership.project_id AND profile.owner_actor_id = membership.actor_id) THEN 'disabled' ",
        "ELSE 'not_configured' END AS profile_readiness ",
        "FROM project_memberships membership JOIN projects project ON project.id = membership.project_id ",
        "JOIN actors actor ON actor.id = membership.actor_id AND actor.kind IN ('human', 'external_agent') ",
        "LEFT JOIN users user_account ON user_account.actor_id = actor.id ",
        "JOIN organization_memberships organization_membership ON organization_membership.organization_id = project.organization_id AND organization_membership.actor_id = actor.id ",
        "LEFT JOIN project_memberships inviter ON inviter.project_id = membership.project_id AND inviter.actor_id = membership.invited_by_actor_id ",
        "WHERE membership.project_id = $1",
        ") bounded_members ORDER BY enabled DESC, created_at, actor_id"
    )
}

fn member_query_sql() -> &'static str {
    r#"
    SELECT membership.id, membership.project_id, project.organization_id, membership.actor_id, actor.kind AS actor_kind,
           organization_membership.role AS organization_role, membership.role,
           membership.display_name, membership.handle, membership.enabled, membership.version,
           membership.invited_by_actor_id, inviter.display_name AS invited_by_display_name,
           membership.accepted_invitation_id, membership.accepted_at::text,
           membership.disabled_by_actor_id, membership.disabled_at::text,
           membership.created_at::text,
           CASE
             WHEN EXISTS (SELECT 1 FROM execution_profiles profile
               WHERE profile.project_id = membership.project_id
                 AND profile.owner_actor_id = membership.actor_id
                 AND profile.status = 'available') THEN 'ready'
             WHEN EXISTS (SELECT 1 FROM execution_profiles profile
               WHERE profile.project_id = membership.project_id
                 AND profile.owner_actor_id = membership.actor_id
                 AND profile.status <> 'disabled') THEN 'setup_required'
             WHEN EXISTS (SELECT 1 FROM execution_profiles profile
               WHERE profile.project_id = membership.project_id
                 AND profile.owner_actor_id = membership.actor_id) THEN 'disabled'
             ELSE 'not_configured'
           END AS profile_readiness
    FROM project_memberships membership
    JOIN projects project ON project.id = membership.project_id
    JOIN actors actor ON actor.id = membership.actor_id AND actor.kind IN ('human', 'external_agent')
    LEFT JOIN users user_account ON user_account.actor_id = actor.id
    JOIN organization_memberships organization_membership
      ON organization_membership.organization_id = project.organization_id
     AND organization_membership.actor_id = actor.id
    LEFT JOIN project_memberships inviter
      ON inviter.project_id = membership.project_id
     AND inviter.actor_id = membership.invited_by_actor_id
    WHERE membership.project_id = $1 AND membership.actor_id = $2
    "#
}

async fn lock_and_require_other_owners(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    let organization_owners = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' AND enabled FOR UPDATE",
    )
    .bind(organization_id)
    .fetch_all(&mut **transaction)
    .await?;
    let project_owners = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = $1 AND role = 'owner' AND enabled FOR UPDATE",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    if !organization_owners.iter().any(|owner| *owner != actor_id)
        || !project_owners.iter().any(|owner| *owner != actor_id)
    {
        return Err(AppError::Conflict(
            "project_membership",
            "the last enabled owner cannot be changed",
        ));
    }
    Ok(())
}

async fn revoke_project_credentials(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE mcp_client_credentials SET status = 'revoked', version = version + 1, revoked_at = now(), updated_at = now() WHERE project_id = $1 AND actor_id = $2 AND status = 'active'",
    )
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE mcp_oauth_grants SET revoked_at = now() WHERE project_id = $1 AND actor_id = $2 AND revoked_at IS NULL",
    )
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit_event(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    target_actor_id: Uuid,
    previous: &MemberRow,
    current: &MemberRow,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session',
            'project_membership.updated', 'actor', $5, 'succeeded', $6, $7
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(target_actor_id)
    .bind(request_id)
    .bind(serde_json::json!({
        "previous_role": previous.role,
        "role": current.role,
        "previous_enabled": previous.enabled,
        "enabled": current.enabled,
        "previous_version": previous.version,
        "version": current.version,
    }))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn parse_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("invalid membership role")),
    }
}

fn map_membership_database_error(error: sqlx::Error) -> AppError {
    if matches!(
        &error,
        sqlx::Error::Database(database)
            if database.constraint() == Some("project_memberships_active_handle_unique")
    ) {
        AppError::Conflict("project_membership", "handle already exists")
    } else {
        AppError::Database(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateMemberRequest, validate_update};

    #[test]
    fn membership_updates_are_bounded_and_cannot_grant_owner() {
        assert!(
            validate_update(&UpdateMemberRequest {
                expected_version: 1,
                role: Some("admin".to_owned()),
                enabled: None,
            })
            .is_ok()
        );
        for role in ["owner", "worker", "system", ""] {
            assert!(
                validate_update(&UpdateMemberRequest {
                    expected_version: 1,
                    role: Some(role.to_owned()),
                    enabled: None,
                })
                .is_err()
            );
        }
        assert!(
            validate_update(&UpdateMemberRequest {
                expected_version: 0,
                role: None,
                enabled: Some(false),
            })
            .is_err()
        );
        assert!(
            validate_update(&UpdateMemberRequest {
                expected_version: 1,
                role: None,
                enabled: None,
            })
            .is_err()
        );
    }
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct MemberCandidate {
    actor_id: Uuid,
    actor_kind: String,
    display_name: String,
    handle: String,
}

async fn member_candidates(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<Vec<MemberCandidate>>, AppError> {
    let access = load_access(&state, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_access(&principal, &access, project_id, Action::MembershipManage)?;
    Ok(Json(sqlx::query_as::<_, MemberCandidate>(r#"
        SELECT profile.actor_id, actor.kind AS actor_kind, profile.display_name, profile.handle
        FROM projects project
        JOIN organization_actor_profiles profile ON profile.organization_id = project.organization_id
        JOIN actors actor ON actor.id = profile.actor_id AND actor.status = 'active'
        JOIN organization_memberships om ON om.organization_id = profile.organization_id
          AND om.actor_id = profile.actor_id AND om.enabled
        LEFT JOIN users u ON u.actor_id = actor.id
        WHERE project.id = $1 AND actor.kind IN ('human', 'external_agent')
          AND (actor.kind = 'external_agent' OR u.status = 'active')
          AND NOT EXISTS (SELECT 1 FROM project_memberships pm WHERE pm.project_id = $1 AND pm.actor_id = actor.id)
        ORDER BY profile.display_name, actor.id LIMIT 200
    "#).bind(project_id).fetch_all(&state.pool).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddExistingMemberRequest {
    actor_id: Uuid,
    role: String,
}

async fn add_existing_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<AddExistingMemberRequest>,
) -> Result<Json<MemberView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if !matches!(request.role.as_str(), "member" | "viewer") {
        return Err(AppError::Validation("membership role"));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("m7-membership:{project_id}"))
        .execute(&mut *tx)
        .await?;
    let access = load_access_for_update(&mut tx, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_access(&principal, &access, project_id, Action::MembershipManage)?;
    if access.project_archived {
        return Err(AppError::ProjectArchived);
    }
    let candidate = sqlx::query_as::<_, (Uuid, String, String)>(r#"
        SELECT p.organization_id, profile.display_name, profile.handle
        FROM projects p
        JOIN organization_actor_profiles profile ON profile.organization_id = p.organization_id AND profile.actor_id = $2
        JOIN organization_memberships om ON om.organization_id = p.organization_id AND om.actor_id = $2 AND om.enabled
        JOIN actors a ON a.id = $2 AND a.status = 'active' AND a.kind IN ('human', 'external_agent')
        WHERE p.id = $1 AND (a.kind = 'external_agent' OR EXISTS (SELECT 1 FROM users u WHERE u.actor_id = a.id AND u.status = 'active'))
        FOR SHARE OF om, a
    "#).bind(project_id).bind(request.actor_id).fetch_optional(&mut *tx).await?
        .ok_or(AppError::NotFound("workspace member"))?;
    let inserted = sqlx::query(r#"
        INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle, invited_by_actor_id)
        VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (project_id, actor_id) DO NOTHING
    "#).bind(Uuid::now_v7()).bind(project_id).bind(request.actor_id).bind(&request.role)
        .bind(candidate.1).bind(candidate.2).bind(principal.actor_id).execute(&mut *tx).await?;
    let row = load_member_for_update(&mut tx, project_id, request.actor_id)
        .await?
        .ok_or(AppError::NotFound("member"))?;
    if !row.enabled || row.role != request.role {
        return Err(AppError::Conflict(
            "membership",
            "member already exists; use its current version to change access",
        ));
    }
    if inserted.rows_affected() > 0 {
        sqlx::query(r#"INSERT INTO audit_events (id,actor_id,project_id,session_id,authentication_class,action,target_kind,target_id,outcome)
            VALUES ($1,$2,$3,$4,'browser_session','project_membership.added','actor',$5,'succeeded')"#)
            .bind(Uuid::now_v7()).bind(principal.actor_id).bind(project_id).bind(principal.session_id)
            .bind(request.actor_id).execute(&mut *tx).await?;
        enqueue_typed(
            &mut tx,
            project_id,
            OutboxEventPayload::ProjectMembershipUpdated(ProjectMembershipUpdatedPayload {
                project_id,
                actor_id: request.actor_id,
                version: row.version,
            }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(member_view(row, &principal, &access)))
}
