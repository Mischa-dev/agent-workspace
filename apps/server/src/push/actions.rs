//! W18: acting straight from a push notification. An approval request gets
//! Approve and Deny, an agent's question and a mention get Reply.
//!
//! The service worker can't rely on the session cookie or the CSRF token, so
//! each push carries one random token for its notification (only its sha256
//! is kept). The token names who acts and what they may do; everything else
//! goes through the app's own commands, which recheck the person's current
//! membership, the conversation's visibility and the approval's policy when
//! they act (principle 9). After one action the token only answers a retry of
//! that same action, and it expires with the push itself.

use axum::{Json, extract::State, http::HeaderMap};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    api::{ApprovalResponse, RunActor, answer_approval, sponsored_execution_request},
    auth::{random_token, require_allowed_origin},
    conversations::{MessageCommandActor, append_message_command},
    error::{ApiJson, AppError},
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    notifications::{self, NotificationView, Selection, UpdateAction},
    policy::ActorKind,
};

/// As long as a push service keeps an undelivered push, so a button works
/// whenever its notification can still show up.
const TOKEN_LIFETIME_HOURS: i64 = 24;
const REPLY_CHARACTERS: usize = 4_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationAction {
    Approve,
    Deny,
    Reply,
    /// A reminder: mark its saved item done (W21).
    Done,
    /// A reminder: remind again in an hour (W21).
    Snooze,
}

impl NotificationAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Reply => "reply",
            Self::Done => "done",
            Self::Snooze => "snooze",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            "reply" => Some(Self::Reply),
            "done" => Some(Self::Done),
            "snooze" => Some(Self::Snooze),
            _ => None,
        }
    }
}

/// What a notification offers on the phone, before any access check: the
/// command that runs checks access when someone acts.
pub fn offered(notification: &NotificationView) -> Vec<NotificationAction> {
    match notification.kind.as_str() {
        notifications::kind::APPROVAL_REQUESTED => match &notification.approval {
            Some(approval) if approval.status == "pending" && approval.kind == "approval" => {
                vec![NotificationAction::Approve, NotificationAction::Deny]
            }
            Some(approval) if approval.status == "pending" && approval.kind == "question" => {
                vec![NotificationAction::Reply]
            }
            _ => Vec::new(),
        },
        notifications::kind::MENTION | notifications::kind::CONVERSATION_MESSAGE
            if notification.reply_conversation_id.is_some() =>
        {
            vec![NotificationAction::Reply]
        }
        notifications::kind::REMINDER if notification.saved_item_id.is_some() => {
            vec![NotificationAction::Done, NotificationAction::Snooze]
        }
        _ => Vec::new(),
    }
}

