//! Space membership operations (K4.8): `space_member_add`,
//! `space_member_update`, `space_member_remove`, `space_join` and
//! `space_leave`. Adding and changing go through `memberships`, which
//! checks who may give which role; every change writes
//! `space.member.changed` with the account it is about.
//!
//! The workspace's own space follows the workspace's members, and a
//! personal space has one member, so neither is changed here.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::SpaceKind;
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    memberships,
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<SpaceMemberAdd>();
    registry.add::<SpaceMemberUpdate>();
    registry.add::<SpaceMemberRemove>();
    registry.add::<SpaceJoin>();
    registry.add::<SpaceLeave>();
    registry.add::<SpaceGroupAdd>();
    registry.add::<SpaceGroupRemove>();
}

/// Someone's membership of a space.
#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpaceMemberView {
    #[schemars(with = "String")]
    pub space_id: Uuid,
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub name: String,
    pub handle: String,
    /// `owner`, `admin`, `member`, `viewer`, or the role a custom one is
    /// based on (see roleId).
    pub role: String,
    /// The workspace's custom role they hold here, if any.
    #[schemars(with = "Option<String>")]
    pub role_id: Option<Uuid>,
    /// False once they left or were removed.
    pub active: bool,
    /// In through a group that holds a role here (K1.27), not added
    /// themselves.
    pub via_group: bool,
    /// When their membership ends by itself, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Pass as expectedVersion to change it.
    pub version: i64,
}

async fn member(
    connection: &mut PgConnection,
    space: Uuid,
    account: Uuid,
) -> Result<SpaceMemberView, AppError> {
    sqlx::query_as::<_, SpaceMemberView>(
        "SELECT project_id AS space_id, actor_id AS account_id, display_name AS name, handle, \
         role, custom_role_id(role_id) AS role_id, enabled AS active, via_group, \
         to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS expires_at, \
         version \
         FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(space)
    .bind(account)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("space member"))
}

/// The space's kind, refusing those whose members aren't set by hand.
async fn managed_space(connection: &mut PgConnection, space: Uuid) -> Result<(), OpError> {
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM spaces WHERE id = $1")
        .bind(space)
        .fetch_optional(&mut *connection)
        .await
        .map_err(AppError::from)?;
    match kind.as_deref().and_then(SpaceKind::from_stored) {
        Some(SpaceKind::Project | SpaceKind::Team) => Ok(()),
        Some(SpaceKind::Workspace) => Err(OpError::invalid(
            "spaceId",
            "Everyone in the workspace is in its own space; change their workspace role instead.",
        )),
        Some(SpaceKind::Personal) => Err(OpError::invalid(
            "spaceId",
            "A personal space has one member: its owner.",
        )),
        None => Err(OpError::not_found("That space")),
    }
}

/// The account `text` names in the space's workspace: its id or @handle.
async fn account_in(
    connection: &mut PgConnection,
    space: Uuid,
    text: &str,
) -> Result<Uuid, OpError> {
    let text = text.trim();
    sqlx::query_scalar::<_, Uuid>(
        "SELECT profile.actor_id FROM spaces space \
         JOIN organization_actor_profiles profile ON profile.organization_id = space.workspace_id \
         WHERE space.id = $1 AND (profile.actor_id::text = $2 OR lower(profile.handle) = lower($3)) \
         LIMIT 1",
    )
    .bind(space)
    .bind(text)
    .bind(text.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That account in this workspace"))
}

/// A built-in role by name, or a custom role of the workspace by name or id.
async fn role_named(
    connection: &mut PgConnection,
    space: Uuid,
    text: &str,
) -> Result<(Option<String>, Option<Uuid>), OpError> {
    let text = text.trim();
    if matches!(text, "admin" | "member" | "viewer") {
        return Ok((Some(text.to_owned()), None));
    }
    let workspace = sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM spaces WHERE id = $1")
        .bind(space)
        .fetch_one(&mut *connection)
        .await
        .map_err(AppError::from)?;
    match crate::roles::find_role(connection, workspace, text).await? {
        Some(role_id) => Ok((None, Some(role_id))),
        None => Err(OpError::invalid(
            "role",
            "Use admin, member, viewer, or a custom role's name from role_list. Ownership is \
             handed over in the web app.",
        )),
    }
}

/// `text` as an end time: RFC 3339 and in the future; empty for none.
pub(crate) fn expiry(text: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>, OpError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let at = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|_| OpError::invalid("expiresAt", "Give a time such as 2026-12-01T09:00:00Z."))?
        .with_timezone(&chrono::Utc);
    if at <= chrono::Utc::now() {
        return Err(OpError::invalid("expiresAt", "Give a time in the future."));
    }
    Ok(Some(at))
}

