//! K1.32: sharing one thing. A page can be shared with an account of its
//! workspace who isn't in its space, or by a link. Either way the share
//! opens that one thing to read and nothing else in the space: the space's
//! other things, lists and search still follow the space's members.
//!
//! A link is a secret shown once to whoever made it; only its digest is
//! kept. It opens the thing for any signed-in account that has it, or, when
//! it is public (C1.27), for anyone who has it, signed in or not, until it
//! expires or is revoked. Sharing needs the edit permission on the thing's
//! kind in its space, so nobody shares more than they could change.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use rand_core::{OsRng, RngCore};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub const LINK_PREFIX: &str = "aw_ln_";
const LINK_PREFIX_LENGTH: usize = 12;
const DEFAULT_LINK_HOURS: i64 = 7 * 24;
const MAX_LINK_HOURS: i64 = 90 * 24;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ThingShareList>()
        .add::<ThingShare>()
        .add::<ThingUnshare>()
        .add::<ThingLinkCreate>();
}

/// The kinds of thing that can be shared one by one.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThingKind {
    Page,
}

impl ThingKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Page => "page",
        }
    }
}

/// Whether `thing` of `kind` is shared with `account`, who is still an
/// active member of its workspace.
pub async fn shared_with(
    connection: &mut PgConnection,
    account: Uuid,
    kind: &str,
    thing: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM thing_grants grant_row
            JOIN organization_memberships membership
              ON membership.organization_id = grant_row.workspace_id
             AND membership.actor_id = grant_row.grantee_account_id
             AND membership.enabled
            JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
            WHERE grant_row.thing_kind = $1 AND grant_row.thing_id = $2
              AND grant_row.grantee_account_id = $3
              AND grant_row.revoked_at IS NULL
              AND (grant_row.expires_at IS NULL OR grant_row.expires_at > now())
        )
        "#,
    )
    .bind(kind)
    .bind(thing)
    .bind(account)
    .fetch_one(&mut *connection)
    .await?)
}

/// What a working link opens.
#[derive(Clone, Debug)]
pub struct Opened {
    pub kind: String,
    pub thing: Uuid,
    /// Anyone with the link may read it, signed in or not.
    pub public: bool,
}

/// The thing a link opens, while it works.
pub async fn by_link(pool: &PgPool, token: &str) -> Result<Option<Opened>, AppError> {
    if !token.starts_with(LINK_PREFIX) || token.len() > 128 {
        return Ok(None);
    }
    let Some(prefix) = token.get(..LINK_PREFIX_LENGTH) else {
        return Ok(None);
    };
    let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let rows = crate::db::scoped_one(pool, async |connection| {
        sqlx::query_as::<_, (String, Uuid, Vec<u8>, bool)>(
            "SELECT thing_kind, thing_id, link_digest, public FROM thing_grants \
             WHERE link_prefix = $1 AND revoked_at IS NULL \
               AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(prefix)
        .fetch_all(connection)
        .await
    })
    .await?;
    Ok(rows.into_iter().find_map(|(kind, thing, digest, public)| {
        (digest.len() == candidate.len() && bool::from(digest.as_slice().ct_eq(&candidate)))
            .then_some(Opened {
                kind,
                thing,
                public,
            })
    }))
}

/// Who and what a thing is shared with now.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThingSharesView {
    pub kind: ThingKind,
    #[schemars(with = "String")]
    pub id: Uuid,
    pub accounts: Vec<SharedAccount>,
    pub links: Vec<SharedLink>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedAccount {
    #[schemars(with = "String")]
    pub account: Uuid,
    /// Their name.
    pub display_name: String,
    #[schemars(with = "Option<String>")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedLink {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Anyone with it may read the page without signing in.
    pub public: bool,
    #[schemars(with = "Option<String>")]
    pub expires_at: Option<DateTime<Utc>>,
}

async fn shares(
    connection: &mut PgConnection,
    kind: ThingKind,
    id: Uuid,
) -> Result<ThingSharesView, AppError> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Option<Uuid>,
            Option<String>,
            bool,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT grant_row.id, grant_row.grantee_account_id, \
                CASE WHEN grant_row.grantee_account_id IS NOT NULL THEN \
                    (SELECT profile.display_name \
                     FROM account_profile(grant_row.grantee_account_id, grant_row.workspace_id) profile) \
                END, \
                grant_row.public, grant_row.expires_at \
         FROM thing_grants grant_row \
         WHERE grant_row.thing_kind = $1 AND grant_row.thing_id = $2 \
           AND grant_row.revoked_at IS NULL \
           AND (grant_row.expires_at IS NULL OR grant_row.expires_at > now()) \
         ORDER BY grant_row.created_at, grant_row.id",
    )
    .bind(kind.key())
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    let mut view = ThingSharesView {
        kind,
        id,
        accounts: Vec::new(),
        links: Vec::new(),
    };
    for (grant, account, name, public, expires_at) in rows {
        match account {
            Some(account) => view.accounts.push(SharedAccount {
                account,
                display_name: name.unwrap_or_default(),
                expires_at,
            }),
            None => view.links.push(SharedLink {
                id: grant,
                public,
                expires_at,
            }),
        }
    }
    Ok(view)
}

