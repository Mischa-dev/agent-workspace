//! Unclaimed accounts and claim links (K1.29). An admin can add a person
//! who hasn't signed in yet (someone imported, or added before they arrive):
//! the account exists, holds its role and can be mentioned and assigned, but
//! nobody can sign in as it. A one-time claim link, shown once to the admin
//! who made it, lets the person choose their sign-in name and password; the
//! account is then theirs, with everything already done for it.
//!
//! - `person_create` (an operation, so the web, agents and the v2 route share
//!   it): the account, its workspace membership and a claim link.
//! - `POST /api/v1/account-claims/accept`: the person claims it and is
//!   signed in.
//!
//! Only a claim's digest is stored. An unknown, used, revoked or expired
//! link gets the same answer.

use axum::{Router, extract::State, http::HeaderMap, response::Response, routing::post};
use chrono::{DateTime, Duration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    AppState,
    access::Scope,
    auth::{
        SessionIdentity, establish_session_in_transaction, load_authenticated_actor,
        normalize_login_name, password, require_allowed_origin, respond_with_established_session,
    },
    error::{ApiJson, AppError},
    invitations::engine::{self, Admission, Admitted, Family},
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// How long a claim link stays usable.
const CLAIM_DAYS: i64 = 14;

/// The workspace module's people operations.
pub fn register_ops(registry: &mut crate::ops::Registry) {
    registry.add::<PersonCreate>();
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/account-claims/accept", post(accept_claim))
}

/// Add a person who hasn't signed in yet, with a link to claim the account.
pub struct PersonCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonCreateInput {
    /// Their name here.
    pub display_name: String,
    /// Their handle here, for mentions.
    pub handle: String,
    /// Their role here. Default: the workspace's default role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<IdRef>,
}

impl Targeted for PersonCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonCreateView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// The claim token, shown only in the answer that added them; absent
    /// when a retry found them already added. The person opens
    /// `/claim#<token>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_token: Option<String>,
    pub claim_expires_at: DateTime<Utc>,
}

impl Operation for PersonCreate {
    const NAME: &'static str = "person_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    // The answer carries the claim token, which must not be stored for a
    // replay: the claim keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["project.membership.updated"];
    const TITLE: &'static str = "Add a person who hasn't signed in yet";
    const DESCRIPTION: &'static str = "Add someone before they sign in, with a role within \
        yours; they can be mentioned and assigned at once. Returns a claim token, shown once, \
        that lets them choose a sign-in name and password within 14 days.";

    type Input = PersonCreateInput;
    type Output = PersonCreateView;

