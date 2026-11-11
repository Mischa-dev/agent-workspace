use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    api::{insert_event_entry, insert_significant_event, retain_run_workspace},
    domain::{RunState, resume_answered_run, transition_run},
    error::AppError,
    external_events::enqueue_external_approval_resolved,
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    outbox,
    policy::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, Membership,
        MembershipRole, ProjectScope, Resource, RoleGrant, Subject, authorize,
    },
};

const ANY_AUTHORIZED_APPROVER: &str = "any_authorized";

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalGetArguments {
    pub approval_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRespondArguments {
    pub approval_id: String,
    pub expected_version: i64,
    pub decision: Option<String>,
    pub answer: Option<String>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalProjection {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub options: Vec<String>,
    pub status: String,
    pub version: i64,
    pub can_respond: bool,
    pub created_at: String,
    pub canonical_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ApprovalToolResult {
    Found {
        approval: ApprovalProjection,
    },
    Applied {
        approval: ApprovalProjection,
        idempotent_replay: bool,
    },
    Conflict {
        current: ApprovalProjection,
        /// Why the answer can't go through, when there's more to say than
        /// that the approval changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        retry_category: String,
    },
    NotFound {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    InvalidRequest {
        field: String,
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

impl ApprovalToolResult {
    pub fn denied() -> Self {
        Self::PermissionDenied {
            retry_category: "reauthenticate_or_request_scope".to_owned(),
        }
    }

    fn invalid(field: &str) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            retry_category: "correct_input".to_owned(),
        }
    }

    fn not_found() -> Self {
        Self::NotFound {
            retry_category: "do_not_retry".to_owned(),
        }
    }

    fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            retry_category: "retry_with_backoff".to_owned(),
        }
    }
}

#[derive(Debug, FromRow)]
struct ApprovalContext {
    id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    project_id: Uuid,
    repository_id: Uuid,
    run_state: String,
    requesting_actor_id: Uuid,
    eligible_actor_id: Option<Uuid>,
    eligible_role: String,
    human_required: bool,
    kind: String,
    summary: String,
    options: serde_json::Value,
    status: String,
    version: i64,
    created_at: String,
    membership_role: String,
    membership_role_permissions: Option<Vec<String>>,
    /// The agent's connection; `None` for a person acting through an app.
    connection_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ApprovalReference {
    approval_id: Uuid,
}

pub async fn get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ApprovalGetArguments,
) -> ApprovalToolResult {
    let Ok(approval_id) = arguments.approval_id.parse::<Uuid>() else {
        return ApprovalToolResult::invalid("approvalId");
    };
    match fetch_context(state, principal, approval_id, false).await {
        Ok(Some(context)) => match projection(&context, may_respond(principal)) {
            Ok(approval) => ApprovalToolResult::Found { approval },
            Err(error) => {
                tracing::error!(
                    category = error.log_category(),
                    "MCP approval projection failed"
                );
                ApprovalToolResult::unavailable()
            }
        },
        Ok(None) => ApprovalToolResult::not_found(),
        Err(error) => {
            tracing::error!(category = error.log_category(), "MCP approval_get failed");
            ApprovalToolResult::unavailable()
        }
    }
}

pub async fn respond(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: ApprovalRespondArguments,
) -> ApprovalToolResult {
    if !may_respond(principal) {
        return ApprovalToolResult::denied();
    }
    let Ok(approval_id) = arguments.approval_id.parse::<Uuid>() else {
        return ApprovalToolResult::invalid("approvalId");
    };
    if arguments.expected_version < 1 {
        return ApprovalToolResult::invalid("expectedVersion");
    }
    let key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(key) => key,
        Err(_) => return ApprovalToolResult::invalid("clientRequestId"),
    };
    arguments.answer = arguments.answer.map(|answer| answer.trim().to_owned());
    match respond_inner(state, principal, arguments, approval_id, key).await {
        Ok(result) => result,
        Err(AppError::NotFound(_)) => ApprovalToolResult::not_found(),
        Err(
            AppError::Forbidden | AppError::AuthenticationRequired | AppError::InvalidCredentials,
        ) => ApprovalToolResult::denied(),
        Err(AppError::Invalid(field)) => ApprovalToolResult::invalid(field),
        Err(AppError::RunNotAdmitted { reason }) => {
            // The answer was rolled back, so the approval is still pending.
            match fetch_projection(state, principal, approval_id).await {
                Ok(Some(current)) => ApprovalToolResult::Conflict {
                    current,
                    reason: Some(reason.to_owned()),
                    retry_category: "do_not_retry".to_owned(),
                },
                Ok(None) => ApprovalToolResult::not_found(),
                Err(error) => {
                    tracing::error!(
                        category = error.log_category(),
                        "MCP approval_respond refetch failed"
                    );
                    ApprovalToolResult::unavailable()
                }
            }
        }
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "MCP approval_respond failed"
            );
            ApprovalToolResult::unavailable()
        }
    }
}

