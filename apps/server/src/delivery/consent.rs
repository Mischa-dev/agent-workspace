//! A2.15: consent for apps that read conversations. An account that reads
//! with consent (apps acting as themselves) hears a DM, a group or a
//! private channel only where one of its participants said it may
//! (`app_may_read`, applied by `delivery_visible`). `app_consent_set` is how
//! a participant says so, or takes it back. A shared channel needs none:
//! whoever reads its space hears it.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
    outbox::{ConversationUpdatedPayload, OutboxEventPayload},
};

/// Let an app hear a private conversation, or stop it.
pub struct AppConsentSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConsentSetInput {
    /// The DM, group or private channel (or a thread in one), or a link to
    /// it. Consent covers the whole conversation and its threads.
    pub conversation: IdRef,
    /// The app's account, or a link to it.
    pub app: IdRef,
    /// false takes consent back. Left out: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted: Option<bool>,
}

impl Targeted for AppConsentSetInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppConsentView {
    /// The conversation consent is held on (a thread's DM, group or
    /// channel).
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    #[schemars(with = "String")]
    pub app_account_id: Uuid,
    /// Whether the app hears this conversation now.
    pub granted: bool,
    /// Whether this call changed anything.
    pub changed: bool,
}

impl Operation for AppConsentSet {
    const NAME: &'static str = "app_consent_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Let an app hear a conversation";
    const DESCRIPTION: &'static str = "Let an app that acts as itself hear a DM, group or \
        private channel you are in, or stop it (granted: false). Without consent it hears \
        nothing from there, even as a member. A shared channel needs none.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = AppConsentSetInput;
    type Output = AppConsentView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AppConsentSetInput,
    ) -> Result<AppConsentView, OpError> {
        let caller = call.account_id();
        let app = input.app.id();
        let granted = input.granted.unwrap_or(true);
        let tx = call.tx().await?;
        let place = sqlx::query_as::<_, (Uuid, bool, Uuid, Option<Uuid>, i64)>(
            r#"
            SELECT root.root_id, root.needs_consent, conversation.workspace_id,
                   conversation.project_id, conversation.version
            FROM app_consent_root($1) root
            JOIN conversations conversation ON conversation.id = root.root_id
            WHERE conversation_visible_to(conversation.id, $2)
            "#,
        )
        .bind(input.conversation.id())
        .bind(caller)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let Some((conversation_id, needs_consent, workspace_id, space_id, version)) = place else {
            return Err(OpError::not_found("That conversation"));
        };
        if !needs_consent {
            return Err(OpError::invalid(
                "conversation",
                "A shared channel needs no consent: whoever reads its space hears it.",
            ));
        }
        let reads_with_consent = sqlx::query_scalar::<_, bool>(
            "SELECT reads_with_consent FROM actors WHERE id = $1 AND status = 'active'",
        )
        .bind(app)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if reads_with_consent != Some(true) {
            return Err(OpError::invalid(
                "app",
                "That account hears what it can see without consent: consent is for apps \
                 that act as themselves.",
            ));
        }
        let changed = if granted {
            sqlx::query(
                r#"
                INSERT INTO conversation_app_consents (
                    workspace_id, space_id, conversation_id, app_account_id,
                    granted_by_account_id
                )
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (conversation_id, app_account_id) DO UPDATE SET
                    granted_by_account_id = $5, granted_at = now(),
                    revoked_by_account_id = NULL, revoked_at = NULL
                WHERE conversation_app_consents.revoked_at IS NOT NULL
                "#,
            )
            .bind(workspace_id)
            .bind(space_id)
            .bind(conversation_id)
            .bind(app)
            .bind(caller)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?
            .rows_affected()
                > 0
        } else {
            sqlx::query(
                "UPDATE conversation_app_consents \
                 SET revoked_by_account_id = $3, revoked_at = now() \
                 WHERE conversation_id = $1 AND app_account_id = $2 AND revoked_at IS NULL",
            )
            .bind(conversation_id)
            .bind(app)
            .bind(caller)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?
            .rows_affected()
                > 0
        };
        if changed {
            // Open copies of the conversation read who may hear it again.
            crate::outbox::enqueue_typed(
                tx,
                space_id.unwrap_or(crate::spaces::DIRECT),
                OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
                    conversation_id,
                    version,
                }),
            )
            .await?;
        }
        call.audit_target(
            "conversation",
            conversation_id,
            json!({ "app": app, "granted": granted }),
        );
        Ok(AppConsentView {
            conversation_id,
            app_account_id: app,
            granted,
            changed,
        })
    }
}