/// A token for a notification's actions, as the push carries it.
#[derive(Clone, Debug)]
pub struct MintedActions {
    pub token: String,
    pub actions: Vec<NotificationAction>,
    pub expires_at: DateTime<Utc>,
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Make a token for `recipient` to act on `notification` from its push, if
/// it offers any action. Sponsored executions are answered only in the app.
pub async fn mint(
    pool: &PgPool,
    organization_id: Uuid,
    recipient: Uuid,
    notification: &NotificationView,
) -> Result<Option<MintedActions>, AppError> {
    let actions = offered(notification);
    if actions.is_empty() {
        return Ok(None);
    }
    if let Some(approval) = &notification.approval {
        let sponsored = sqlx::query_scalar::<_, bool>(
            "SELECT external_execution_request_id IS NOT NULL FROM agent_approvals WHERE id = $1",
        )
        .bind(approval.id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(true);
        if sponsored {
            return Ok(None);
        }
    }
    let token = random_token();
    let expires_at = Utc::now() + Duration::hours(TOKEN_LIFETIME_HOURS);
    sqlx::query(
        r#"
        INSERT INTO notification_action_tokens (
            id, organization_id, actor_id, notification_id, token_hash, actions, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(recipient)
    .bind(notification.id)
    .bind(token_hash(&token))
    .bind(
        actions
            .iter()
            .map(|action| action.as_str().to_owned())
            .collect::<Vec<_>>(),
    )
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(Some(MintedActions {
        token,
        actions,
        expires_at,
    }))
}

/// The buttons the service worker shows, Approve and Deny or Reply. A
/// Reply is an inline text field where the platform has one; elsewhere the
/// worker opens the composer instead.
pub fn payload_actions(notification: &NotificationView, minted: &MintedActions) -> Vec<Value> {
    let question = notification
        .approval
        .as_ref()
        .is_some_and(|approval| approval.kind == "question");
    minted
        .actions
        .iter()
        .map(|action| match action {
            NotificationAction::Approve => json!({"action": "approve", "title": "Approve"}),
            NotificationAction::Done => json!({"action": "done", "title": "Done"}),
            NotificationAction::Snooze => json!({
                "action": "snooze",
                "title": format!("In {} min", crate::later::SNOOZE_MINUTES),
            }),
            NotificationAction::Deny => json!({"action": "deny", "title": "Deny"}),
            NotificationAction::Reply => json!({
                "action": "reply",
                "title": if question { "Answer" } else { "Reply" },
                "type": "text",
                "placeholder": if question { "Your answer" } else { "Reply" },
            }),
        })
        .collect()
}

/// Remove tokens a day after they expire.
pub async fn prune(pool: &PgPool) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "DELETE FROM notification_action_tokens WHERE expires_at < now() - interval '1 day'",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

// ---------------------------------------------------------------------------
// Acting

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActRequest {
    token: String,
    action: String,
    /// The reply, for Reply.
    text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActResponse {
    /// approved, denied, answered, replied, done or snoozed
    outcome: &'static str,
    /// Where to open the app to see it.
    href: String,
}

#[derive(FromRow)]
struct TokenRow {
    id: Uuid,
    actor_id: Uuid,
    notification_id: Uuid,
    actions: Vec<String>,
    used_action: Option<String>,
}

/// The person the token acts as, while their account is active.
struct TokenActor {
    actor_id: Uuid,
}

impl RunActor for TokenActor {
    fn actor_id(&self) -> Uuid {
        self.actor_id
    }
    fn actor_kind(&self) -> Result<ActorKind, AppError> {
        Ok(ActorKind::Human)
    }
}

pub(crate) async fn act_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ActRequest>,
) -> Result<Json<ActResponse>, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    act(&state, request).await.map(Json)
}

async fn act(state: &AppState, request: ActRequest) -> Result<ActResponse, AppError> {
    let action =
        NotificationAction::parse(&request.action).ok_or(AppError::Validation("action"))?;
    if request.token.is_empty() || request.token.len() > 128 {
        return Err(AppError::NotFound("notification_action"));
    }
    if URL_SAFE_NO_PAD.decode(&request.token).is_err() {
        return Err(AppError::NotFound("notification_action"));
    }
    // A token belongs to a person whose account is still active; a removed
    // or disabled person's tokens act as nobody.
    let token = sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT t.id, t.actor_id, t.notification_id, t.actions, t.used_action
        FROM notification_action_tokens t
        JOIN actors a ON a.id = t.actor_id AND a.status = 'active' AND a.kind = 'human'
        JOIN users u ON u.actor_id = t.actor_id AND u.status = 'active'
        JOIN organization_memberships om
          ON om.organization_id = t.organization_id AND om.actor_id = t.actor_id AND om.enabled
        WHERE t.token_hash = $1 AND t.expires_at > now()
        "#,
    )
    .bind(token_hash(&request.token))
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("notification_action"))?;
    state.rate_limits.check_authenticated_write(token.id)?;
    if !token
        .actions
        .iter()
        .any(|allowed| allowed == action.as_str())
    {
        return Err(AppError::Validation("action"));
    }
    if token
        .used_action
        .as_deref()
        .is_some_and(|used| used != action.as_str())
    {
        return Err(AppError::Conflict(
            "notification_action",
            "this notification was already answered",
        ));
    }
    let actor = TokenActor {
        actor_id: token.actor_id,
    };
    // Read the notification as its recipient: one they can no longer see
    // can't be acted on.
    let notification =
        notifications::get_many(&state.pool, actor.actor_id, &[token.notification_id])
            .await?
            .into_iter()
            .next()
            .ok_or(AppError::NotFound("notification_action"))?;
    // Every retry of the same button reuses one key, so a send that timed
    // out on the phone can't post or answer twice.
    let key = IdempotencyKey::parse(&format!(
        "notification-action-{}-{}",
        token.id,
        action.as_str()
    ))?;
    let outcome = match action {
        NotificationAction::Approve | NotificationAction::Deny => {
            let approval = notification
                .approval
                .as_ref()
                .filter(|approval| approval.kind == "approval")
                .ok_or(AppError::NotFound("notification_action"))?;
            if sponsored_execution_request(state, approval.id)
                .await?
                .is_some()
            {
                return Err(AppError::NotFound("notification_action"));
            }
            let decision = if action == NotificationAction::Approve {
                "approve"
            } else {
                "deny"
            };
            answer_approval(
                state,
                &actor,
                approval.id,
                ApprovalResponse {
                    decision: Some(decision.to_owned()),
                    answer: None,
                },
                key,
            )
            .await?;
            if action == NotificationAction::Approve {
                "approved"
            } else {
                "denied"
            }
        }
        NotificationAction::Done | NotificationAction::Snooze => {
            let saved_item_id = notification
                .saved_item_id
                .ok_or(AppError::NotFound("notification_action"))?;
            if action == NotificationAction::Done {
                crate::later::change(
                    state,
                    actor.actor_id,
                    saved_item_id,
                    crate::later::ChangeRequest {
                        done: Some(true),
                        ..crate::later::ChangeRequest::default()
                    },
                )
                .await?;
                "done"
            } else {
                crate::later::snooze(state, actor.actor_id, saved_item_id).await?;
                "snoozed"
            }
        }
        NotificationAction::Reply => {
            let text = request
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty() && text.chars().count() <= REPLY_CHARACTERS)
                .ok_or(AppError::Validation("text"))?;
            match &notification.approval {
                Some(approval) if approval.kind == "question" => {
                    if sponsored_execution_request(state, approval.id)
                        .await?
                        .is_some()
                    {
                        return Err(AppError::NotFound("notification_action"));
                    }
                    answer_approval(
                        state,
                        &actor,
                        approval.id,
                        ApprovalResponse {
                            decision: None,
                            answer: Some(text.to_owned()),
                        },
                        key,
                    )
                    .await?;
                    "answered"
                }
                _ => {
                    let conversation_id = notification
                        .reply_conversation_id
                        .ok_or(AppError::NotFound("notification_action"))?;
                    let content = MessageContent::from_plain_text(text)
                        .map_err(|_| AppError::Validation("text"))?;
                    let (content, _, _) = content.into_parts();
                    append_message_command(
                        state,
                        MessageCommandActor {
                            actor_id: actor.actor_id,
                            actor_kind: "human".to_owned(),
                            mcp_credential: None,
                            via_grant_id: None,
                        },
                        conversation_id,
                        None,
                        key,
                        serde_json::to_value(content)
                            .map_err(|_| AppError::Internal("reply content"))?,
                        None,
                    )
                    .await?;
                    "replied"
                }
            }
        }
    };
    sqlx::query(
        r#"
        UPDATE notification_action_tokens
        SET used_action = $2, used_at = COALESCE(used_at, now())
        WHERE id = $1
        "#,
    )
    .bind(token.id)
    .bind(action.as_str())
    .execute(&state.pool)
    .await?;
    // Acting on it is reading it.
    notifications::update(
        &state.pool,
        actor.actor_id,
        None,
        &Selection::Ids(vec![notification.id]),
        UpdateAction::Read,
        None,
    )
    .await?;
    Ok(ActResponse {
        outcome,
        href: notification.href,
    })
}