async fn respond_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ApprovalRespondArguments,
    approval_id: Uuid,
    key: IdempotencyKey,
) -> Result<ApprovalToolResult, AppError> {
    let mut transaction = state.pool.begin().await?;
    let Some(context) = load_context(&mut transaction, principal, approval_id, true, true).await?
    else {
        return Ok(ApprovalToolResult::not_found());
    };
    if context.human_required {
        insert_human_required_denial_audit(&mut transaction, principal, &context).await?;
        transaction.commit().await?;
        return Ok(ApprovalToolResult::not_found());
    }
    require_policy(principal, &context)?;
    let (response, approved) = validate_response(&context.kind, &arguments)?;
    let request_hash = RequestHash::for_value(&arguments)?;
    let scope = resource_bound_scope("approval", approval_id, "external_respond")?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let reference = replay.decode::<ApprovalReference>()?;
            if reference.approval_id != approval_id {
                return Err(AppError::Internal("external approval replay mismatch"));
            }
            transaction.commit().await?;
            let approval = fetch_projection(state, principal, approval_id)
                .await?
                .ok_or(AppError::NotFound("agent_approval"))?;
            return Ok(ApprovalToolResult::Applied {
                approval,
                idempotent_replay: true,
            });
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    if context.version != arguments.expected_version || context.status != "pending" {
        return Ok(ApprovalToolResult::Conflict {
            current: projection(&context, may_respond(principal))?,
            reason: None,
            retry_category: "refetch_before_retry".to_owned(),
        });
    }
    if context.run_state != "blocked" {
        return Ok(ApprovalToolResult::Conflict {
            current: projection(&context, may_respond(principal))?,
            reason: None,
            retry_category: "do_not_retry".to_owned(),
        });
    }
    let updated = sqlx::query(
        r#"
        UPDATE agent_approvals
        SET status = 'resolved', responded_by_actor_id = $2,
            response = $3, responded_at = now()
        WHERE id = $1 AND status = 'pending' AND version = $4
        "#,
    )
    .bind(approval_id)
    .bind(principal.actor_id)
    .bind(&response)
    .bind(arguments.expected_version)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "agent_approval",
            "approval changed during response",
        ));
    }
    let event_kind = if context.kind == "question" {
        "question_resolved"
    } else {
        "approval_resolved"
    };
    let event_id = insert_significant_event(
        &mut transaction,
        context.run_id,
        event_kind,
        if context.kind == "question" {
            "The question was answered."
        } else {
            "The approval was resolved."
        },
        serde_json::json!({
            "approval_id": approval_id,
            "resolution": if context.kind == "question" { "answered" } else if approved { "approved" } else { "denied" },
            "responded_by_actor_id": principal.actor_id
        }),
    )
    .await?;
    insert_event_entry(&mut transaction, context.conversation_id, event_id).await?;
    if approved {
        resume_answered_run(&mut transaction, context.run_id).await?;
    } else {
        transition_run(
            &mut transaction,
            context.run_id,
            RunState::Interrupted,
            Some("The requested approval was denied."),
        )
        .await?;
        retain_run_workspace(&mut transaction, context.run_id).await?;
        crate::provider::idle_provider_session_for_terminal_run(&mut transaction, context.run_id)
            .await?;
        let interrupted_event = insert_significant_event(
            &mut transaction,
            context.run_id,
            "run_interrupted",
            "The run was interrupted after the approval was denied.",
            serde_json::json!({}),
        )
        .await?;
        insert_event_entry(&mut transaction, context.conversation_id, interrupted_event).await?;
    }
    enqueue_external_approval_resolved(&mut transaction, approval_id).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        context.project_id,
        context.conversation_id,
        context.run_id,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action,
            target_kind, target_id, outcome, channel, client_name,
            mcp_credential_id, external_agent_connection_id, metadata,
            mcp_oauth_grant_id
        ) VALUES (
            $1, $2, $3, $10, 'approval.external_responded',
            'agent_approval', $4, 'succeeded', 'mcp', $5,
            $6, $7, jsonb_build_object('resolution', $8, 'version', $9),
            $11
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(context.project_id)
    .bind(approval_id)
    .bind(&principal.client_name)
    .bind(principal.mcp_credential_id)
    .bind(context.connection_id)
    .bind(if context.kind == "question" {
        "answered"
    } else if approved {
        "approved"
    } else {
        "denied"
    })
    .bind(context.version + 1)
    .bind(principal.authentication_class)
    .bind(principal.oauth_grant_id)
    .execute(&mut *transaction)
    .await?;
    complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &ApprovalReference { approval_id },
    )
    .await?;
    transaction.commit().await?;
    let approval = fetch_projection(state, principal, approval_id)
        .await?
        .ok_or(AppError::NotFound("agent_approval"))?;
    Ok(ApprovalToolResult::Applied {
        approval,
        idempotent_replay: false,
    })
}

