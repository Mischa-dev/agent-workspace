//! `group_list`: the workspace's groups, for mention suggestions and for
//! anyone deciding whom `@handle` reaches. K1.27's writes: `group_create`,
//! `group_update`, `group_members_set`, `group_archive` and
//! `group_restore`, under "manage groups" (`members.groups`).

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use sqlx::PgConnection;

use crate::{
    error::AppError,
    events::{self, Audience, NewEvent, catalog},
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted},
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<GroupList>()
        .add::<GroupCreate>()
        .add::<GroupUpdate>()
        .add::<GroupMembersSet>()
        .add::<GroupArchive>()
        .add::<GroupRestore>();
}

/// The most accounts one call adds or removes, and one group holds.
const MAX_CHANGED: usize = 200;
const MAX_MEMBERS: i64 = 2000;

pub struct GroupList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupListInput {
    /// Only groups whose handle or name starts with this (as typed after
    /// `@`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Include each group's members' account ids.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub members: bool,
}

impl Targeted for GroupListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Mentioned as `@handle`.
    pub handle: String,
    pub name: String,
    pub description: String,
    pub member_count: i64,
    /// With `members`: who is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    pub member_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupListing {
    pub groups: Vec<GroupView>,
}

impl Operation for GroupList {
    const NAME: &'static str = "group_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List groups";
    const DESCRIPTION: &'static str = "The workspace's groups (not archived): each one's \
        @handle, name, description and member count, and with members: true who is in it. \
        Writing @handle in a message notifies the group's members who can read it.";

    type Input = GroupListInput;
    type Output = GroupListing;

