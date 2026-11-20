//! Themes as operations: `theme_list`, `theme_get`, `theme_create`,
//! `theme_update`, `theme_delete` and `workspace_theme_set` (offer a theme
//! to everyone in the workspace, or stop). The web app's Appearance page,
//! agents and apps run these.
//!
//! Anyone in a workspace makes themes of their own there. Only whoever made
//! a theme changes it; anyone else who wants a variation duplicates it. A
//! holder of the workspace's settings permission offers a theme to
//! everyone, withdraws one, and can delete an offered one.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{CONTRACT, ThemeDocument, ThemeSummary, ThemeView, invalid};
use crate::{
    access,
    error::AppError,
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, error::ConflictReason, refs::IdRef,
    },
    policy::Permission,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ThemeList>()
        .add::<ThemeGet>()
        .add::<ThemeCreate>()
        .add::<ThemeUpdate>()
        .add::<ThemeDelete>()
        .add::<WorkspaceThemeSet>();
}

const NOT_FOUND: &str = "That theme";

fn check_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > CONTRACT.name_limit
        || name.chars().any(char::is_control)
    {
        return Err(invalid(
            "name",
            format!(
                "Name it in one line of 1 to {} characters.",
                CONTRACT.name_limit
            ),
        ));
    }
    Ok(name.to_owned())
}

fn not_yours() -> OpError {
    OpError::Denied {
        reason: DenyReason::Rule,
        message: "Only whoever made this theme changes it. To make a variation, create your own \
            from its document (theme_get, then theme_create)."
            .into(),
    }
}

/// List themes.
pub struct ThemeList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeListInput {}

impl Targeted for ThemeListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeListOutput {
    /// Your own themes and the ones the workspace offers, by name.
    pub themes: Vec<ThemeSummary>,
    /// The built-in themes a theme's modes can start from, by id.
    pub built_in: Vec<String>,
    /// The theme contract's version: write documents with this format.
    pub format: u32,
    /// Whether you may offer themes to everyone in the workspace
    /// (workspace_theme_set), and withdraw or delete offered ones.
    pub can_offer: bool,
}

impl Operation for ThemeList {
    const NAME: &'static str = "theme_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List themes";
    const DESCRIPTION: &'static str = "List your own themes and the ones this workspace offers to \
        everyone, with the built-in themes a theme can start from. theme_get reads one whole.";

    type Input = ThemeListInput;
    type Output = ThemeListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: ThemeListInput,
    ) -> Result<ThemeListOutput, OpError> {
        let (workspace_id, viewer) = (call.ctx.workspace_id, call.account_id());
        let caller = call.ctx.caller.clone();
        let connection = &mut **call.tx().await?;
        let themes = super::list(&mut *connection, workspace_id, viewer).await?;
        let need = access::Need {
            permission: Some(Permission::SettingsManage),
            module: None,
            writes: true,
        };
        let can_offer = access::check(
            connection,
            &caller,
            need,
            access::Target::Workspace(workspace_id),
        )
        .await?
        .is_ok();
        Ok(ThemeListOutput {
            themes,
            built_in: CONTRACT.preset_ids(),
            format: CONTRACT.version,
            can_offer,
        })
    }
}

/// Read a theme.
pub struct ThemeGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeGetInput {
    /// The theme's id.
    pub id: IdRef,
}

impl Targeted for ThemeGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ThemeGet {
    const NAME: &'static str = "theme_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read theme";
    const DESCRIPTION: &'static str = "Read one theme whole: its light and dark modes (the \
        built-in theme each starts from, and the tokens it changes) and its CSS. Yours, or one \
        the workspace offers.";

    type Input = ThemeGetInput;
    type Output = ThemeView;

