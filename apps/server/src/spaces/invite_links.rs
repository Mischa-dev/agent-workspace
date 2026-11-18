//! Invitation links (K4.26, ruling R30): a link anyone holding it may use to
//! join a workspace with its role, and the space it names when it names
//! one, up to its number of uses, until it expires or is revoked.
//!
//! - `invite_link_create`, `invite_link_list`, `invite_link_revoke`
//!   (operations, so the web, agents and the v2 route share them): for
//!   someone who may invite people, with a role within what they hold
//!   (`access::may_grant`), never ownership.
//! - `POST /api/v1/invite-links/{resolve,accept-new,accept-existing}`: the
//!   same three steps as a workspace invitation (`/join#token=...` reads
//!   either). Accepting is K1's one way in: `accounts::create_person` for
//!   someone new, `engine::admit` for the workspace, and the space through
//!   `memberships::add_member` as the link's maker.
//!
//! The token is one of the invitation engine's families (`aw_lnk_`). It is
//! shown once, in the answer to the create that made it: only its digest
//! and a short prefix are stored, a retried create finds the link without
//! its token (the operation keeps its own request id rather than the
//! dispatcher's stored answer), and no audit row or log line carries it. An
//! unknown, used up, revoked or expired link, or one whose maker may no
//! longer give its role, gets the same answer.

use axum::{Json, Router, extract::State, http::HeaderMap, response::Response, routing::post};
use chrono::{DateTime, Duration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    AppState,
    access::{self, Need, Scope, Target as AccessTarget},
    auth::{
        AuthenticatedActor, SessionIdentity, establish_session_in_transaction,
        load_authenticated_actor, normalize_login_name, password, require_allowed_origin,
        require_authenticated_write, respond_with_established_session,
    },
    error::{ApiJson, AppError},
    invitations::engine::{self, Admission, Admitted, Family},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
    policy::Permission,
    spaces::SpaceKind,
};

/// How long a link lasts when the maker doesn't say, and at most.
const DEFAULT_DAYS: i64 = 7;
const MAX_DAYS: i64 = 30;
const MAX_USES: i32 = 10_000;
/// The answer for anything but a usable link.
const RESOURCE: &str = "invite_link";

pub fn register_ops(registry: &mut Registry) {
    registry.add::<InviteLinkCreate>();
    registry.add::<InviteLinkList>();
    registry.add::<InviteLinkRevoke>();
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/invite-links/resolve", post(resolve))
        .route("/api/v1/invite-links/accept-new", post(accept_new))
        .route(
            "/api/v1/invite-links/accept-existing",
            post(accept_existing),
        )
}

/// A link as its workspace sees it: never its token.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InviteLinkView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The space it also adds people to, if any.
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    #[schemars(with = "String")]
    pub role_id: Uuid,
    pub role_name: String,
    #[schemars(with = "String")]
    pub created_by: Uuid,
    /// How many people it admits; no limit when absent.
    pub max_uses: Option<i32>,
    pub uses: i32,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Whether someone could join with it now.
    pub active: bool,
}

const VIEW_SELECT: &str = r#"
    SELECT link.id, link.space_id, link.role_id, role.name AS role_name,
           link.created_by_actor_id AS created_by, link.max_uses, link.uses,
           link.created_at, link.expires_at, link.revoked_at,
           (link.revoked_at IS NULL AND link.expires_at > now()
            AND (link.max_uses IS NULL OR link.uses < link.max_uses)) AS active
    FROM invite_tokens link
    JOIN workspace_roles role ON role.id = link.role_id
"#;

async fn view(connection: &mut PgConnection, link_id: Uuid) -> Result<InviteLinkView, AppError> {
    sqlx::query_as::<_, InviteLinkView>(&format!("{VIEW_SELECT} WHERE link.id = $1"))
        .bind(link_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound(RESOURCE))
}

// ---------------------------------------------------------------------------
// invite_link_create

pub struct InviteLinkCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InviteLinkCreateInput {
    /// The role people join with. Default: the workspace's default role.
    /// It must be within what you hold, and never ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<IdRef>,
    /// A space they also join, as members. You must be able to manage its
    /// members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
    /// How many people it admits, 1 to 10000. Default: no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i32>,
    /// Days until it stops working, 1 to 30. Default: 7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<i64>,
}