fn validate_response(
    kind: &str,
    arguments: &ApprovalRespondArguments,
) -> Result<(serde_json::Value, bool), AppError> {
    match kind {
        "approval" => {
            if arguments.answer.is_some()
                || !matches!(arguments.decision.as_deref(), Some("approve" | "deny"))
            {
                return Err(AppError::Invalid("approval decision"));
            }
            let decision = arguments
                .decision
                .as_deref()
                .ok_or(AppError::Invalid("approval decision"))?;
            Ok((
                serde_json::json!({"decision": decision}),
                decision == "approve",
            ))
        }
        "question" => {
            if arguments.decision.is_some() {
                return Err(AppError::Invalid("question answer"));
            }
            let answer = arguments
                .answer
                .as_deref()
                .filter(|answer| {
                    (1..=4_000).contains(&answer.chars().count())
                        && !answer.chars().any(|character| {
                            character.is_control() && character != '\n' && character != '\t'
                        })
                })
                .ok_or(AppError::Invalid("question answer"))?;
            Ok((serde_json::json!({"answer": answer}), true))
        }
        _ => Err(AppError::Internal("unknown approval kind")),
    }
}

async fn fetch_context(
    state: &AppState,
    principal: &McpPrincipal,
    approval_id: Uuid,
    lock: bool,
) -> Result<Option<ApprovalContext>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let context = load_context(&mut transaction, principal, approval_id, lock, false).await?;
    if let Some(context) = &context {
        require_policy(principal, context)?;
    }
    transaction.commit().await?;
    Ok(context)
}

async fn fetch_projection(
    state: &AppState,
    principal: &McpPrincipal,
    approval_id: Uuid,
) -> Result<Option<ApprovalProjection>, AppError> {
    fetch_context(state, principal, approval_id, false)
        .await?
        .map(|context| projection(&context, may_respond(principal)))
        .transpose()
}