/// `thing.share_changed`, heard by the thing's space (ids only), in the
/// change's transaction.
async fn share_changed(call: &mut OpCall<'_>, kind: ThingKind, id: Uuid) -> Result<(), OpError> {
    let (workspace_id, space_id) = located(call)?;
    let by = call.ctx.caller.account_id;
    let event = crate::events::NewEvent::thing(
        &crate::events::catalog::THING_SHARE_CHANGED,
        space_id,
        kind.key(),
        id,
    )
    .workspace(workspace_id)
    .actor(by);
    let tx = call.tx().await?;
    crate::events::emit(tx, event).await?;
    Ok(())
}

fn located(call: &OpCall<'_>) -> Result<(Uuid, Uuid), OpError> {
    Ok((
        call.ctx.workspace_id,
        call.ctx.space_id.ok_or(OpError::Unavailable)?,
    ))
}

// ---------------------------------------------------------------------------
// Operations

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingShareListInput {
    /// What kind of thing: `page`.
    pub kind: ThingKind,
    /// The thing: its id or a link to it.
    pub id: IdRef,
}

impl Targeted for ThingShareListInput {
    fn target(&self) -> Target {
        Target::thing(self.kind.key(), self.id.id())
    }
}

/// Who and what one thing is shared with.
pub struct ThingShareList;

impl Operation for ThingShareList {
    const NAME: &'static str = "thing_share_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List one thing's shares";
    const DESCRIPTION: &'static str = "The accounts a page is shared with on its own and the \
        links that open it (never their secrets), with when each stops. Needs the right to edit \
        the page, as sharing does.";