impl Targeted for InviteLinkCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteLinkCreated {
    pub link: InviteLinkView,
    /// The token, shown only in the answer that made the link; absent when
    /// a retry found a link already made. Lost tokens can't be shown again:
    /// revoke the link and make another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The address to share, carrying the token; absent with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Operation for InviteLinkCreate {
    const NAME: &'static str = "invite_link_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    // The answer carries the token, which must not be stored for a replay:
    // the link keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    // Recorded in the audit row: nobody else's view changes.
    const EMITS: &'static [&'static str] = &["invite_link.created"];
    const TITLE: &'static str = "Make an invitation link";
    const DESCRIPTION: &'static str = "Make a link anyone holding it can use to join this \
        workspace with a role within yours (and a space, if you name one), up to a number of \
        people, until it expires. Returns the link and its token, shown only this once; share \
        the url. Revoke it with invite_link_revoke.";

    type Input = InviteLinkCreateInput;
    type Output = InviteLinkCreated;

    async fn run(
        call: &mut OpCall<'_>,
        input: InviteLinkCreateInput,
    ) -> Result<InviteLinkCreated, OpError> {
        if input
            .max_uses
            .is_some_and(|uses| !(1..=MAX_USES).contains(&uses))
        {
            return Err(OpError::invalid(
                "maxUses",
                "Give a number of people from 1 to 10000, or leave it out for no limit.",
            ));
        }
        let days = input.expires_in_days.unwrap_or(DEFAULT_DAYS);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(OpError::invalid(
                "expiresInDays",
                "Give a number of days from 1 to 30.",
            ));
        }
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let request_id = call.ctx.request_id.clone();
        let state = call.state;
        let tx = call.tx().await?;
        if let Some(request_id) = request_id.as_deref() {
            let made = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM invite_tokens \
                 WHERE created_by_actor_id = $1 AND request_id = $2 AND workspace_id = $3",
            )
            .bind(caller.account_id)
            .bind(request_id)
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if let Some(made) = made {
                let link = view(tx, made).await?;
                call.mark_replayed(true);
                call.audit_target("invite_link", made, json!({ "replayed": true }));
                return Ok(InviteLinkCreated {
                    link,
                    token: None,
                    url: None,
                });
            }
        }
        let role_id = match input.role_id.map(IdRef::id) {
            Some(role_id) => role_id,
            None => default_role(tx, workspace_id).await?,
        };
        let (role, custom) = engine::workspace_role(tx, workspace_id, Some(role_id))
            .await?
            .ok_or_else(|| OpError::not_found("That role"))?;
        if engine::is_ownership(&role) {
            return Err(OpError::invalid(
                "roleId",
                "Ownership is handed over with Transfer ownership, not given by a link.",
            ));
        }
        if !workspace_role_usable(tx, role_id).await? {
            return Err(OpError::invalid(
                "roleId",
                "That role is for spaces, not the workspace. Pick a workspace role.",
            ));
        }
        if !engine::issuer_may_grant(tx, &caller, Scope::Workspace(workspace_id), &role, custom)
            .await?
        {
            return Err(OpError::invalid(
                "roleId",
                "You can only give a role within what you hold yourself.",
            ));
        }
        let space_id = match input.space.map(IdRef::id) {
            None => None,
            Some(space_id) => Some(link_space(tx, &caller, workspace_id, space_id).await?),
        };
        let issued = engine::issue(Family::Link);
        let link_id = Uuid::now_v7();
        let expires_at = Utc::now() + Duration::days(days);
        sqlx::query(
            r#"
            INSERT INTO invite_tokens (
                id, workspace_id, space_id, role_id, token_prefix, token_digest,
                created_by_actor_id, request_id, max_uses, expires_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(link_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(role_id)
        .bind(&issued.prefix)
        .bind(issued.digest.as_slice())
        .bind(caller.account_id)
        .bind(request_id.as_deref())
        .bind(input.max_uses)
        .bind(expires_at)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let link = view(tx, link_id).await?;
        call.audit_target(
            "invite_link",
            link_id,
            json!({
                "roleId": role_id,
                "spaceId": space_id,
                "maxUses": input.max_uses,
                "expiresInDays": days,
            }),
        );
        let url = crate::invitations::join_url(state, &issued.token);
        Ok(InviteLinkCreated {
            link,
            token: Some(issued.token),
            url: Some(url),
        })
    }
}

/// The workspace's role for newcomers: the one marked default, else the
/// starting member role.
async fn default_role(connection: &mut PgConnection, workspace_id: Uuid) -> Result<Uuid, OpError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workspace_roles WHERE organization_id = $1 \
         ORDER BY is_default DESC, (built_in AND key = 'member') DESC, created_at LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That role"))
}