async fn set_expiry(
    connection: &mut PgConnection,
    space: Uuid,
    account: Uuid,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), OpError> {
    sqlx::query(
        "UPDATE project_memberships SET expires_at = $3 \
         WHERE project_id = $1 AND actor_id = $2 AND NOT via_group",
    )
    .bind(space)
    .bind(account)
    .bind(expires_at)
    .execute(&mut *connection)
    .await
    .map_err(AppError::from)?;
    Ok(())
}

fn refused(error: AppError) -> OpError {
    match error {
        AppError::Forbidden => OpError::Denied {
            reason: DenyReason::Role {
                permission: Perm::MembersManage,
            },
            message: "You can't give or change that access: only a role within your own, admins \
                manage members and viewers, only owners manage admins, and nobody changes \
                their own."
                .into(),
        },
        AppError::NotFound("project") => OpError::not_found("That space"),
        error => error.into(),
    }
}

// ---------------------------------------------------------------------------
// space_member_add

pub struct SpaceMemberAdd;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceMemberAddInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// Who: an account's id or @handle in this workspace.
    pub account: String,
    /// `member` (the default), `viewer`, or a custom role of the workspace
    /// based on one of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// When their membership ends by itself (RFC 3339, in the future), for
    /// a guest or anyone here for a while.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl Targeted for SpaceMemberAddInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceMemberAdd {
    const NAME: &'static str = "space_member_add";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Add someone to a space";
    const DESCRIPTION: &'static str = "Add a person or agent of this workspace to a project or \
        team space, as a member, a viewer or a custom role within yours. Someone already in it \
        is changed with space_member_update. Returns their membership.";

    type Input = SpaceMemberAddInput;
    type Output = SpaceMemberView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceMemberAddInput,
    ) -> Result<SpaceMemberView, OpError> {
        let space = input.space_id.id();
        let caller = call.account_id();
        let tx = call.tx().await?;
        managed_space(tx, space).await?;
        let account = account_in(tx, space, &input.account).await?;
        let (role, role_id) = match input.role.as_deref() {
            None => (Some("member".to_owned()), None),
            Some(text) => role_named(tx, space, text).await?,
        };
        let added = memberships::add_member(
            tx,
            caller,
            space,
            account,
            role.as_deref().unwrap_or("member"),
            role_id,
        )
        .await
        .map_err(refused)?;
        if let Some(expires_at) = input.expires_at.as_deref() {
            set_expiry(tx, space, added.member.actor_id, expiry(expires_at)?).await?;
        }
        let view = member(tx, space, added.member.actor_id).await?;
        call.audit_target(
            "account",
            account,
            json!({ "space": space, "role": view.role, "roleId": view.role_id }),
        );
        Ok(view)
    }
}

// ---------------------------------------------------------------------------
// space_member_update and space_member_remove