    async fn run(call: &mut OpCall<'_>, input: ThemeGetInput) -> Result<ThemeView, OpError> {
        let (workspace_id, viewer) = (call.ctx.workspace_id, call.account_id());
        let connection = &mut **call.tx().await?;
        super::visible(connection, workspace_id, viewer, input.id.id(), false)
            .await?
            .ok_or(OpError::not_found(NOT_FOUND))
    }
}

/// Make a theme.
pub struct ThemeCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeCreateInput {
    /// What it's called, in one line.
    pub name: String,
    /// The theme: its light and dark modes and optional CSS.
    pub document: ThemeDocument,
}

impl Targeted for ThemeCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ThemeCreate {
    const NAME: &'static str = "theme_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Make theme";
    const DESCRIPTION: &'static str = "Make a theme of your own: a light and a dark mode, each \
        starting from a built-in theme (theme_list names them) and changing any design tokens, \
        plus optional CSS for the published part names. Nothing in it may load from elsewhere \
        (no url(), @import or @font-face). Only you see it until the workspace offers it \
        (workspace_theme_set).";
    const EMITS: &'static [&'static str] = &["theme.changed"];

    type Input = ThemeCreateInput;
    type Output = ThemeView;

    async fn run(call: &mut OpCall<'_>, input: ThemeCreateInput) -> Result<ThemeView, OpError> {
        let name = check_name(&input.name)?;
        input.document.check()?;
        let (workspace_id, account) = (call.ctx.workspace_id, call.account_id());
        let id = Uuid::now_v7();
        let document = serde_json::to_value(&input.document).map_err(|_| OpError::Unavailable)?;
        let tx = call.tx().await?;
        sqlx::query(
            "INSERT INTO themes (id, workspace_id, owner_account_id, name, document) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(account)
        .bind(&name)
        .bind(&document)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        super::announce(tx, workspace_id, account, id, false).await?;
        let view = super::visible(tx, workspace_id, account, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        call.audit_target("theme", id, json!({}));
        Ok(view)
    }
}

/// Change a theme.
pub struct ThemeUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeUpdateInput {
    /// The theme's id.
    pub id: IdRef,
    /// The version you last read (theme_get or theme_list).
    pub expected_version: i64,
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The whole new document, replacing the old one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<ThemeDocument>,
}

impl Targeted for ThemeUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ThemeUpdate {
    const NAME: &'static str = "theme_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change theme";
    const DESCRIPTION: &'static str = "Rename one of your themes, or replace its document (pass \
        the whole document; theme_get gives the current one). Pass the version you last read as \
        expectedVersion. Only whoever made a theme changes it, offered or not.";
    const EMITS: &'static [&'static str] = &["theme.changed"];

    type Input = ThemeUpdateInput;
    type Output = ThemeView;

    async fn run(call: &mut OpCall<'_>, input: ThemeUpdateInput) -> Result<ThemeView, OpError> {
        let name = input.name.as_deref().map(check_name).transpose()?;
        if let Some(document) = &input.document {
            document.check()?;
        }
        let document = input
            .document
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| OpError::Unavailable)?;
        let (workspace_id, account, id) = (call.ctx.workspace_id, call.account_id(), input.id.id());
        let tx = call.tx().await?;
        let current = super::visible(tx, workspace_id, account, id, true)
            .await?
            .ok_or(OpError::not_found(NOT_FOUND))?;
        if !current.summary.mine {
            return Err(not_yours());
        }
        if current.summary.version != input.expected_version {
            return Err(OpError::Conflict {
                reason: ConflictReason::Version {
                    current_version: current.summary.version,
                },
                message: "The theme changed since you read it: read it again (theme_get).".into(),
                current: None,
            });
        }
        sqlx::query(
            "UPDATE themes SET name = COALESCE($2, name), document = COALESCE($3, document), \
             version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&name)
        .bind(&document)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        super::announce(tx, workspace_id, account, id, current.summary.offered).await?;
        let view = super::visible(tx, workspace_id, account, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        call.audit_target(
            "theme",
            id,
            json!({ "renamed": name.is_some(), "document": document.is_some() }),
        );
        Ok(view)
    }
}

/// Delete a theme.
pub struct ThemeDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeDeleteInput {
    /// The theme's id.
    pub id: IdRef,
}

impl Targeted for ThemeDeleteInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDeleted {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub deleted: bool,
}

impl Operation for ThemeDelete {
    const NAME: &'static str = "theme_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Delete theme";
    const DESCRIPTION: &'static str = "Delete one of your themes. Someone who manages the \
        workspace's settings can also delete a theme the workspace offers. Anyone using it goes \
        back to the default look.";
    const EMITS: &'static [&'static str] = &["theme.changed"];

    type Input = ThemeDeleteInput;
    type Output = ThemeDeleted;

    async fn run(call: &mut OpCall<'_>, input: ThemeDeleteInput) -> Result<ThemeDeleted, OpError> {
        let (workspace_id, account, id) = (call.ctx.workspace_id, call.account_id(), input.id.id());
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let current = super::visible(tx, workspace_id, account, id, true)
            .await?
            .ok_or(OpError::not_found(NOT_FOUND))?;
        if !current.summary.mine {
            let need = access::Need {
                permission: Some(Permission::SettingsManage),
                module: None,
                writes: true,
            };
            if access::check_locked(tx, &caller, need, access::Target::Workspace(workspace_id))
                .await?
                .is_err()
            {
                return Err(OpError::Denied {
                    reason: DenyReason::Rule,
                    message: "Only whoever made this theme, or someone who manages the \
                        workspace's settings, deletes it."
                        .into(),
                });
            }
        }
        super::announce(
            tx,
            workspace_id,
            current.summary.owner_account_id,
            id,
            current.summary.offered,
        )
        .await?;
        sqlx::query("DELETE FROM themes WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        call.audit_target("theme", id, json!({}));
        Ok(ThemeDeleted { id, deleted: true })
    }
}

/// Offer a theme to the workspace, or stop.
pub struct WorkspaceThemeSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceThemeSetInput {
    /// The theme's id: one of yours, or one the workspace offers.
    pub theme_id: IdRef,
    /// True offers it to everyone in the workspace; false stops (it stays
    /// its maker's own).
    pub offered: bool,
}

impl Targeted for WorkspaceThemeSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceThemeSet {
    const NAME: &'static str = "workspace_theme_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Offer theme to the workspace";
    const DESCRIPTION: &'static str = "Offer one of your themes to everyone in this workspace \
        (offered: true), so anyone can pick it in Appearance, or stop offering a theme \
        (offered: false), which leaves it its maker's own.";
    const EMITS: &'static [&'static str] = &["theme.changed"];

    type Input = WorkspaceThemeSetInput;
    type Output = ThemeView;

    async fn run(
        call: &mut OpCall<'_>,
        input: WorkspaceThemeSetInput,
    ) -> Result<ThemeView, OpError> {
        let (workspace_id, account, id) = (
            call.ctx.workspace_id,
            call.account_id(),
            input.theme_id.id(),
        );
        let tx = call.tx().await?;
        let current = super::visible(tx, workspace_id, account, id, true)
            .await?
            .ok_or(OpError::not_found(NOT_FOUND))?;
        if current.summary.offered != input.offered {
            sqlx::query(
                "UPDATE themes SET offered = $2, offered_by_account_id = CASE WHEN $2 THEN $3 END, \
                 version = version + 1, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(input.offered)
            .bind(account)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
            super::announce(tx, workspace_id, current.summary.owner_account_id, id, true).await?;
        }
        // Read back whether or not it is still offered: withdrawing someone
        // else's theme leaves it theirs alone.
        let view = super::by_id(tx, workspace_id, account, id)
            .await?
            .ok_or(OpError::Unavailable)?;
        call.audit_target("theme", id, json!({ "offered": input.offered }));
        Ok(view)
    }
}