/// Whether a role can be someone's role in the workspace: the starting
/// viewer and worker roles are for spaces only.
async fn workspace_role_usable(
    connection: &mut PgConnection,
    role_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT NOT (built_in AND key IN ('viewer', 'worker')) FROM workspace_roles WHERE id = $1",
    )
    .bind(role_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// The space a link also adds people to: in this workspace, one people
/// join (not the workspace's own, which everyone is in), and one whose
/// members the caller manages.
async fn link_space(
    connection: &mut PgConnection,
    caller: &access::Caller,
    workspace_id: Uuid,
    space_id: Uuid,
) -> Result<Uuid, OpError> {
    let space = super::access::get(connection, caller.account_id, space_id)
        .await?
        .filter(|space| space.workspace_id == workspace_id)
        .ok_or_else(|| OpError::not_found("That space"))?;
    if space.kind != SpaceKind::Project && space.kind != SpaceKind::Team {
        return Err(OpError::invalid(
            "space",
            "Name a project or team space; everyone who joins is in the workspace's own space.",
        ));
    }
    if space.archived_at.is_some() {
        return Err(OpError::invalid(
            "space",
            "That space is archived. Restore it first.",
        ));
    }
    match access::check(
        connection,
        caller,
        Need::permission(Permission::MembersManage).writes(),
        AccessTarget::Space(space_id),
    )
    .await?
    {
        Ok(_) => Ok(space_id),
        Err(access::Denied::NotFound) => Err(OpError::not_found("That space")),
        Err(_) => Err(OpError::invalid(
            "space",
            "You can't add members to that space. Leave it out, or ask someone who manages it.",
        )),
    }
}

// ---------------------------------------------------------------------------
// invite_link_list

pub struct InviteLinkList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InviteLinkListInput {
    /// Include revoked, expired and used-up links. Default: only links that
    /// work now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_inactive: bool,
}

impl Targeted for InviteLinkListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteLinkListView {
    pub links: Vec<InviteLinkView>,
}

impl Operation for InviteLinkList {
    const NAME: &'static str = "invite_link_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List invitation links";
    const DESCRIPTION: &'static str = "This workspace's invitation links, newest first: their \
        role, space, uses and expiry. Tokens are never shown again after a link is made.";

    type Input = InviteLinkListInput;
    type Output = InviteLinkListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: InviteLinkListInput,
    ) -> Result<InviteLinkListView, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let links = sqlx::query_as::<_, InviteLinkView>(&format!(
            "{VIEW_SELECT} WHERE link.workspace_id = $1 \
               AND ($2 OR (link.revoked_at IS NULL AND link.expires_at > now() \
                    AND (link.max_uses IS NULL OR link.uses < link.max_uses))) \
             ORDER BY link.created_at DESC, link.id DESC LIMIT 200"
        ))
        .bind(workspace_id)
        .bind(input.include_inactive)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(InviteLinkListView { links })
    }
}

// ---------------------------------------------------------------------------
// invite_link_revoke

pub struct InviteLinkRevoke;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InviteLinkRevokeInput {
    /// The link: its id.
    pub link: IdRef,
}