pub struct SpaceMemberUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceMemberUpdateInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// Who: an account's id or @handle.
    pub account: String,
    /// `admin`, `member`, `viewer`, or a custom role of the workspace.
    pub role: String,
    /// When their membership ends by itself (RFC 3339, in the future); an
    /// empty string makes it last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Their membership's version, as you read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for SpaceMemberUpdateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceMemberUpdate {
    const NAME: &'static str = "space_member_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Change someone's role in a space";
    const DESCRIPTION: &'static str = "Give a member of a space another role, within yours. \
        Someone removed comes back with it. Nobody changes their own role. Returns their \
        membership.";

    type Input = SpaceMemberUpdateInput;
    type Output = SpaceMemberView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceMemberUpdateInput,
    ) -> Result<SpaceMemberView, OpError> {
        let space = input.space_id.id();
        let caller = call.account_id();
        let tx = call.tx().await?;
        managed_space(tx, space).await?;
        let account = account_in(tx, space, &input.account).await?;
        let (role, role_id) = role_named(tx, space, &input.role).await?;
        let expires_at = input.expires_at.as_deref().map(expiry).transpose()?;
        memberships::change_member(
            tx,
            caller,
            space,
            account,
            role.as_deref(),
            role_id,
            Some(true),
            input.expected_version,
        )
        .await
        .map_err(refused)?;
        if let Some(expires_at) = expires_at {
            set_expiry(tx, space, account, expires_at).await?;
        }
        let view = member(tx, space, account).await?;
        call.audit_target(
            "account",
            account,
            json!({ "space": space, "role": view.role, "roleId": view.role_id }),
        );
        Ok(view)
    }
}

pub struct SpaceMemberRemove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceMemberRemoveInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// Who: an account's id or @handle.
    pub account: String,
    /// Their membership's version, as you read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for SpaceMemberRemoveInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceMemberRemove {
    const NAME: &'static str = "space_member_remove";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Remove someone from a space";
    const DESCRIPTION: &'static str = "Take someone out of a project or team space: they stop \
        seeing it and their credentials held to it stop. Their work stays. A space always keeps \
        an owner. Returns their membership.";

    type Input = SpaceMemberRemoveInput;
    type Output = SpaceMemberView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceMemberRemoveInput,
    ) -> Result<SpaceMemberView, OpError> {
        let space = input.space_id.id();
        let caller = call.account_id();
        let tx = call.tx().await?;
        managed_space(tx, space).await?;
        let account = account_in(tx, space, &input.account).await?;
        memberships::change_member(
            tx,
            caller,
            space,
            account,
            None,
            None,
            Some(false),
            input.expected_version,
        )
        .await
        .map_err(refused)?;
        let view = member(tx, space, account).await?;
        call.audit_target(
            "account",
            account,
            json!({ "space": space, "removed": true }),
        );
        Ok(view)
    }
}

// ---------------------------------------------------------------------------
// space_join and space_leave

pub struct SpaceJoin;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceJoinInput {
    /// A project or team space open to the workspace: its id or link.
    pub space_id: IdRef,
}

impl Targeted for SpaceJoinInput {
    // Not a member yet: the space is found by whether the caller may read
    // it, and its workspace is the target.
    fn target(&self) -> Target {
        Target::thing("readable_space", self.space_id.id())
    }
}

impl Operation for SpaceJoin {
    const NAME: &'static str = "space_join";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Join a space";
    const DESCRIPTION: &'static str = "Join a project or team space open to the workspace \
        (space_list shows them), as a member. A space only its members find is joined by being \
        added. Returns your membership.";

    type Input = SpaceJoinInput;
    type Output = SpaceMemberView;

    async fn run(call: &mut OpCall<'_>, input: SpaceJoinInput) -> Result<SpaceMemberView, OpError> {
        let space = input.space_id.id();
        let account = call.account_id();
        if call
            .ctx
            .caller
            .confined_space
            .is_some_and(|confined| confined != space)
        {
            return Err(OpError::not_found("That space"));
        }
        let tx = call.tx().await?;
        memberships::join(tx, account, space)
            .await
            .map_err(|error| match error {
                AppError::NotFound(_) => OpError::invalid(
                    "spaceId",
                    "Only a project or team space open to the workspace can be joined. Ask \
                     someone in it to add you.",
                ),
                error => error.into(),
            })?;
        let view = member(tx, space, account).await?;
        call.audit_target(
            "account",
            account,
            json!({ "space": space, "joined": true }),
        );
        Ok(view)
    }
}