async fn load_context(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    approval_id: Uuid,
    lock: bool,
    include_human_required_for_denial: bool,
) -> Result<Option<ApprovalContext>, AppError> {
    if principal.actor_kind == ActorKind::Human {
        return person_context(transaction, principal, approval_id, lock).await;
    }
    if !may_respond(principal) || !principal.can_read() {
        return Ok(None);
    }
    let locking = if lock {
        "FOR UPDATE OF approval, run FOR SHARE OF credential, connection, actor, project, membership, organization_membership"
    } else {
        "FOR SHARE OF credential, connection, actor, project, membership, organization_membership, approval, run"
    };
    let query = format!(
        r#"
        SELECT approval.id, approval.agent_run_id AS run_id,
               run.conversation_id, conversation.project_id,
               run.repository_id, run.state AS run_state,
               approval.requesting_actor_id, approval.eligible_actor_id,
               approval.eligible_role, approval.human_required,
               approval.kind, approval.summary, approval.options,
               approval.status, approval.version,
               approval.created_at::text AS created_at,
               membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions,
               connection.id AS connection_id
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
         AND connection.project_id = credential.project_id
         AND connection.actor_id = credential.actor_id
         AND connection.status = 'active'
         AND connection.scopes = credential.scopes
        JOIN actors actor ON actor.id = credential.actor_id
         AND actor.kind = 'external_agent' AND actor.status = 'active'
        JOIN projects project ON project.id = $5
         AND project.archived_at IS NULL
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = credential.actor_id
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = credential.actor_id
         AND organization_membership.enabled
        JOIN agent_approvals approval ON approval.id = $4
         AND approval.agent_run_id IS NOT NULL
         AND ($6 OR NOT approval.human_required)
         AND approval.requesting_actor_id <> credential.actor_id
         AND (
             approval.eligible_actor_id = credential.actor_id
             OR (
                 approval.eligible_actor_id IS NULL
                 AND approval.eligible_role IN (membership.role, 'any_authorized')
             )
         )
        JOIN agent_runs run ON run.id = approval.agent_run_id
        JOIN conversations conversation ON conversation.id = run.conversation_id
         AND conversation.project_id = project.id
        WHERE credential.id = $1
          AND conversation_visible_to(conversation.id, $3)
          AND connection.id = $2
          AND credential.actor_id = $3
          -- The credential stays anchored to its home project; the approval
          -- may be in any project this connection can act in.
          AND agent_connection_can_access(connection.id, project.id)
          AND credential.status = 'active'
          AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY['read','write','approval_respond']::text[]
        {locking}
        "#,
    );
    Ok(sqlx::query_as::<_, ApprovalContext>(&query)
        .bind(principal.credential_id)
        .bind(principal.external_agent_connection_id)
        .bind(principal.actor_id)
        .bind(approval_id)
        .bind(principal.project_id)
        .bind(include_human_required_for_denial)
        .fetch_optional(&mut **transaction)
        .await?)
}

fn require_policy(principal: &McpPrincipal, context: &ApprovalContext) -> Result<(), AppError> {
    let role = match RoleGrant::stored(
        &context.membership_role,
        context.membership_role_permissions.as_deref(),
    ) {
        Ok(role) if role.role() != MembershipRole::Worker => role,
        _ => return Err(AppError::NotFound("agent_approval")),
    };
    let eligibility = match context.eligible_actor_id {
        Some(actor_id) => ApprovalEligibility::Actor(actor_id),
        None if context.eligible_role == ANY_AUTHORIZED_APPROVER => {
            ApprovalEligibility::AnyAuthorizedMember
        }
        None => ApprovalEligibility::Role(parse_role(&context.eligible_role)?),
    };
    let subject = Subject::new(
        principal.actor_id,
        principal.actor_kind,
        true,
        Some(Membership::project(context.project_id, role, true)),
        None,
    );
    if matches!(
        authorize(
            &subject,
            Action::ApprovalRespond,
            Resource::Approval {
                scope: ProjectScope::approval(
                    context.project_id,
                    context.conversation_id,
                    context.run_id,
                    context.repository_id,
                    context.id,
                ),
                requirements: ApprovalRequirements {
                    requesting_actor_id: context.requesting_actor_id,
                    eligibility,
                    human_required: context.human_required,
                    separation_of_duty: true,
                },
            },
        ),
        Decision::Allow
    ) {
        Ok(())
    } else {
        Err(AppError::NotFound("agent_approval"))
    }
}