    type Input = ThingShareListInput;
    type Output = ThingSharesView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThingShareListInput,
    ) -> Result<ThingSharesView, OpError> {
        let (kind, id) = (input.kind, input.id.id());
        let tx = call.tx().await?;
        Ok(shares(tx, kind, id).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingShareInput {
    /// What kind of thing: `page`.
    pub kind: ThingKind,
    /// The thing: its id or a link to it.
    pub id: IdRef,
    /// The account to share it with: a member of this workspace.
    pub account: IdRef,
    /// When the share stops (RFC 3339). Leave out to keep it until it is
    /// unshared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl Targeted for ThingShareInput {
    fn target(&self) -> Target {
        Target::thing(self.kind.key(), self.id.id())
    }
}

/// Share one thing with one account.
pub struct ThingShare;

impl Operation for ThingShare {
    const NAME: &'static str = "thing_share_add";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["thing.share_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Share one thing";
    const DESCRIPTION: &'static str = "Let one member of this workspace read one page that is \
        in a space they aren't in, without giving them anything else there. Sharing again \
        changes when it stops. Needs the right to edit the page. Undo it with thing_share_remove.";

    type Input = ThingShareInput;
    type Output = ThingSharesView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThingShareInput,
    ) -> Result<ThingSharesView, OpError> {
        let (workspace_id, space_id) = located(call)?;
        let (kind, id, account) = (input.kind, input.id.id(), input.account.id());
        if input.expires_at.is_some_and(|at| at <= Utc::now()) {
            return Err(OpError::invalid("expiresAt", "Pick a time in the future."));
        }
        let by = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_memberships membership \
             JOIN actors account ON account.id = membership.actor_id AND account.status = 'active' \
             WHERE membership.organization_id = $1 AND membership.actor_id = $2 \
               AND membership.enabled)",
        )
        .bind(workspace_id)
        .bind(account)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if !member {
            return Err(OpError::not_found("That account"));
        }
        sqlx::query(
            "UPDATE thing_grants SET revoked_at = now() \
             WHERE thing_kind = $1 AND thing_id = $2 AND grantee_account_id = $3 \
               AND revoked_at IS NULL",
        )
        .bind(kind.key())
        .bind(id)
        .bind(account)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        sqlx::query(
            "INSERT INTO thing_grants (id, workspace_id, space_id, thing_kind, thing_id, \
                 grantee_account_id, level, expires_at, granted_by_account_id) \
             VALUES ($1, $2, $3, $4, $5, $6, 'see', $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(space_id)
        .bind(kind.key())
        .bind(id)
        .bind(account)
        .bind(input.expires_at)
        .bind(by)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target(kind.key(), id, json!({ "sharedWith": account }));
        share_changed(call, kind, id).await?;
        let tx = call.tx().await?;
        Ok(shares(tx, kind, id).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingUnshareInput {
    /// What kind of thing: `page`.
    pub kind: ThingKind,
    /// The thing: its id or a link to it.
    pub id: IdRef,
    /// The account to stop sharing it with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
    /// The link to revoke (its id from thing_link_create).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<IdRef>,
}

impl Targeted for ThingUnshareInput {
    fn target(&self) -> Target {
        Target::thing(self.kind.key(), self.id.id())
    }
}

/// Stop sharing one thing with an account, or revoke one of its links.
pub struct ThingUnshare;

impl Operation for ThingUnshare {
    const NAME: &'static str = "thing_share_remove";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["thing.share_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Stop sharing one thing";
    const DESCRIPTION: &'static str = "Stop sharing a page with one account (account), or \
        revoke one of its links (link); it stops at once. Needs the right to edit the page.";

    type Input = ThingUnshareInput;
    type Output = ThingSharesView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThingUnshareInput,
    ) -> Result<ThingSharesView, OpError> {
        let (kind, id) = (input.kind, input.id.id());
        let (column, value) = match (&input.account, &input.link) {
            (Some(account), None) => ("grantee_account_id", account.id()),
            (None, Some(link)) => ("id", link.id()),
            _ => {
                return Err(OpError::invalid(
                    "account",
                    "Name either the account or the link.",
                ));
            }
        };
        let tx = call.tx().await?;
        let revoked = sqlx::query(&format!(
            "UPDATE thing_grants SET revoked_at = now() \
             WHERE thing_kind = $1 AND thing_id = $2 AND {column} = $3 AND revoked_at IS NULL"
        ))
        .bind(kind.key())
        .bind(id)
        .bind(value)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?
        .rows_affected();
        if revoked == 0 {
            return Err(OpError::not_found("That share"));
        }
        call.audit_target(kind.key(), id, json!({ "unshared": value }));
        share_changed(call, kind, id).await?;
        let tx = call.tx().await?;
        Ok(shares(tx, kind, id).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingLinkCreateInput {
    /// What kind of thing: `page`.
    pub kind: ThingKind,
    /// The thing: its id or a link to it.
    pub id: IdRef,
    /// How long the link works, in hours: 1 to 2160 (90 days); 168 if left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_hours: Option<i64>,
    /// Let anyone with the link read the page without signing in. Left
    /// out, only someone signed in to this server can open it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
}

impl Targeted for ThingLinkCreateInput {
    fn target(&self) -> Target {
        Target::thing(self.kind.key(), self.id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThingLinkView {
    /// The link's id, to revoke it with thing_share_remove.
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The secret that opens the thing. Shown once: keep it now. Empty
    /// when the same request is retried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[schemars(with = "String")]
    pub expires_at: DateTime<Utc>,
}

/// Make a link that opens one thing to read.
pub struct ThingLinkCreate;

impl Operation for ThingLinkCreate {
    const NAME: &'static str = "thing_link_create";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["thing.share_changed"];
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    // The answer carries the secret, which is never stored for a replay:
    // the link's row keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Make a link to one thing";
    const DESCRIPTION: &'static str = "Make a link that lets anyone signed in who has it read \
        one page, and nothing else, until it expires (168 hours unless you say) or is revoked \
        with thing_share_remove; with public, anyone who has it may read the page without \
        signing in. The token is shown once. Needs the right to edit the page.";

    type Input = ThingLinkCreateInput;
    type Output = ThingLinkView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThingLinkCreateInput,
    ) -> Result<ThingLinkView, OpError> {
        let (workspace_id, space_id) = located(call)?;
        let hours = input.expires_in_hours.unwrap_or(DEFAULT_LINK_HOURS);
        if !(1..=MAX_LINK_HOURS).contains(&hours) {
            return Err(OpError::invalid("expiresInHours", "Give 1 to 2160 hours."));
        }
        let (kind, id) = (input.kind, input.id.id());
        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        let token = format!("{LINK_PREFIX}{}", URL_SAFE_NO_PAD.encode(secret));
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let link_id = Uuid::now_v7();
        let expires_at = Utc::now() + Duration::hours(hours);
        let by = call.ctx.caller.account_id;
        let request_id = call.ctx.request_id.clone();
        let tx = call.tx().await?;
        if let Some(request_id) = request_id.as_deref() {
            let made = sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(
                "SELECT id, expires_at FROM thing_grants \
                 WHERE granted_by_account_id = $1 AND request_id = $2",
            )
            .bind(by)
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if let Some((link_id, made_until)) = made {
                call.mark_replayed(true);
                return Ok(ThingLinkView {
                    id: link_id,
                    token: None,
                    expires_at: made_until.unwrap_or(expires_at),
                });
            }
        }
        let tx = call.tx().await?;
        sqlx::query(
            "INSERT INTO thing_grants (id, workspace_id, space_id, thing_kind, thing_id, \
                 link_prefix, link_digest, level, expires_at, granted_by_account_id, request_id, \
                 public) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'see', $8, $9, $10, $11)",
        )
        .bind(link_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(kind.key())
        .bind(id)
        .bind(&token[..LINK_PREFIX_LENGTH])
        .bind(digest.to_vec())
        .bind(expires_at)
        .bind(by)
        .bind(request_id.as_deref())
        .bind(input.public.unwrap_or(false))
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        // The audit row names the link, never its secret.
        call.audit_target(kind.key(), id, json!({ "link": link_id }));
        share_changed(call, kind, id).await?;
        Ok(ThingLinkView {
            id: link_id,
            token: Some(token),
            expires_at,
        })
    }
}