    async fn run(
        call: &mut OpCall<'_>,
        input: PersonCreateInput,
    ) -> Result<PersonCreateView, OpError> {
        let display_name = input.display_name.trim().to_owned();
        if display_name.is_empty()
            || display_name.chars().count() > 120
            || display_name.chars().any(char::is_control)
        {
            return Err(OpError::invalid(
                "displayName",
                "Give a name of 1 to 120 characters on one line.",
            ));
        }
        let handle = crate::workspace::normalize_handle(&input.handle).map_err(|_| {
            OpError::invalid(
                "handle",
                "Use 2 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
            )
        })?;
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let request_id = call.ctx.request_id.clone();
        let tx = call.tx().await?;
        if let Some(request_id) = request_id.as_deref() {
            let made = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                "SELECT account_id, expires_at FROM account_claims \
                 WHERE workspace_id = $1 AND created_by_actor_id = $2 AND request_id = $3",
            )
            .bind(workspace_id)
            .bind(caller.account_id)
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if let Some((account_id, claim_expires_at)) = made {
                call.mark_replayed(true);
                call.audit_target("account", account_id, json!({ "replayed": true }));
                return Ok(PersonCreateView {
                    account_id,
                    claim_token: None,
                    claim_expires_at,
                });
            }
        }
        let (role, role_id) =
            engine::workspace_role(tx, workspace_id, input.role_id.map(IdRef::id))
                .await?
                .ok_or_else(|| OpError::not_found("That role"))?;
        if engine::is_ownership(&role) {
            return Err(OpError::invalid(
                "roleId",
                "Ownership is handed over with Transfer ownership, not given on arrival.",
            ));
        }
        if !engine::issuer_may_grant(tx, &caller, Scope::Workspace(workspace_id), &role, role_id)
            .await?
        {
            return Err(OpError::invalid(
                "roleId",
                "You can only give a role within what you hold yourself.",
            ));
        }
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
             WHERE organization_id = $1 AND lower(handle) = lower($2))",
        )
        .bind(workspace_id)
        .bind(&handle)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if taken {
            return Err(OpError::invalid(
                "handle",
                "Someone here already has that handle. Choose another.",
            ));
        }
        // A sign-in name nobody chose: replaced when the person claims it.
        let placeholder = format!("unclaimed-{}", Uuid::now_v7().simple());
        let person = crate::accounts::create_person(
            tx,
            crate::accounts::NewPerson {
                login_name: &placeholder,
                display_name: &display_name,
                default_workspace: Some(workspace_id),
                created_by: Some(caller.account_id),
                claimed: false,
            },
        )
        .await
        .map_err(AppError::from)?;
        let admitted = engine::admit(
            tx,
            Admission {
                workspace_id,
                account_id: person.account_id,
                role: &role,
                role_id,
                display_name: &display_name,
                handle: &handle,
            },
        )
        .await?;
        debug_assert_eq!(admitted, Admitted::Joined);
        engine::join_workspace_space(tx, workspace_id, person.account_id).await?;
        let issued = engine::issue(Family::Claim);
        let expires_at = Utc::now() + Duration::days(CLAIM_DAYS);
        sqlx::query(
            r#"
            INSERT INTO account_claims
                (id, workspace_id, account_id, token_prefix, token_digest,
                 created_by_actor_id, expires_at, request_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(person.account_id)
        .bind(&issued.prefix)
        .bind(issued.digest.as_slice())
        .bind(caller.account_id)
        .bind(expires_at)
        .bind(request_id.as_deref())
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target(
            "account",
            person.account_id,
            json!({ "role": role, "roleId": role_id, "unclaimed": true }),
        );
        Ok(PersonCreateView {
            account_id: person.account_id,
            claim_token: Some(issued.token),
            claim_expires_at: expires_at,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptClaimRequest {
    token: String,
    login_name: String,
    password: String,
}

#[derive(sqlx::FromRow)]
struct ClaimRow {
    id: Uuid,
    workspace_id: Uuid,
    account_id: Uuid,
    token_digest: Vec<u8>,
    user_id: Uuid,
    display_name: String,
}

/// The person claims their account: chooses a sign-in name and password,
/// and is signed in.
async fn accept_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<AcceptClaimRequest>,
) -> Result<Response, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    let mut submitted_password = Zeroizing::new(std::mem::take(&mut request.password));
    require_allowed_origin(&state.config, &headers)?;
    if load_authenticated_actor(&state, &headers).await?.is_some() {
        return Err(AppError::Conflict(
            "account_claim",
            "sign out before claiming another account",
        ));
    }
    let (prefix, digest) =
        engine::consume(&mut token, Family::Claim, &state, "account_claim").await?;
    let login_name = normalize_login_name(&request.login_name)
        .map_err(|_| AppError::Validation("login name"))?;
    if !(12..=1024).contains(&submitted_password.len()) {
        return Err(AppError::Validation("password"));
    }
    let _password_kdf_permit = state.rate_limits.try_acquire_password_kdf()?;
    let password_phc = password::hash_password(std::mem::take(&mut *submitted_password)).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let claim = sqlx::query_as::<_, ClaimRow>(
        r#"
        SELECT claim.id, claim.workspace_id, claim.account_id, claim.token_digest,
               user_account.id AS user_id, user_account.display_name
        FROM account_claims claim
        JOIN users user_account ON user_account.actor_id = claim.account_id
        JOIN actors account ON account.id = claim.account_id AND account.status = 'active'
        JOIN organization_memberships membership
          ON membership.organization_id = claim.workspace_id
         AND membership.actor_id = claim.account_id AND membership.enabled
        WHERE claim.token_prefix = $1
          AND claim.used_at IS NULL AND claim.revoked_at IS NULL AND claim.expires_at > now()
          AND user_account.claimed_at IS NULL AND user_account.status = 'active'
        FOR UPDATE OF claim, user_account
        "#,
    )
    .bind(&prefix)
    .fetch_optional(&mut *transaction)
    .await?
    .filter(|row| engine::digest_matches(&row.token_digest, &digest))
    .ok_or(AppError::NotFound("account_claim"))?;
    sqlx::query(
        "UPDATE users SET login_name = $2, claimed_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(claim.user_id)
    .bind(&login_name)
    .execute(&mut *transaction)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            AppError::Conflict("user", "that sign-in name is taken")
        }
        _ => AppError::from(error),
    })?;
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(claim.user_id)
        .bind(password_phc)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE account_claims SET used_at = now() WHERE id = $1")
        .bind(claim.id)
        .execute(&mut *transaction)
        .await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id: claim.user_id,
            actor_id: claim.account_id,
            actor_kind: "human".to_owned(),
            login_name,
            display_name: claim.display_name,
            authentication_epoch: 1,
        },
    )
    .await?;
    // A claim link is one of the invitation engine's links (K1.29).
    crate::account_security::record_sign_in_method(
        &mut transaction,
        established.principal.session_id,
        "invitation",
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, organization_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, channel
        ) VALUES ($1, $2, $3, $4, 'claim', 'account.claimed', 'account', $2, 'succeeded',
                  '{}'::jsonb, 'browser')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(claim.account_id)
    .bind(claim.workspace_id)
    .bind(established.principal.session_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

/// Whether `account_id` is still waiting to be claimed.
pub async fn unclaimed(
    connection: &mut sqlx::PgConnection,
    account_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM users WHERE actor_id = $1 AND claimed_at IS NULL)",
    )
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?)
}