fn parse_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        _ => Err(AppError::NotFound("agent_approval")),
    }
}

fn projection(context: &ApprovalContext, may_respond: bool) -> Result<ApprovalProjection, AppError> {
    let summary = bounded_text(&context.summary, 500);
    let options = context
        .options
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .filter_map(|option| {
            option
                .as_str()
                .or_else(|| option.get("label").and_then(serde_json::Value::as_str))
                .or_else(|| option.get("id").and_then(serde_json::Value::as_str))
        })
        .filter(|option| !option.chars().any(char::is_control))
        .map(|option| bounded_text(option, 120))
        .collect();
    Ok(ApprovalProjection {
        id: context.id.to_string(),
        kind: context.kind.clone(),
        summary,
        options,
        status: context.status.clone(),
        version: context.version,
        can_respond: may_respond && context.status == "pending" && context.run_state == "blocked",
        created_at: context.created_at.clone(),
        canonical_path: format!(
            "/projects/{}/threads/{}",
            context.project_id, context.conversation_id
        ),
    })
}

fn bounded_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

async fn insert_human_required_denial_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    context: &ApprovalContext,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action,
            target_kind, target_id, outcome, channel, client_name,
            mcp_credential_id, external_agent_connection_id, metadata
        ) VALUES (
            $1, $2, $3, 'mcp_bearer', 'approval.external_response_rejected',
            'agent_approval', $4, 'denied', 'mcp', $5,
            $6, $7, jsonb_build_object('reason', 'human_required', 'version', $8)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(context.project_id)
    .bind(context.id)
    .bind(&principal.client_name)
    .bind(principal.mcp_credential_id)
    .bind(context.connection_id)
    .bind(context.version)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bounded_text;

    #[test]
    fn approval_projection_strips_controls_and_bounds_text() {
        assert_eq!(bounded_text("safe\u{1b}[31m", 8), "safe[31m");
        assert_eq!(bounded_text("abcdefgh", 4), "abcd");
    }
}

/// People with full access answer approvals as themselves; agents need an
/// active connection with the approval_respond scope.
fn may_respond(principal: &McpPrincipal) -> bool {
    match principal.actor_kind {
        ActorKind::Human => principal.can_write(),
        ActorKind::ExternalAgent => {
            principal.external_agent_connection_id.is_some()
                && principal.can_write()
                && principal.has_scope("approval_respond")
        }
        _ => false,
    }
}