pub struct SpaceLeave;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceLeaveInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
}

impl Targeted for SpaceLeaveInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceLeave {
    const NAME: &'static str = "space_leave";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Leave a space";
    const DESCRIPTION: &'static str = "Leave a project or team space you're in. Your work there \
        stays, and your credentials held to it stop. Its last owner hands it over first. \
        Returns your membership.";

    type Input = SpaceLeaveInput;
    type Output = SpaceMemberView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceLeaveInput,
    ) -> Result<SpaceMemberView, OpError> {
        let space = input.space_id.id();
        let account = call.account_id();
        let tx = call.tx().await?;
        let view = member(tx, space, account).await?;
        memberships::leave(tx, account, space).await?;
        let after = member(tx, space, account).await.unwrap_or(view);
        call.audit_target("account", account, json!({ "space": space, "left": true }));
        Ok(after)
    }
}

// ---------------------------------------------------------------------------
// space_group_add and space_group_remove (K1.27)

/// A group's role in a space.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpaceGroupView {
    #[schemars(with = "String")]
    pub space_id: Uuid,
    #[schemars(with = "String")]
    pub group_id: Uuid,
    pub handle: String,
    /// `member`, `viewer`, or the role a custom one is based on.
    pub role: String,
    #[schemars(with = "String")]
    pub role_id: Uuid,
    /// How many of its members are in the space through it.
    pub seats: i64,
}

async fn space_group(
    connection: &mut PgConnection,
    space: Uuid,
    group: Uuid,
) -> Result<SpaceGroupView, AppError> {
    sqlx::query_as::<_, SpaceGroupView>(
        "SELECT grant_row.space_id, grant_row.group_id, grp.handle, held.base_role AS role, \
                grant_row.role_id, \
                (SELECT count(*) FROM project_memberships seat \
                 JOIN account_group_members member \
                   ON member.group_id = grant_row.group_id AND member.account_id = seat.actor_id \
                 WHERE seat.project_id = grant_row.space_id AND seat.enabled AND seat.via_group) \
                  AS seats \
         FROM role_grants grant_row \
         JOIN account_groups grp ON grp.id = grant_row.group_id \
         JOIN workspace_roles held ON held.id = grant_row.role_id \
         WHERE grant_row.space_id = $1 AND grant_row.group_id = $2",
    )
    .bind(space)
    .bind(group)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("space group"))
}