impl Targeted for InviteLinkRevokeInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for InviteLinkRevoke {
    const NAME: &'static str = "invite_link_revoke";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    // Recorded in the audit row: nobody else's view changes.
    const EMITS: &'static [&'static str] = &["invite_link.revoked"];
    const TITLE: &'static str = "Revoke an invitation link";
    const DESCRIPTION: &'static str = "Stop an invitation link from letting anyone else in. \
        People who already joined with it stay. You can revoke links giving a role within \
        yours. Returns the link.";

    type Input = InviteLinkRevokeInput;
    type Output = InviteLinkView;

    async fn run(
        call: &mut OpCall<'_>,
        input: InviteLinkRevokeInput,
    ) -> Result<InviteLinkView, OpError> {
        let link_id = input.link.id();
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let role_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT role_id FROM invite_tokens WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(link_id)
        .bind(workspace_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That link"))?;
        let (role, custom) = engine::workspace_role(tx, workspace_id, Some(role_id))
            .await?
            .ok_or_else(|| OpError::not_found("That link"))?;
        if !engine::issuer_may_grant(tx, &caller, Scope::Workspace(workspace_id), &role, custom)
            .await?
        {
            return Err(OpError::invalid(
                "link",
                "That link gives a role above yours. Ask someone who holds it to revoke it.",
            ));
        }
        sqlx::query(
            "UPDATE invite_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(link_id)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target("invite_link", link_id, json!({ "roleId": role_id }));
        let tx = call.tx().await?;
        Ok(view(tx, link_id).await?)
    }
}

// ---------------------------------------------------------------------------
// Joining with a link

/// A link someone could join with now.
#[derive(Debug, sqlx::FromRow)]
struct UsableLink {
    id: Uuid,
    workspace_id: Uuid,
    workspace_name: String,
    space_id: Option<Uuid>,
    space_name: Option<String>,
    role_id: Uuid,
    token_digest: Vec<u8>,
    created_by: Uuid,
    expires_at: DateTime<Utc>,
}

/// The usable link a token names, locked when `lock`: unrevoked, unexpired,
/// not used up, its maker still an active member who may give its role. Any
/// other answer is the same `not_found`.
async fn find(
    connection: &mut PgConnection,
    prefix: &str,
    digest: &[u8; 32],
    lock: bool,
) -> Result<(UsableLink, String, Option<Uuid>), AppError> {
    let link = sqlx::query_as::<_, UsableLink>(&format!(
        r#"
        SELECT link.id, link.workspace_id, workspace.name AS workspace_name,
               link.space_id, space.name AS space_name, link.role_id, link.token_digest,
               link.created_by_actor_id AS created_by, link.expires_at
        FROM invite_tokens link
        JOIN organizations workspace ON workspace.id = link.workspace_id
        LEFT JOIN projects space ON space.id = link.space_id
        WHERE link.token_prefix = $1 AND link.revoked_at IS NULL AND link.expires_at > now()
          AND (link.max_uses IS NULL OR link.uses < link.max_uses)
        {}
        "#,
        if lock { "FOR UPDATE OF link" } else { "" }
    ))
    .bind(prefix)
    .fetch_optional(&mut *connection)
    .await?
    .filter(|link| engine::digest_matches(&link.token_digest, digest))
    .ok_or(AppError::NotFound(RESOURCE))?;
    let (role, custom) = engine::workspace_role(connection, link.workspace_id, Some(link.role_id))
        .await?
        .ok_or(AppError::NotFound(RESOURCE))?;
    // The maker's role may have narrowed, or they may have left.
    if engine::is_ownership(&role)
        || !crate::roles::issuer_still_grants(
            connection,
            link.workspace_id,
            link.created_by,
            &role,
            custom,
        )
        .await?
    {
        return Err(AppError::NotFound(RESOURCE));
    }
    Ok((link, role, custom))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    token: String,
}

/// What someone opening a link sees before joining. The same shape as a
/// workspace invitation's, with the space's name when it names one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedLink {
    organization_name: String,
    project_name: Option<String>,
    role: String,
    expires_at: DateTime<Utc>,
}

async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ResolveRequest>,
) -> Result<Json<ResolvedLink>, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        resolve_here(State(state), headers, ApiJson(request)),
    )
    .await
}

