//! Custom emoji as operations (C2.19): `custom_emoji_create`,
//! `custom_emoji_list` and `custom_emoji_update` (retire or bring back).
//! The web's picker and settings, agents and apps run these.

use base64::Engine as _;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{CustomEmojiView, MAX_IMAGE_BYTES, MAX_LISTED, VIEW_COLUMNS, image_type};
use crate::{
    access,
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        error::ConflictReason, refs::IdRef,
    },
    policy::Permission,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<CustomEmojiCreate>()
        .add::<CustomEmojiList>()
        .add::<CustomEmojiUpdate>();
}

/// Add a custom emoji.
pub struct CustomEmojiCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomEmojiCreateInput {
    /// 2 to 32 lowercase letters, digits and underscores; used as :name:.
    pub name: String,
    /// The image: a PNG, GIF or WebP of at most 64 KB, base64-encoded.
    pub image_base64: String,
}

impl Targeted for CustomEmojiCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for CustomEmojiCreate {
    const NAME: &'static str = "custom_emoji_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Add custom emoji";
    const DESCRIPTION: &'static str = "Add one of this workspace's own emoji: a small PNG, GIF or \
        WebP (base64, at most 64 KB) under a name people react with as :name:.";
    const EMITS: &'static [&'static str] = &["custom_emoji.changed"];

    type Input = CustomEmojiCreateInput;
    type Output = CustomEmojiView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CustomEmojiCreateInput,
    ) -> Result<CustomEmojiView, OpError> {
        let name = input.name.trim().trim_matches(':').to_lowercase();
        if !(2..=32).contains(&name.len())
            || !name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(OpError::invalid(
                "name",
                "Use 2 to 32 lowercase letters, digits and underscores, such as party_parrot.",
            ));
        }
        if crate::reactions::resolve(&name).is_some() {
            return Err(OpError::invalid(
                "name",
                "That's the name of a standard emoji: pick another.",
            ));
        }
        if input.image_base64.len() > MAX_IMAGE_BYTES * 4 / 3 + 8 {
            return Err(OpError::TooLarge {
                limit_bytes: MAX_IMAGE_BYTES,
            });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input.image_base64.trim())
            .map_err(|_| OpError::invalid("imageBase64", "Pass the image as base64."))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(OpError::TooLarge {
                limit_bytes: MAX_IMAGE_BYTES,
            });
        }
        let content_type = image_type(&bytes)
            .ok_or_else(|| OpError::invalid("imageBase64", "Use a PNG, GIF or WebP image."))?;
        let account = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        let view = sqlx::query_as::<_, CustomEmojiView>(&format!(
            r#"
            INSERT INTO custom_emojis (id, workspace_id, name, content_type, image, created_by_account_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT DO NOTHING
            RETURNING {VIEW_COLUMNS}
            "#
        ))
        .bind(id)
        .bind(workspace_id)
        .bind(&name)
        .bind(content_type)
        .bind(&bytes)
        .bind(account)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or(OpError::Conflict {
            reason: ConflictReason::State { code: "name_taken" },
            message: "This workspace already has an emoji by that name.".into(),
            current: None,
        })?;
        super::announce(tx, workspace_id, id).await?;
        call.audit_target("custom_emoji", id, json!({ "name": name }));
        Ok(view)
    }
}

/// List custom emoji.
pub struct CustomEmojiList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomEmojiListInput {
    /// Include retired ones (they still show on old reactions).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_retired: bool,
}

impl Targeted for CustomEmojiListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomEmojiListOutput {
    /// By name.
    pub emoji: Vec<CustomEmojiView>,
}

impl Operation for CustomEmojiList {
    const NAME: &'static str = "custom_emoji_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List custom emoji";
    const DESCRIPTION: &'static str = "List this workspace's own emoji, by name: react with any of \
        them as :name: (message_react).";

    type Input = CustomEmojiListInput;
    type Output = CustomEmojiListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: CustomEmojiListInput,
    ) -> Result<CustomEmojiListOutput, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let connection = &mut **call.tx().await?;
        let emoji = sqlx::query_as::<_, CustomEmojiView>(&format!(
            r#"
            SELECT {VIEW_COLUMNS}
            FROM custom_emojis
            WHERE workspace_id = $1 AND ($2 OR retired_at IS NULL)
            ORDER BY name, created_at DESC
            LIMIT $3
            "#
        ))
        .bind(workspace_id)
        .bind(input.include_retired)
        .bind(MAX_LISTED)
        .fetch_all(&mut *connection)
        .await
        .map_err(AppError::from)?;
        Ok(CustomEmojiListOutput { emoji })
    }
}

/// Retire a custom emoji, or bring it back.
pub struct CustomEmojiUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomEmojiUpdateInput {
    /// The emoji's id.
    pub id: IdRef,
    /// True to retire it (out of the picker, still on old reactions);
    /// false to bring it back.
    pub retired: bool,
}

impl Targeted for CustomEmojiUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for CustomEmojiUpdate {
    const NAME: &'static str = "custom_emoji_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Retire custom emoji";
    const DESCRIPTION: &'static str = "Retire one of this workspace's own emoji (retired: true), so \
        it leaves the picker but stays on the reactions that use it, or bring it back. Whoever \
        added it, or someone who manages the workspace's settings, can.";
    const EMITS: &'static [&'static str] = &["custom_emoji.changed"];

    type Input = CustomEmojiUpdateInput;
    type Output = CustomEmojiView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CustomEmojiUpdateInput,
    ) -> Result<CustomEmojiView, OpError> {
        let account = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let id = input.id.id();
        let tx = call.tx().await?;
        let added_by = sqlx::query_scalar::<_, Uuid>(
            "SELECT created_by_account_id FROM custom_emojis WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or(OpError::not_found("That emoji"))?;
        if added_by != account {
            let need = access::Need {
                permission: Some(Permission::SettingsManage),
                module: None,
                writes: true,
            };
            if access::check_locked(tx, &caller, need, access::Target::Workspace(workspace_id))
                .await?
                .is_err()
            {
                return Err(OpError::invalid(
                    "id",
                    "Only whoever added this emoji, or someone who manages the workspace's settings, can change it.",
                ));
            }
        }
        let view = sqlx::query_as::<_, CustomEmojiView>(&format!(
            r#"
            UPDATE custom_emojis
            SET retired_at = CASE WHEN $3 THEN COALESCE(retired_at, now()) END,
                retired_by_account_id = CASE WHEN $3 THEN COALESCE(retired_by_account_id, $4) END
            WHERE id = $1 AND workspace_id = $2
            RETURNING {VIEW_COLUMNS}
            "#
        ))
        .bind(id)
        .bind(workspace_id)
        .bind(input.retired)
        .bind(account)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| match &error {
            sqlx::Error::Database(database)
                if database.constraint() == Some("custom_emojis_live_name") =>
            {
                OpError::Conflict {
                    reason: ConflictReason::State { code: "name_taken" },
                    message: "Another emoji has that name now: retire it first.".into(),
                    current: None,
                }
            }
            _ => AppError::from(error).into(),
        })?;
        super::announce(tx, workspace_id, id).await?;
        call.audit_target("custom_emoji", id, json!({ "retired": input.retired }));
        Ok(view)
    }
}
