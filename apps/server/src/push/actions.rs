//! W18: acting straight from a push notification. A request offering
//! approve and deny gets Approve and Deny, one that takes words and a
//! mention get Reply.
//!
//! The service worker can't rely on the session cookie or the CSRF token, so
//! each push carries one random token for its notification (only its sha256
//! is kept). The token names who acts and what they may do; everything else
//! goes through the app's own commands, which recheck the person's current
//! membership, the conversation's visibility and who may answer when
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
    auth::{random_token, require_allowed_origin},
    conversations::{MessageCommandActor, append_message_command},
    error::{ApiJson, AppError},
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    notifications::{self, NotificationView, Selection, UpdateAction},
    policy::ActorKind,
    requests::{AnswerError, RequestCard},
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
    /// A mention or message: read the conversation (C2.2), as opening it
    /// would.
    Read,
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
            Self::Read => "read",
            Self::Done => "done",
            Self::Snooze => "snooze",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            "reply" => Some(Self::Reply),
            "read" => Some(Self::Read),
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
        notifications::kind::REQUEST_RAISED => match &notification.request {
            Some(request) if request.status == "open" => {
                let mut actions = Vec::new();
                if styled(request, "approve").is_some() && styled(request, "deny").is_some() {
                    actions.extend([NotificationAction::Approve, NotificationAction::Deny]);
                }
                if request.allow_text {
                    actions.push(NotificationAction::Reply);
                }
                actions
            }
            _ => Vec::new(),
        },
        notifications::kind::MENTION | notifications::kind::CONVERSATION_MESSAGE
            if notification.reply_conversation_id.is_some() =>
        {
            vec![NotificationAction::Reply, NotificationAction::Read]
        }
        notifications::kind::REMINDER if notification.saved_item_id.is_some() => {
            vec![NotificationAction::Done, NotificationAction::Snooze]
        }
        _ => Vec::new(),
    }
}

/// The option of a request styled (or, unstyled, named) approve or deny.
fn styled<'a>(request: &'a RequestCard, style: &str) -> Option<&'a str> {
    request
        .options
        .iter()
        .find(|option| match option.style.as_deref() {
            Some(found) => found == style,
            None => option.id == style,
        })
        .map(|option| option.id.as_str())
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
/// it offers any action. A run in someone's name is approved only in the
/// app, where its sponsor sees what it will do.
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
    if let Some(request) = &notification.request {
        let sponsored = crate::db::scoped_one(pool, async |connection| {
            sqlx::query_scalar::<_, bool>(
                "SELECT subject_kind IS NOT DISTINCT FROM $2 FROM requests WHERE id = $1",
            )
            .bind(request.id)
            .bind(crate::external_execution::REQUEST_SUBJECT)
            .fetch_optional(connection)
            .await
        })
        .await?
        .unwrap_or(true);
        if sponsored {
            return Ok(None);
        }
    }
    let token = random_token();
    let expires_at = Utc::now() + Duration::hours(TOKEN_LIFETIME_HOURS);
    crate::db::scoped_one(pool, async |connection| {
        sqlx::query(
            r#"
        INSERT INTO notification_action_tokens (
            id, organization_id, actor_id, notification_id, token_hash, actions, expires_at,
            request_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
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
        .bind(notification.request.as_ref().map(|request| request.id))
        .execute(connection)
        .await
    })
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
    let question = notification.request.is_some();
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
            NotificationAction::Read => json!({"action": "read", "title": "Mark read"}),
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
    Ok(crate::db::scoped_one(pool, async |connection| {
        sqlx::query(
            "DELETE FROM notification_action_tokens WHERE expires_at < now() - interval '1 day'",
        )
        .execute(connection)
        .await
    })
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
    /// The places that need the person now, for the app's badge.
    badge: i64,
}

#[derive(FromRow)]
struct TokenRow {
    id: Uuid,
    actor_id: Uuid,
    notification_id: Uuid,
    request_id: Option<Uuid>,
    actions: Vec<String>,
    used_action: Option<String>,
}

/// The person the token acts as, while their account is active.
struct TokenActor {
    actor_id: Uuid,
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
    let token = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, TokenRow>(
            r#"
        SELECT t.id, t.actor_id, t.notification_id, t.request_id, t.actions, t.used_action
        FROM notification_action_tokens t
        JOIN actors a ON a.id = t.actor_id AND a.status = 'active' AND a.kind = 'human' -- // kind-check: mechanical (push actions answer from a person's device)
        JOIN users u ON u.actor_id = t.actor_id AND u.status = 'active'
        JOIN organization_memberships om
          ON om.organization_id = t.organization_id AND om.actor_id = t.actor_id AND om.enabled
        WHERE t.token_hash = $1 AND t.expires_at > now()
        "#,
        )
        .bind(token_hash(&request.token))
        .fetch_optional(connection)
        .await
    })
    .await?
    .ok_or(AppError::NotFound("notification_action"))?;
    state
        .rate_limits
        .check_authenticated_write(token.id)
        .await?;
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
    let notification = notifications::get_many(
        &state.pool,
        crate::addresses::Addresses::of(state),
        actor.actor_id,
        &[token.notification_id],
    )
    .await?
    .into_iter()
    .next()
    .ok_or(AppError::NotFound("notification_action"))?;
    // A run's waiting notifications are grouped, so the row may now show
    // another request: a button only answers the one it was shown with.
    let shown_request = notification
        .request
        .as_ref()
        .filter(|request| Some(request.id) == token.request_id);
    // A retry of the button already used gives the same answer again.
    let retried = token.used_action.as_deref() == Some(action.as_str());
    // Every retry of the same button reuses one key, so a send that timed
    // out on the phone can't post twice.
    let key = IdempotencyKey::parse(&format!(
        "notification-action-{}-{}",
        token.id,
        action.as_str()
    ))?;
    let outcome = match action {
        NotificationAction::Approve | NotificationAction::Deny => {
            let request = shown_request.ok_or(AppError::NotFound("notification_action"))?;
            let style = if action == NotificationAction::Approve {
                "approve"
            } else {
                "deny"
            };
            let option = styled(request, style).ok_or(AppError::NotFound("notification_action"))?;
            if !retried {
                answer_request(state, &token, action, request.id, Some(option), None).await?;
            }
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
        NotificationAction::Read => {
            let conversation_id = notification
                .reply_conversation_id
                .ok_or(AppError::NotFound("notification_action"))?;
            read_conversation(state, actor.actor_id, conversation_id).await?;
            "read"
        }
        NotificationAction::Reply => {
            let text = request
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty() && text.chars().count() <= REPLY_CHARACTERS)
                .ok_or(AppError::Validation("text"))?;
            if notification.request.is_some() && shown_request.is_none() {
                return Err(AppError::NotFound("notification_action"));
            }
            match shown_request {
                Some(request) => {
                    if !request.allow_text {
                        return Err(AppError::NotFound("notification_action"));
                    }
                    if !retried {
                        answer_request(state, &token, action, request.id, None, Some(text)).await?;
                    }
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
                            run_id: None,
                        },
                        conversation_id,
                        None,
                        key,
                        serde_json::to_value(content)
                            .map_err(|_| AppError::Internal("reply content"))?,
                        None,
                    )
                    .await?;
                    // C2.34: answering from the phone leaves the
                    // conversation read, up to and including the reply.
                    read_conversation(state, actor.actor_id, conversation_id).await?;
                    "replied"
                }
            }
        }
    };
    crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query(
            r#"
        UPDATE notification_action_tokens
        SET used_action = $2, used_at = COALESCE(used_at, now())
        WHERE id = $1
        "#,
        )
        .bind(token.id)
        .bind(action.as_str())
        .execute(connection)
        .await
    })
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
    // The app's badge as it stands now, for the phone to show (C2.34).
    let badge = notifications::attentions(&state.pool, actor.actor_id)
        .await?
        .needs_you_conversations;
    Ok(ActResponse {
        outcome,
        href: notification.href,
        badge,
    })
}