/// The approval as a person sees it: in a project they belong to and can
/// reach, in a thread they can see, addressed to them or their role, and not
/// on a run they started. Approvals that need a person stay in the web app,
/// where the step Codex wants to take is shown in full.
async fn person_context(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    approval_id: Uuid,
    lock: bool,
) -> Result<Option<ApprovalContext>, AppError> {
    if !principal.can_read() {
        return Ok(None);
    }
    let locking = if lock {
        "FOR UPDATE OF approval, run FOR SHARE OF project, membership"
    } else {
        "FOR SHARE OF approval, run, project, membership"
    };
    let query = format!(
        r#"
        SELECT approval.id, approval.agent_run_id AS run_id,
               run.conversation_id, conversation.project_id,
               run.repository_id, run.state AS run_state,
               approval.requesting_actor_id, approval.eligible_actor_id,
               approval.eligible_role, approval.human_required,
               approval.kind, approval.summary, approval.options,
               approval.status, approval.version,
               approval.created_at::text AS created_at,
               membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions,
               NULL::uuid AS connection_id
        FROM agent_approvals approval
        JOIN agent_runs run ON run.id = approval.agent_run_id
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN projects project ON project.id = conversation.project_id
         AND project.archived_at IS NULL
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $1
         AND membership.enabled
        WHERE approval.id = $2
          AND NOT approval.human_required
          AND approval.requesting_actor_id <> $1
          AND (
              approval.eligible_actor_id = $1
              OR (
                  approval.eligible_actor_id IS NULL
                  AND approval.eligible_role IN (membership.role, 'any_authorized')
              )
          )
          AND conversation_visible_to(conversation.id, $1)
          AND actor_can_access_project($1, NULL, project.id)
        {locking}
        "#,
    );
    Ok(sqlx::query_as::<_, ApprovalContext>(&query)
        .bind(principal.actor_id)
        .bind(approval_id)
        .fetch_optional(&mut **transaction)
        .await?)
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalListArguments {
    /// Only this project (ID, link or key).
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListValue {
    /// Approvals Codex is waiting on that you can answer, oldest first. Answer
    /// one with approval_respond, passing its version as expectedVersion.
    pub approvals: Vec<ApprovalProjection>,
    /// Approvals that need a person in the web app, where the step is shown in
    /// full.
    pub in_web_app: i64,
}

/// Approvals waiting on the caller (people only; agents are told about theirs
/// through events).
pub async fn list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ApprovalListArguments,
) -> super::common::ToolResult<ApprovalListValue> {
    use super::common::{ToolResult, parse_id};
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(
            "Agents get their approvals through events; open one with approval_get.",
        );
    }
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let project = match arguments.project_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::not_found("That project"),
        },
        None => None,
    };
    let result = async {
        // AWD-75: not from projects where Runs is off for the caller.
        let off = super::runs::projects_with_module_off(state, principal.actor_id, "runs").await?;
        let rows = sqlx::query_as::<_, ApprovalContext>(
            r#"
            SELECT approval.id, approval.agent_run_id AS run_id,
                   run.conversation_id, conversation.project_id,
                   run.repository_id, run.state AS run_state,
                   approval.requesting_actor_id, approval.eligible_actor_id,
                   approval.eligible_role, approval.human_required,
                   approval.kind, approval.summary, approval.options,
                   approval.status, approval.version,
                   approval.created_at::text AS created_at,
                   membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions,
                   NULL::uuid AS connection_id
            FROM agent_approvals approval
            JOIN agent_runs run ON run.id = approval.agent_run_id
            JOIN conversations conversation ON conversation.id = run.conversation_id
            JOIN projects project ON project.id = conversation.project_id
             AND project.archived_at IS NULL
            JOIN project_memberships membership
              ON membership.project_id = project.id
             AND membership.actor_id = $1
             AND membership.enabled
            WHERE approval.status = 'pending'
              AND run.state = 'blocked'
              AND approval.requesting_actor_id <> $1
              AND (
                  approval.eligible_actor_id = $1
                  OR (
                      approval.eligible_actor_id IS NULL
                      AND approval.eligible_role IN (membership.role, 'any_authorized')
                  )
              )
              AND ($2::uuid IS NULL OR project.id = $2)
              AND NOT (project.id = ANY($3))
              AND conversation_visible_to(conversation.id, $1)
              AND actor_can_access_project($1, NULL, project.id)
            ORDER BY approval.created_at, approval.id
            LIMIT 50
            "#,
        )
        .bind(principal.actor_id)
        .bind(project)
        .bind(&off)
        .fetch_all(&state.pool)
        .await?;
        let in_web_app = rows.iter().filter(|row| row.human_required).count();
        let approvals = rows
            .iter()
            .filter(|row| !row.human_required)
            .filter(|row| require_policy(principal, row).is_ok())
            .map(|row| projection(row, may_respond(principal)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, AppError>(ToolResult::ok(ApprovalListValue {
            approvals,
            in_web_app: i64::try_from(in_web_app).unwrap_or(i64::MAX),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "approval_list"))
}