async fn resolve_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<ResolveRequest>,
) -> Result<Json<ResolvedLink>, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    require_allowed_origin(&state.config, &headers)?;
    let (prefix, digest) = engine::consume(&mut token, Family::Link, &state, RESOURCE).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = find(&mut transaction, &prefix, &digest, false).await;
    let ((link, role, _), transaction) = state
        .rate_limits
        .release_on_link_miss(transaction, found)
        .await?;
    transaction.commit().await?;
    Ok(Json(ResolvedLink {
        organization_name: link.workspace_name,
        project_name: link.space_name,
        role,
        expires_at: link.expires_at,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptNewRequest {
    token: String,
    login_name: String,
    display_name: String,
    handle: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptExistingRequest {
    token: String,
    handle: String,
}

/// Let `account_id` in with the link: the workspace with its role (or
/// nothing new there when they're already a member), the link's space as a
/// member, and one use counted. Someone the link adds nothing for is told
/// so and uses nothing.
async fn join(
    transaction: &mut Transaction<'_, Postgres>,
    link: &UsableLink,
    role: &str,
    custom: Option<Uuid>,
    account_id: Uuid,
    display_name: &str,
    handle: &str,
) -> Result<(), AppError> {
    let admitted = engine::admit(
        transaction,
        Admission {
            workspace_id: link.workspace_id,
            account_id,
            role,
            role_id: custom,
            display_name,
            handle,
        },
    )
    .await
    .map_err(|error| match error {
        AppError::Database(database) => crate::workspace::map_join_error(database),
        other => other,
    })?;
    if admitted != Admitted::AlreadyMember {
        engine::join_workspace_space(transaction, link.workspace_id, account_id).await?;
    }
    let mut added_to_space = false;
    if let Some(space_id) = link.space_id {
        // As the link's maker: they must still manage the space's members.
        let added = crate::memberships::add_member(
            transaction,
            link.created_by,
            space_id,
            account_id,
            "member",
            None,
        )
        .await
        .map_err(|error| match error {
            AppError::Forbidden | AppError::NotFound(_) | AppError::ProjectArchived => {
                AppError::NotFound(RESOURCE)
            }
            // Already in the space with another role.
            AppError::Conflict(..) => AppError::Conflict(RESOURCE, "already a member"),
            other => other,
        })?;
        added_to_space = added.inserted;
    }
    if admitted == Admitted::AlreadyMember && !added_to_space {
        return Err(AppError::Conflict(RESOURCE, "already a member"));
    }
    let counted = sqlx::query(
        "UPDATE invite_tokens SET uses = uses + 1 \
         WHERE id = $1 AND revoked_at IS NULL AND expires_at > now() \
           AND (max_uses IS NULL OR uses < max_uses)",
    )
    .bind(link.id)
    .execute(&mut **transaction)
    .await?;
    if counted.rows_affected() != 1 {
        return Err(AppError::NotFound(RESOURCE));
    }
    Ok(())
}

async fn accept_new(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<AcceptNewRequest>,
) -> Result<Response, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        accept_new_here(State(state), headers, ApiJson(request)),
    )
    .await
}

async fn accept_new_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<AcceptNewRequest>,
) -> Result<Response, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    let mut submitted_password = Zeroizing::new(std::mem::take(&mut request.password));
    require_allowed_origin(&state.config, &headers)?;
    if load_authenticated_actor(&state, &headers).await?.is_some() {
        return Err(AppError::Conflict(
            RESOURCE,
            "signed-in users must accept with their existing identity",
        ));
    }
    let (prefix, digest) = engine::consume(&mut token, Family::Link, &state, RESOURCE).await?;
    let login_name = normalize_login_name(&request.login_name)
        .map_err(|_| AppError::Validation("login name"))?;
    let display_name = crate::workspace::normalize_display_name(&request.display_name)?;
    let handle = crate::workspace::normalize_handle(&request.handle)?;
    if !(12..=1024).contains(&submitted_password.len()) {
        return Err(AppError::Validation("password"));
    }
    let _password_kdf_permit = state.rate_limits.try_acquire_password_kdf()?;
    let password_phc = password::hash_password(std::mem::take(&mut *submitted_password)).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = find(&mut transaction, &prefix, &digest, true).await;
    let ((link, role, custom), mut transaction) = state
        .rate_limits
        .release_on_link_miss(transaction, found)
        .await?;
    let crate::accounts::Person {
        account_id,
        user_id,
    } = crate::accounts::create_person(
        &mut transaction,
        crate::accounts::NewPerson {
            login_name: &login_name,
            display_name: &display_name,
            default_workspace: Some(link.workspace_id),
            created_by: Some(link.created_by),
            claimed: true,
        },
    )
    .await
    .map_err(crate::workspace::map_join_error)?;
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user_id)
        .bind(password_phc)
        .execute(&mut *transaction)
        .await?;
    join(
        &mut transaction,
        &link,
        &role,
        custom,
        account_id,
        &display_name,
        &handle,
    )
    .await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id,
            actor_id: account_id,
            actor_kind: "human".to_owned(),
            login_name,
            display_name,
            authentication_epoch: 1,
        },
    )
    .await?;
    crate::account_security::record_sign_in_method(
        &mut transaction,
        established.principal.session_id,
        "invitation",
    )
    .await?;
    acceptance_audit(
        &mut transaction,
        &established.principal,
        &link,
        "invitation",
        true,
    )
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