    async fn run(call: &mut OpCall<'_>, input: GroupListInput) -> Result<GroupListing, OpError> {
        let prefix = input
            .prefix
            .as_deref()
            .map(|prefix| prefix.trim().trim_start_matches('@').to_lowercase())
            .filter(|prefix| !prefix.is_empty());
        if prefix
            .as_ref()
            .is_some_and(|prefix| prefix.chars().count() > 80)
        {
            return Err(OpError::invalid(
                "prefix",
                "A prefix is up to 80 characters.",
            ));
        }
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let groups = sqlx::query_as::<_, GroupView>(
            r#"
            SELECT grp.id, grp.handle, grp.name, grp.description,
                   (SELECT count(*) FROM account_group_members member
                    WHERE member.group_id = grp.id) AS member_count,
                   CASE WHEN $3 THEN ARRAY(
                       SELECT member.account_id FROM account_group_members member
                       WHERE member.group_id = grp.id
                       ORDER BY member.account_id
                   ) END AS member_ids
            FROM account_groups grp
            WHERE grp.workspace_id = $1
              AND grp.archived_at IS NULL
              AND ($2::text IS NULL
                   OR grp.handle LIKE replace(replace(replace($2, '\', '\\'), '%', '\%'), '_', '\_') || '%'
                   OR lower(grp.name) LIKE replace(replace(replace($2, '\', '\\'), '%', '\%'), '_', '\_') || '%')
            ORDER BY grp.handle
            LIMIT 200
            "#,
        )
        .bind(workspace)
        .bind(prefix)
        .bind(input.members)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(GroupListing { groups })
    }
}

// ---------------------------------------------------------------------------
// Writes

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct GroupDetails {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub handle: String,
    pub name: String,
    pub description: String,
    pub version: i64,
    pub archived: bool,
    /// Who is in it.
    #[schemars(with = "Vec<String>")]
    pub member_ids: Vec<Uuid>,
}

async fn details(
    connection: &mut PgConnection,
    workspace: Uuid,
    group: Uuid,
) -> Result<GroupDetails, OpError> {
    sqlx::query_as::<_, GroupDetails>(
        r#"
        SELECT grp.id, grp.handle, grp.name, grp.description, grp.version,
               grp.archived_at IS NOT NULL AS archived,
               ARRAY(SELECT member.account_id FROM account_group_members member
                     WHERE member.group_id = grp.id ORDER BY member.account_id) AS member_ids
        FROM account_groups grp
        WHERE grp.id = $1 AND grp.workspace_id = $2
        "#,
    )
    .bind(group)
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That group"))
}

/// The group `text` names in `workspace` (its id or @handle), locked.
pub(crate) async fn group_named(
    connection: &mut PgConnection,
    workspace: Uuid,
    text: &str,
) -> Result<(Uuid, i64, bool), OpError> {
    let text = text.trim();
    sqlx::query_as::<_, (Uuid, i64, bool)>(
        "SELECT id, version, archived_at IS NOT NULL FROM account_groups \
         WHERE workspace_id = $1 AND (id::text = $2 OR handle = lower($3)) \
         FOR UPDATE",
    )
    .bind(workspace)
    .bind(text)
    .bind(text.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That group"))
}

/// The enabled members of `workspace` that `accounts` name (ids or
/// @handles), each once.
async fn accounts_named(
    connection: &mut PgConnection,
    workspace: Uuid,
    accounts: &[String],
) -> Result<Vec<Uuid>, OpError> {
    if accounts.len() > MAX_CHANGED {
        return Err(OpError::invalid(
            "add",
            "Change up to 200 accounts at once.",
        ));
    }
    let mut found = Vec::with_capacity(accounts.len());
    for text in accounts {
        let text = text.trim();
        let account = sqlx::query_scalar::<_, Uuid>(
            "SELECT profile.actor_id FROM organization_actor_profiles profile \
             JOIN organization_memberships membership \
               ON membership.organization_id = profile.organization_id \
              AND membership.actor_id = profile.actor_id AND membership.enabled \
             WHERE profile.organization_id = $1 \
               AND (profile.actor_id::text = $2 OR lower(profile.handle) = lower($3)) \
             LIMIT 1",
        )
        .bind(workspace)
        .bind(text)
        .bind(text.trim_start_matches('@'))
        .fetch_optional(&mut *connection)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That account in this workspace"))?;
        if !found.contains(&account) {
            found.push(account);
        }
    }
    Ok(found)
}

fn checked_handle(handle: &str) -> Result<String, OpError> {
    let handle = handle.trim().trim_start_matches('@').to_lowercase();
    let mut characters = handle.chars();
    let shaped = handle.chars().count() >= 2
        && handle.chars().count() <= 32
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !shaped {
        return Err(OpError::invalid(
            "handle",
            "A handle is 2 to 32 lowercase letters, digits, - or _, starting with a letter.",
        ));
    }
    if matches!(handle.as_str(), "everyone" | "here" | "channel") {
        return Err(OpError::invalid(
            "handle",
            "@everyone, @here and @channel already mean something.",
        ));
    }
    Ok(handle)
}

fn checked_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "name",
            "A name is 1 to 80 characters on one line.",
        ));
    }
    Ok(name.to_owned())
}

fn checked_description(description: &str) -> Result<String, OpError> {
    let description = description.trim();
    if description.chars().count() > 300 {
        return Err(OpError::invalid(
            "description",
            "A description is up to 300 characters.",
        ));
    }
    Ok(description.to_owned())
}

/// A group's handle mustn't be someone's: `@handle` names one or the other.
async fn handle_free(
    connection: &mut PgConnection,
    workspace: Uuid,
    handle: &str,
    group: Option<Uuid>,
) -> Result<(), OpError> {
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
                        WHERE organization_id = $1 AND lower(handle) = $2) \
             OR EXISTS (SELECT 1 FROM account_groups \
                        WHERE workspace_id = $1 AND handle = $2 AND id IS DISTINCT FROM $3)",
    )
    .bind(workspace)
    .bind(handle)
    .bind(group)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::from)?;
    if taken {
        return Err(AppError::Conflict("group", "that handle is taken in this workspace").into());
    }
    Ok(())
}

async fn add_members(
    connection: &mut PgConnection,
    workspace: Uuid,
    group: Uuid,
    accounts: &[Uuid],
    by: Uuid,
) -> Result<(), OpError> {
    sqlx::query(
        "INSERT INTO account_group_members (group_id, workspace_id, account_id, added_by_actor_id) \
         SELECT $1, $2, account, $4 FROM unnest($3::uuid[]) AS account \
         ON CONFLICT (group_id, account_id) DO NOTHING",
    )
    .bind(group)
    .bind(workspace)
    .bind(accounts)
    .bind(by)
    .execute(&mut *connection)
    .await
    .map_err(AppError::from)?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM account_group_members WHERE group_id = $1",
    )
    .bind(group)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::from)?;
    if count > MAX_MEMBERS {
        return Err(OpError::invalid(
            "add",
            "A group holds up to 2,000 accounts.",
        ));
    }
    Ok(())
}

/// `group.changed`, heard by the workspace (ids only).
async fn changed(
    connection: &mut PgConnection,
    workspace: Uuid,
    group: Uuid,
    version: i64,
    by: Uuid,
) -> Result<(), OpError> {
    let mut event = NewEvent::thing(&catalog::GROUP_CHANGED, Uuid::nil(), "group", group)
        .workspace(workspace)
        .version(version)
        .actor(by)
        .audience(Audience::Workspace);
    event.space_id = None;
    events::emit(connection, event).await?;
    Ok(())
}

// group_create

