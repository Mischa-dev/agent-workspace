//! `guard_set` and `guard_list` (A2.27): a workspace's guards, set by
//! those who manage its settings.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<GuardWrite>().add::<GuardList>();
}

/// What a guard does with a change it matches.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardAction {
    /// Stop it.
    Deny,
    /// Hold it until a reviewer allows it.
    Review,
}

impl GuardAction {
    const fn key(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::Review => "review",
        }
    }
}

/// One guard.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct GuardView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    pub name: String,
    /// deny or review.
    pub action: String,
    pub operations: Vec<String>,
    pub target_kinds: Vec<String>,
    pub terms: Vec<String>,
    #[schemars(with = "Vec<String>")]
    pub accounts: Vec<Uuid>,
    #[schemars(with = "Vec<String>")]
    pub reviewers: Vec<Uuid>,
    pub enabled: bool,
    pub version: i64,
}

const SELECT: &str = "SELECT id, space_id, name, action, operations, target_kinds, terms, \
    accounts, reviewers, enabled, version FROM guards";

/// Make, change or remove a guard.
pub struct GuardWrite;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardWriteInput {
    /// The guard to change; left out, a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_id: Option<IdRef>,
    /// Only changes in this space; left out, the whole workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// deny stops a matching change; review holds it for the reviewers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<GuardAction>,
    /// Operation names it watches, such as message_send; empty: every change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<String>>,
    /// Kinds of thing it watches, such as task; empty: any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kinds: Option<Vec<String>>,
    /// Words it looks for in the change's text; empty: any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<Vec<String>>,
    /// The accounts it watches; empty: everyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    pub accounts: Option<Vec<Uuid>>,
    /// For review: who decides (people or agents in the space).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    pub reviewers: Option<Vec<Uuid>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// true removes the guard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
    /// The version you read, when changing one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for GuardWriteInput {
    fn target(&self) -> Target {
        match self.space {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuardWriteOutput {
    /// The guard as it is now; null once removed.
    pub guard: Option<GuardView>,
}

fn words(field: &'static str, values: Vec<String>, max: usize) -> Result<Vec<String>, OpError> {
    let values = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.len() > max
        || values
            .iter()
            .any(|value| value.chars().count() > 200 || value.chars().any(char::is_control))
    {
        return Err(OpError::invalid(
            field,
            "Keep each entry to one line of 200 characters, and the list short.",
        ));
    }
    Ok(values)
}

impl Operation for GuardWrite {
    const NAME: &'static str = "guard_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set a guard";
    const DESCRIPTION: &'static str = "Make, change or remove a guard: a rule on the changes \
        accounts make. deny stops a match; review holds it, answering status pending_review \
        with the request to wait on: call again once it is allowed.";
    const EMITS: &'static [&'static str] = &["guards.changed"];

    type Input = GuardWriteInput;
    type Output = GuardWriteOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: GuardWriteInput,
    ) -> Result<GuardWriteOutput, OpError> {
        let account = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let space_id = input.space.map(IdRef::id);
        let tx = call.tx().await?;
        let existing = match input.guard_id {
            Some(guard) => Some(
                sqlx::query_as::<_, GuardView>(&format!(
                    // In the scope the caller was authorized for: a guard
                    // of the whole workspace or another space is not one
                    // a space's managers can change.
                    "{SELECT} WHERE id = $1 AND workspace_id = $2 \
                     AND space_id IS NOT DISTINCT FROM $3 FOR UPDATE"
                ))
                .bind(guard.id())
                .bind(workspace_id)
                .bind(space_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?
                .ok_or_else(|| OpError::not_found("That guard"))?,
            ),
            None => None,
        };
        if let (Some(existing), Some(expected)) = (&existing, input.expected_version)
            && existing.version != expected
        {
            return Err(AppError::VersionConflict {
                resource: "guard",
                current_version: existing.version,
            }
            .into());
        }
        let id = match (&existing, input.remove) {
            (Some(existing), true) => {
                sqlx::query("DELETE FROM guards WHERE id = $1")
                    .bind(existing.id)
                    .execute(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
                None
            }
            (None, true) => return Err(OpError::invalid("remove", "Name the guard to remove.")),
            (existing, false) => {
                let existing = existing.as_ref();
                let name = input
                    .name
                    .map(|name| name.trim().to_owned())
                    .or_else(|| existing.map(|guard| guard.name.clone()))
                    .filter(|name| !name.is_empty() && name.chars().count() <= 120)
                    .ok_or_else(|| OpError::invalid("name", "Name the guard (120 characters)."))?;
                let action = input
                    .action
                    .map(GuardAction::key)
                    .map(str::to_owned)
                    .or_else(|| existing.map(|guard| guard.action.clone()))
                    .ok_or_else(|| {
                        OpError::invalid("action", "Say what it does: deny or review.")
                    })?;
                let operations = match input.operations {
                    Some(values) => words("operations", values, 50)?,
                    None => existing
                        .map(|guard| guard.operations.clone())
                        .unwrap_or_default(),
                };
                if let Some(unknown) = operations.iter().find(|name| {
                    crate::ops::registry().get(name).is_none() && !crate::mcp::has_tool(name)
                }) {
                    return Err(OpError::Invalid {
                        field: "operations".into(),
                        message: format!("There's no operation or agent tool called {unknown}.")
                            .into(),
                    });
                }
                let target_kinds = match input.target_kinds {
                    Some(values) => words("targetKinds", values, 20)?,
                    None => existing
                        .map(|guard| guard.target_kinds.clone())
                        .unwrap_or_default(),
                };
                let terms = match input.terms {
                    Some(values) => words("terms", values, 50)?,
                    None => existing
                        .map(|guard| guard.terms.clone())
                        .unwrap_or_default(),
                };
                let accounts = input
                    .accounts
                    .or_else(|| existing.map(|guard| guard.accounts.clone()))
                    .unwrap_or_default();
                let reviewers = input
                    .reviewers
                    .or_else(|| existing.map(|guard| guard.reviewers.clone()))
                    .unwrap_or_default();
                if accounts.len() > 50 || reviewers.len() > 20 {
                    return Err(OpError::invalid(
                        "accounts",
                        "Name at most 50 accounts and 20 reviewers.",
                    ));
                }
                if action == "review" && reviewers.is_empty() {
                    return Err(OpError::invalid(
                        "reviewers",
                        "A review guard needs someone to review: name them in reviewers.",
                    ));
                }
                // Everyone named is a member of this workspace.
                let named = accounts
                    .iter()
                    .chain(&reviewers)
                    .copied()
                    .collect::<Vec<_>>();
                let members = sqlx::query_scalar::<_, i64>(
                    "SELECT count(DISTINCT actor_id) FROM organization_memberships \
                     WHERE organization_id = $1 AND actor_id = ANY($2) AND enabled",
                )
                .bind(workspace_id)
                .bind(&named)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?;
                let mut distinct = named.clone();
                distinct.sort_unstable();
                distinct.dedup();
                if usize::try_from(members).unwrap_or(0) != distinct.len() {
                    return Err(OpError::invalid(
                        "accounts",
                        "Name people and agents of this workspace.",
                    ));
                }
                let enabled = input
                    .enabled
                    .or_else(|| existing.map(|guard| guard.enabled))
                    .unwrap_or(true);
                let id = existing.map_or_else(Uuid::now_v7, |guard| guard.id);
                sqlx::query(
                    r#"
                    INSERT INTO guards (
                        id, workspace_id, space_id, name, action, operations, target_kinds,
                        terms, accounts, reviewers, enabled, created_by_account_id,
                        updated_by_account_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name, action = EXCLUDED.action,
                        operations = EXCLUDED.operations, target_kinds = EXCLUDED.target_kinds,
                        terms = EXCLUDED.terms, accounts = EXCLUDED.accounts,
                        reviewers = EXCLUDED.reviewers, enabled = EXCLUDED.enabled,
                        updated_by_account_id = EXCLUDED.updated_by_account_id,
                        version = guards.version + 1, updated_at = now()
                    "#,
                )
                .bind(id)
                .bind(workspace_id)
                .bind(existing.map_or(space_id, |guard| guard.space_id))
                .bind(&name)
                .bind(&action)
                .bind(&operations)
                .bind(&target_kinds)
                .bind(&terms)
                .bind(&accounts)
                .bind(&reviewers)
                .bind(enabled)
                .bind(account)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
                Some(id)
            }
        };
        let space = crate::spaces::workspace::ensure(tx, workspace_id).await?;
        crate::outbox::enqueue_typed(
            tx,
            space,
            crate::outbox::OutboxEventPayload::GuardsChanged(crate::outbox::GuardsChangedPayload {
                workspace_id,
                project_id: space,
            }),
        )
        .await?;
        let guard = match id {
            Some(id) => Some(
                sqlx::query_as::<_, GuardView>(&format!("{SELECT} WHERE id = $1"))
                    .bind(id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(AppError::from)?,
            ),
            None => None,
        };
        let audited = id
            .or_else(|| existing.as_ref().map(|guard| guard.id))
            .unwrap_or(workspace_id);
        call.audit_target("guard", audited, json!({ "removed": input.remove }));
        Ok(GuardWriteOutput { guard })
    }
}

/// The workspace's guards.
pub struct GuardList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardListInput {}

impl Targeted for GuardListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuardListOutput {
    pub guards: Vec<GuardView>,
}

impl Operation for GuardList {
    const NAME: &'static str = "guard_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Guards";
    const DESCRIPTION: &'static str = "The workspace's guards: rules that stop changes or hold \
        them for review. Anyone in the workspace can read them.";

    type Input = GuardListInput;
    type Output = GuardListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: GuardListInput,
    ) -> Result<GuardListOutput, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let tx = call.tx().await?;
        Ok(GuardListOutput {
            guards: sqlx::query_as::<_, GuardView>(&format!(
                "{SELECT} WHERE workspace_id = $1 ORDER BY created_at, id LIMIT 200"
            ))
            .bind(workspace_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?,
        })
    }
}