pub struct SpaceGroupAdd;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceGroupAddInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// The group: its id or @handle.
    pub group: String,
    /// `member` (the default), `viewer`, or a custom role of the workspace
    /// based on one of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl Targeted for SpaceGroupAddInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceGroupAdd {
    const NAME: &'static str = "space_group_add";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Give a group a role in a space";
    const DESCRIPTION: &'static str = "Give a group of this workspace a role in a project or \
        team space, within yours: everyone in the group is in the space with it, and people \
        joining or leaving the group follow. Someone already in the space keeps their own role \
        too. Given again, the group's role changes. Returns the group's place in the space.";

    type Input = SpaceGroupAddInput;
    type Output = SpaceGroupView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceGroupAddInput,
    ) -> Result<SpaceGroupView, OpError> {
        let space = input.space_id.id();
        let caller = call.account_id();
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        managed_space(tx, space).await?;
        let (group, _, archived) =
            crate::groups::ops::group_named(tx, workspace, &input.group).await?;
        if archived {
            return Err(AppError::Conflict("group", "that group is archived").into());
        }
        let (role, role_id) = match input.role.as_deref() {
            None => (Some("member".to_owned()), None),
            Some(text) => role_named(tx, space, text).await?,
        };
        let (base, role_id) = match (role, role_id) {
            (Some(key), None) => {
                let role_id = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM workspace_roles \
                     WHERE organization_id = $1 AND built_in AND key = $2",
                )
                .bind(workspace)
                .bind(&key)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?;
                (key, role_id)
            }
            (_, Some(role_id)) => {
                let (base, _) = memberships::pick_custom_role(tx, space, role_id)
                    .await
                    .map_err(refused)?;
                (base, role_id)
            }
            (None, None) => return Err(OpError::invalid("role", "Name a role.")),
        };
        if !matches!(base.as_str(), "member" | "viewer") {
            return Err(OpError::invalid(
                "role",
                "A group holds member, viewer, or a custom role based on one of them.",
            ));
        }
        memberships::require_grantable(tx, caller, space, &base, Some(role_id))
            .await
            .map_err(refused)?;
        sqlx::query(
            "INSERT INTO role_grants (id, workspace_id, space_id, group_id, role_id, granted_by_actor_id) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (space_id, group_id) DO UPDATE \
             SET role_id = EXCLUDED.role_id, granted_by_actor_id = EXCLUDED.granted_by_actor_id \
             WHERE role_grants.role_id IS DISTINCT FROM EXCLUDED.role_id",
        )
        .bind(Uuid::now_v7())
        .bind(workspace)
        .bind(space)
        .bind(group)
        .bind(role_id)
        .bind(caller)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        crate::groups::ops::seats_changed(tx, workspace, group, caller).await?;
        let view = space_group(tx, space, group).await?;
        call.audit_target(
            "group",
            group,
            json!({ "space": space, "role": view.role, "roleId": view.role_id }),
        );
        Ok(view)
    }
}

pub struct SpaceGroupRemove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceGroupRemoveInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// The group: its id or @handle.
    pub group: String,
}

impl Targeted for SpaceGroupRemoveInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

/// What taking a group out of a space did.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceGroupRemoved {
    #[schemars(with = "String")]
    pub space_id: Uuid,
    #[schemars(with = "String")]
    pub group_id: Uuid,
    /// False when the group held no role there.
    pub removed: bool,
}

impl Operation for SpaceGroupRemove {
    const NAME: &'static str = "space_group_remove";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.member.changed"];
    const TITLE: &'static str = "Take a group out of a space";
    const DESCRIPTION: &'static str = "Take back the role a group holds in a space: those in \
        the space only through it leave it; those added themselves stay.";

    type Input = SpaceGroupRemoveInput;
    type Output = SpaceGroupRemoved;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceGroupRemoveInput,
    ) -> Result<SpaceGroupRemoved, OpError> {
        let space = input.space_id.id();
        let caller = call.account_id();
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        managed_space(tx, space).await?;
        let (group, _, _) = crate::groups::ops::group_named(tx, workspace, &input.group).await?;
        let held = sqlx::query_as::<_, (String, Uuid)>(
            "SELECT held.base_role, held.id FROM role_grants grant_row \
             JOIN workspace_roles held ON held.id = grant_row.role_id \
             WHERE grant_row.space_id = $1 AND grant_row.group_id = $2",
        )
        .bind(space)
        .bind(group)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let removed = held.is_some();
        if let Some((base, role_id)) = held {
            // Taking a role back is within the caller's reach, as giving it is.
            memberships::require_grantable(tx, caller, space, &base, Some(role_id))
                .await
                .map_err(refused)?;
            events::emit(
                tx,
                NewEvent::thing(&catalog::SPACE_MEMBER_CHANGED, space, "space", space)
                    .workspace(workspace)
                    .actor(caller)
                    .payload(json!({ "groupId": group })),
            )
            .await?;
            sqlx::query("DELETE FROM role_grants WHERE space_id = $1 AND group_id = $2")
                .bind(space)
                .bind(group)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        }
        call.audit_target(
            "group",
            group,
            json!({ "space": space, "removed": removed }),
        );
        Ok(SpaceGroupRemoved {
            space_id: space,
            group_id: group,
            removed,
        })
    }
}