pub struct GroupCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupCreateInput {
    /// Mentioned as `@handle`: 2 to 32 lowercase letters, digits, - or _.
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Who is in it: account ids or @handles of this workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

impl Targeted for GroupCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for GroupCreate {
    const NAME: &'static str = "group_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersGroups);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["group.changed"];
    const TITLE: &'static str = "Make a group";
    const DESCRIPTION: &'static str = "Make a group of people and agents in this workspace, \
        mentioned as @handle. A group can be given a role in a space (space_group_add). \
        Returns the group.";

    type Input = GroupCreateInput;
    type Output = GroupDetails;

    async fn run(call: &mut OpCall<'_>, input: GroupCreateInput) -> Result<GroupDetails, OpError> {
        let handle = checked_handle(&input.handle)?;
        let name = checked_name(&input.name)?;
        let description = checked_description(input.description.as_deref().unwrap_or(""))?;
        let workspace = call.ctx.workspace_id;
        let by = call.account_id();
        let tx = call.tx().await?;
        handle_free(tx, workspace, &handle, None).await?;
        let members = accounts_named(tx, workspace, &input.members).await?;
        let group = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO account_groups (id, workspace_id, handle, name, description, created_by_actor_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(group)
        .bind(workspace)
        .bind(&handle)
        .bind(&name)
        .bind(&description)
        .bind(by)
        .execute(&mut **tx)
        .await
        .map_err(|error| match AppError::from(error) {
            AppError::Database(sqlx::Error::Database(database))
                if database.is_unique_violation() =>
            {
                AppError::Conflict("group", "that handle is taken in this workspace")
            }
            other => other,
        })?;
        add_members(tx, workspace, group, &members, by).await?;
        changed(tx, workspace, group, 1, by).await?;
        call.audit_target(
            "group",
            group,
            serde_json::json!({ "members": members.len() }),
        );
        let tx = call.tx().await?;
        details(tx, workspace, group).await
    }
}

// group_update

pub struct GroupUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupUpdateInput {
    /// The group: its id or @handle.
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The version you read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for GroupUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for GroupUpdate {
    const NAME: &'static str = "group_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersGroups);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["group.changed"];
    const TITLE: &'static str = "Change a group";
    const DESCRIPTION: &'static str = "Rename a group, or change its @handle or description. \
        Returns the group.";

    type Input = GroupUpdateInput;
    type Output = GroupDetails;

    async fn run(call: &mut OpCall<'_>, input: GroupUpdateInput) -> Result<GroupDetails, OpError> {
        let handle = input.handle.as_deref().map(checked_handle).transpose()?;
        let name = input.name.as_deref().map(checked_name).transpose()?;
        let description = input
            .description
            .as_deref()
            .map(checked_description)
            .transpose()?;
        let workspace = call.ctx.workspace_id;
        let by = call.account_id();
        let tx = call.tx().await?;
        let (group, version, _) = group_named(tx, workspace, &input.group).await?;
        if let Some(expected) = input.expected_version
            && expected != version
        {
            return Err(AppError::VersionConflict {
                resource: "group",
                current_version: version,
            }
            .into());
        }
        if let Some(handle) = &handle {
            handle_free(tx, workspace, handle, Some(group)).await?;
        }
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE account_groups SET handle = COALESCE($2, handle), name = COALESCE($3, name), \
                 description = COALESCE($4, description), version = version + 1 \
             WHERE id = $1 RETURNING version",
        )
        .bind(group)
        .bind(handle)
        .bind(name)
        .bind(description)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        changed(tx, workspace, group, version, by).await?;
        call.audit_target("group", group, serde_json::json!({}));
        let tx = call.tx().await?;
        details(tx, workspace, group).await
    }
}

// group_members_set

pub struct GroupMembersSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupMembersSetInput {
    /// The group: its id or @handle.
    pub group: String,
    /// Accounts to put in it: ids or @handles of this workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    /// Accounts to take out of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

impl Targeted for GroupMembersSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for GroupMembersSet {
    const NAME: &'static str = "group_members_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersGroups);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["group.changed", "space.member.changed"];
    const TITLE: &'static str = "Change who is in a group";
    const DESCRIPTION: &'static str = "Put accounts in a group and take others out. They gain \
        or lose the spaces the group holds a role in. Returns the group.";

    type Input = GroupMembersSetInput;
    type Output = GroupDetails;