/// The same read as opening the conversation (C2.2), rechecked now: one they
/// can no longer read reads nothing.
async fn read_conversation(
    state: &AppState,
    actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let conversation = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT conversation.project_id, conversation.next_entry_sequence - 1
        FROM conversations conversation
        WHERE conversation.id = $1
          AND actor_can_access_project($2, NULL, conversation.project_id)
          AND conversation_readable_by(conversation.id, $2)
        "#,
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("notification_action"))?;
    crate::read_state::mark_seen(
        &mut transaction,
        conversation.0,
        conversation_id,
        actor_id,
        conversation.1,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Answer a request as the token's person, with the same checks as
/// `request_respond`: they must still see it and be one it asks. The token
/// is marked used in the same transaction, so a retry after the answer
/// committed is recognised as that answer, never refused as a second one.
async fn answer_request(
    state: &AppState,
    token: &TokenRow,
    action: NotificationAction,
    request_id: Uuid,
    option_id: Option<&str>,
    text: Option<&str>,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    // An answer can set a run going again: a suspended or closing workspace
    // answers nothing, held before the request's row is locked, as
    // `request_respond` does.
    if let Some(space) = crate::requests::space_of(&mut *transaction, request_id).await? {
        crate::workspace::lifecycle::hold_open_for_space(
            &mut transaction,
            space,
            "request_respond",
        )
        .await?;
    }
    let actor_id = token.actor_id;
    let row = crate::requests::visible(&mut transaction, request_id, actor_id, true)
        .await?
        .ok_or(AppError::NotFound("notification_action"))?;
    let caller = crate::access::Caller::person(actor_id, ActorKind::Human); // kind-check: mechanical (push tokens are minted for people's devices)
    crate::requests::answer(&mut transaction, state, &caller, &row, option_id, text)
        .await
        .map_err(|error| match error {
            AnswerError::Invalid { field, .. } => AppError::Validation(field),
            AnswerError::Settled(_) => AppError::Conflict(
                "notification_action",
                "this notification was already answered",
            ),
            AnswerError::NotYours => AppError::NotFound("notification_action"),
            AnswerError::App(error) => error,
        })?;
    sqlx::query(
        "UPDATE notification_action_tokens SET used_action = $2, used_at = COALESCE(used_at, now()) WHERE id = $1",
    )
    .bind(token.id)
    .bind(action.as_str())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}
