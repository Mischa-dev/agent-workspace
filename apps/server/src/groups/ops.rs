//! `group_list`: the workspace's groups, for mention suggestions and for
//! anyone deciding whom `@handle` reaches.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted},
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<GroupList>();
}

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