    async fn run(
        call: &mut OpCall<'_>,
        input: GroupMembersSetInput,
    ) -> Result<GroupDetails, OpError> {
        if input.add.is_empty() && input.remove.is_empty() {
            return Err(OpError::invalid("add", "Name someone to add or remove."));
        }
        let workspace = call.ctx.workspace_id;
        let by = call.account_id();
        let tx = call.tx().await?;
        let (group, _, archived) = group_named(tx, workspace, &input.group).await?;
        if archived {
            return Err(AppError::Conflict("group", "that group is archived").into());
        }
        let adding = accounts_named(tx, workspace, &input.add).await?;
        if input.remove.len() > MAX_CHANGED {
            return Err(OpError::invalid(
                "remove",
                "Change up to 200 accounts at once.",
            ));
        }
        let mut removing = Vec::new();
        for text in &input.remove {
            let text = text.trim();
            let account = sqlx::query_scalar::<_, Uuid>(
                "SELECT member.account_id FROM account_group_members member \
                 LEFT JOIN organization_actor_profiles profile \
                   ON profile.organization_id = member.workspace_id \
                  AND profile.actor_id = member.account_id \
                 WHERE member.group_id = $1 \
                   AND (member.account_id::text = $2 OR lower(profile.handle) = lower($3))",
            )
            .bind(group)
            .bind(text)
            .bind(text.trim_start_matches('@'))
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?
            .ok_or_else(|| OpError::not_found("That account in this group"))?;
            removing.push(account);
        }
        add_members(tx, workspace, group, &adding, by).await?;
        sqlx::query(
            "DELETE FROM account_group_members WHERE group_id = $1 AND account_id = ANY($2)",
        )
        .bind(group)
        .bind(&removing)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE account_groups SET version = version + 1 WHERE id = $1 RETURNING version",
        )
        .bind(group)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        changed(tx, workspace, group, version, by).await?;
        seats_changed(tx, workspace, group, by).await?;
        call.audit_target(
            "group",
            group,
            serde_json::json!({ "added": adding, "removed": removing }),
        );
        let tx = call.tx().await?;
        details(tx, workspace, group).await
    }
}

/// `space.member.changed` for each space the group holds a role in, whose
/// seats a change of members or archiving moved.
pub(crate) async fn seats_changed(
    connection: &mut PgConnection,
    workspace: Uuid,
    group: Uuid,
    by: Uuid,
) -> Result<(), OpError> {
    let spaces = sqlx::query_scalar::<_, Uuid>(
        "SELECT space_id FROM role_grants WHERE group_id = $1 AND workspace_id = $2 ORDER BY space_id",
    )
    .bind(group)
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await
    .map_err(AppError::from)?;
    for space in spaces {
        events::emit(
            connection,
            NewEvent::thing(&catalog::SPACE_MEMBER_CHANGED, space, "space", space)
                .workspace(workspace)
                .actor(by)
                .payload(serde_json::json!({ "groupId": group })),
        )
        .await?;
    }
    Ok(())
}

// group_archive and group_restore

pub struct GroupArchive;
pub struct GroupRestore;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupRef {
    /// The group: its id or @handle.
    pub group: String,
}

impl Targeted for GroupRef {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

async fn set_archived(
    call: &mut OpCall<'_>,
    input: GroupRef,
    archive: bool,
) -> Result<GroupDetails, OpError> {
    let workspace = call.ctx.workspace_id;
    let by = call.account_id();
    let tx = call.tx().await?;
    let (group, _, archived) = group_named(tx, workspace, &input.group).await?;
    if archived != archive {
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE account_groups \
             SET archived_at = CASE WHEN $2 THEN now() END, version = version + 1 \
             WHERE id = $1 RETURNING version",
        )
        .bind(group)
        .bind(archive)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        changed(tx, workspace, group, version, by).await?;
        seats_changed(tx, workspace, group, by).await?;
    }
    call.audit_target("group", group, serde_json::json!({ "archived": archive }));
    let tx = call.tx().await?;
    details(tx, workspace, group).await
}

impl Operation for GroupArchive {
    const NAME: &'static str = "group_archive";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersGroups);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["group.changed", "space.member.changed"];
    const TITLE: &'static str = "Archive a group";
    const DESCRIPTION: &'static str = "Archive a group: @handle stops reaching it, and its \
        members lose the spaces only it gave them. group_restore brings it back.";

    type Input = GroupRef;
    type Output = GroupDetails;

    async fn run(call: &mut OpCall<'_>, input: GroupRef) -> Result<GroupDetails, OpError> {
        set_archived(call, input, true).await
    }
}

impl Operation for GroupRestore {
    const NAME: &'static str = "group_restore";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersGroups);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["group.changed", "space.member.changed"];
    const TITLE: &'static str = "Bring back a group";
    const DESCRIPTION: &'static str = "Bring an archived group back, with the roles it held.";

    type Input = GroupRef;
    type Output = GroupDetails;

    async fn run(call: &mut OpCall<'_>, input: GroupRef) -> Result<GroupDetails, OpError> {
        set_archived(call, input, false).await
    }
}