async fn accept_existing(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<AcceptExistingRequest>,
) -> Result<Response, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        accept_existing_here(State(state), headers, principal, ApiJson(request)),
    )
    .await
}

async fn accept_existing_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(mut request): ApiJson<AcceptExistingRequest>,
) -> Result<Response, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    require_authenticated_write(&state, &headers, &principal).await?;
    let (prefix, digest) = engine::consume(&mut token, Family::Link, &state, RESOURCE).await?;
    let handle = crate::workspace::normalize_handle(&request.handle)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = find(&mut transaction, &prefix, &digest, true).await;
    let ((link, role, custom), mut transaction) = state
        .rate_limits
        .release_on_link_miss(transaction, found)
        .await?;
    let epoch = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT user_account.authentication_epoch
        FROM users user_account
        JOIN actors actor ON actor.id = user_account.actor_id
        WHERE user_account.id = $1 AND user_account.status = 'active' AND actor.status = 'active'
        FOR UPDATE OF user_account
        "#,
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound(RESOURCE))?;
    join(
        &mut transaction,
        &link,
        &role,
        custom,
        principal.actor_id,
        &principal.display_name,
        &handle,
    )
    .await?;
    // Joining is not choosing (P0.37): the default workspace moves here
    // only for someone who had none and belongs nowhere else.
    sqlx::query(
        r#"
        UPDATE users SET default_organization_id = $2, updated_at = now()
        WHERE actor_id = $1 AND default_organization_id IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM organization_memberships other
              WHERE other.actor_id = users.actor_id AND other.enabled
                AND other.organization_id <> $2
          )
        "#,
    )
    .bind(principal.actor_id)
    .bind(link.workspace_id)
    .execute(&mut *transaction)
    .await?;
    // A fresh session, as after any invitation, so the new workspace is in
    // what the browser holds.
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id: principal.user_id,
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            login_name: principal.login_name.clone(),
            display_name: principal.display_name.clone(),
            authentication_epoch: epoch,
        },
    )
    .await?;
    crate::account_security::copy_session_details(
        &mut transaction,
        principal.session_id,
        established.principal.session_id,
    )
    .await?;
    acceptance_audit(
        &mut transaction,
        &established.principal,
        &link,
        "password_session",
        false,
    )
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

async fn acceptance_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    link: &UsableLink,
    authentication_class: &'static str,
    created_user: bool,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, organization_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, channel
        ) VALUES ($1, $2, $3, $4, $5, 'workspace.invite_link.accept',
                  'invite_link', $6, 'succeeded', $7, 'browser')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(link.workspace_id)
    .bind(principal.session_id)
    .bind(authentication_class)
    .bind(link.id)
    .bind(json!({
        "roleId": link.role_id,
        "spaceId": link.space_id,
        "createdBy": link.created_by,
        "createdUser": created_user,
    }))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
