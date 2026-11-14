//! AWD-62: an agent asks a person something on an inbox card, and the answer
//! goes back to it.
//!
//! `ask` stores the question and puts a card in the person's inbox (a
//! `question_asked` notification whose subject is the question). The card
//! offers the question's options and a box to type an answer; `answer`
//! records it once, marks the card read, and tells each of the asker's
//! connections with a `question.answered` event. The asker reads the answer
//! with the `question_list` tool.
//!
//! Only the person asked can answer, and only while they can still see the
//! project and the conversation or task the question is about.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    external_events::enqueue_external_event,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    notifications::{self, NewNotification, kind},
};

pub const MAX_QUESTION_CHARACTERS: usize = 2_000;
pub const MAX_OPTIONS: usize = 4;
pub const MAX_OPTION_CHARACTERS: usize = 120;
pub const MAX_ANSWER_CHARACTERS: usize = 4_000;
/// Open questions one asker may have waiting on one person at a time.
pub const MAX_PENDING_PER_PERSON: i64 = 20;

/// The event each of the asker's connections gets when the person answers.
pub const ANSWERED_EVENT: &str = "question.answered";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

/// What the person answered: the option they picked, the words they typed,
/// or both.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// The picked option's label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A question as its card shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionView {
    pub id: Uuid,
    /// pending or answered
    pub status: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<AnswerView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
}

fn allowed_character(character: char) -> bool {
    !character.is_control() || character == '\n' || character == '\t'
}

/// The question's text, trimmed: 1 to 2,000 characters, line breaks allowed.
pub fn normalize_question(value: &str) -> Option<String> {
    let value = value.trim();
    ((1..=MAX_QUESTION_CHARACTERS).contains(&value.chars().count())
        && value.chars().all(allowed_character))
    .then(|| value.to_owned())
}

/// A typed answer, trimmed: 1 to 4,000 characters, line breaks allowed.
pub fn normalize_answer(value: &str) -> Option<String> {
    let value = value.trim();
    ((1..=MAX_ANSWER_CHARACTERS).contains(&value.chars().count())
        && value.chars().all(allowed_character))
    .then(|| value.to_owned())
}

/// Up to four distinct one-line labels, numbered "1" to "4" in order.
pub fn normalize_options(labels: &[String]) -> Result<Vec<QuestionOption>, &'static str> {
    if labels.len() > MAX_OPTIONS {
        return Err("Offer at most 4 options; the person can always type their own answer.");
    }
    let mut options: Vec<QuestionOption> = Vec::with_capacity(labels.len());
    for label in labels {
        let label = label.trim();
        if !(1..=MAX_OPTION_CHARACTERS).contains(&label.chars().count())
            || label.chars().any(char::is_control)
        {
            return Err("Each option is 1 to 120 characters, on one line.");
        }
        if options
            .iter()
            .any(|option| option.label.to_lowercase() == label.to_lowercase())
        {
            return Err("Each option must be different.");
        }
        options.push(QuestionOption {
            id: (options.len() + 1).to_string(),
            label: label.to_owned(),
        });
    }
    Ok(options)
}

/// A question to put in front of someone. The caller has checked the asker
/// may write here, that the person is a member who can see the context, and
/// the limit on open questions.
pub struct NewQuestion {
    pub project_id: Uuid,
    pub asked_by_actor_id: Uuid,
    pub person_actor_id: Uuid,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
}

/// Store the question and put its card in the person's inbox.
pub async fn ask(
    transaction: &mut Transaction<'_, Postgres>,
    question: &NewQuestion,
) -> Result<Uuid, AppError> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO person_questions (
            id, project_id, asked_by_actor_id, person_actor_id, question, options,
            conversation_id, work_item_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(question.project_id)
    .bind(question.asked_by_actor_id)
    .bind(question.person_actor_id)
    .bind(&question.question)
    .bind(serde_json::to_value(&question.options).map_err(|_| AppError::Internal("options"))?)
    .bind(question.conversation_id)
    .bind(question.work_item_id)
    .execute(&mut **transaction)
    .await?;
    crate::requests::legacy::question_written(&mut **transaction, id).await?;
    // A question about a task opens the task; one about a conversation opens
    // it (the inbox works that out from conversation_id); otherwise the card
    // is the place to answer.
    let href = match (question.conversation_id, question.work_item_id) {
        (None, Some(work_item_id)) => {
            format!(
                "/projects/{}/work-items/{work_item_id}",
                question.project_id
            )
        }
        _ => "/inbox".to_owned(),
    };
    let notification = NewNotification {
        project_id: question.project_id,
        recipient_actor_id: question.person_actor_id,
        kind: kind::QUESTION_ASKED,
        subject_kind: "question",
        subject_id: id,
        actor_id: Some(question.asked_by_actor_id),
        conversation_id: question.conversation_id,
        message_id: None,
        work_item_id: question.work_item_id,
        comment_id: None,
        agent_run_id: None,
        approval_id: None,
        pull_request_id: None,
        detail: serde_json::json!({ "href": href }),
        group_key: format!("question:{id}"),
        dedupe_key: format!("question:{id}"),
    };
    if notifications::deliver(transaction, &notification).await? {
        notifications::announce(transaction, question.project_id, question.person_actor_id).await?;
    }
    Ok(id)
}

/// How a person answers: an option, words, or both.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(FromRow)]
struct QuestionRow {
    id: Uuid,
    project_id: Uuid,
    asked_by_actor_id: Uuid,
    status: String,
    question: String,
    options: serde_json::Value,
    answer_option_id: Option<String>,
    answer_text: Option<String>,
    answered_at: Option<String>,
}

const ROW_COLUMNS: &str = r#"
    q.id, q.project_id, q.asked_by_actor_id, q.status, q.question, q.options,
    q.answer_option_id, q.answer_text,
    to_char(q.answered_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS answered_at
"#;

/// Whether `$2` can still see the question's project and what it's about:
/// the conversation, and (for a task) tasks in that project. `$3` is the
/// agent connection, if any.
pub const CONTEXT_VISIBLE: &str = r#"
    actor_can_access_project($2, $3, q.project_id)
    AND (q.conversation_id IS NULL OR conversation_visible_to(q.conversation_id, $2))
    AND (q.work_item_id IS NULL OR EXISTS (
        SELECT 1 FROM project_memberships sees_task
        WHERE sees_task.project_id = q.project_id AND sees_task.actor_id = $2
          AND sees_task.enabled AND custom_role_allows(sees_task.role_id, 'tasks.view')
    ))
"#;

fn parse_options(value: serde_json::Value) -> Vec<QuestionOption> {
    serde_json::from_value(value).unwrap_or_default()
}

fn view_of(row: QuestionRow) -> QuestionView {
    let options = parse_options(row.options);
    let answer = (row.status == "answered").then(|| AnswerView {
        label: row.answer_option_id.as_ref().and_then(|picked| {
            options
                .iter()
                .find(|option| &option.id == picked)
                .map(|option| option.label.clone())
        }),
        option_id: row.answer_option_id,
        text: row.answer_text,
    });
    QuestionView {
        id: row.id,
        status: row.status,
        question: row.question,
        options,
        answer,
        answered_at: row.answered_at,
    }
}

/// Answer a question as the person it was asked of. `connection_id` is the
/// agent connection a call came through (none for people).
pub async fn answer(
    pool: &PgPool,
    person_actor_id: Uuid,
    connection_id: Option<Uuid>,
    question_id: Uuid,
    request: AnswerRequest,
    key: &IdempotencyKey,
) -> Result<(QuestionView, bool), AppError> {
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = pool.begin().await?;
    let row = sqlx::query_as::<_, QuestionRow>(&format!(
        r#"
        SELECT {ROW_COLUMNS}
        FROM person_questions q
        WHERE q.id = $1 AND q.person_actor_id = $2 AND {CONTEXT_VISIBLE}
        FOR UPDATE OF q
        "#
    ))
    .bind(question_id)
    .bind(person_actor_id)
    .bind(connection_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("question"))?;

    let scope = idempotency::resource_bound_scope("question", question_id, "answer")?;
    let reservation =
        match idempotency::reserve(&mut transaction, person_actor_id, &scope, key, request_hash)
            .await?
        {
            IdempotencyOutcome::Replay(_) => {
                transaction.commit().await?;
                return Ok((view_of(row), true));
            }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
    if row.status != "pending" {
        return Err(AppError::Conflict("question", "already answered"));
    }
    let options = parse_options(row.options.clone());
    let option_id = match request.option_id.as_deref().map(str::trim) {
        Some(picked) => Some(
            options
                .iter()
                .find(|option| option.id == picked)
                .map(|option| option.id.clone())
                .ok_or(AppError::Validation("optionId"))?,
        ),
        None => None,
    };
    let text = match request.text.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            Some(normalize_answer(text).ok_or(AppError::Validation("text"))?)
        }
        _ => None,
    };
    if option_id.is_none() && text.is_none() {
        return Err(AppError::Validation("text"));
    }
    let row = sqlx::query_as::<_, QuestionRow>(&format!(
        r#"
        UPDATE person_questions q
        SET status = 'answered', answer_option_id = $2, answer_text = $3,
            answered_at = now(), updated_at = now()
        WHERE q.id = $1
        RETURNING {ROW_COLUMNS}
        "#
    ))
    .bind(question_id)
    .bind(&option_id)
    .bind(&text)
    .fetch_one(&mut *transaction)
    .await?;
    crate::requests::legacy::question_written(&mut *transaction, question_id).await?;

    // The card has been dealt with.
    sqlx::query(
        r#"
        UPDATE notifications SET read_at = COALESCE(read_at, now()), updated_at = now()
        WHERE recipient_actor_id = $1 AND subject_kind = 'question' AND subject_id = $2
        "#,
    )
    .bind(person_actor_id)
    .bind(question_id)
    .execute(&mut *transaction)
    .await?;
    notifications::announce(&mut transaction, row.project_id, person_actor_id).await?;
    tell_asker(&mut transaction, &row).await?;

    let view = view_of(row);
    idempotency::complete(&mut transaction, &reservation, StatusCode::OK, &view).await?;
    transaction.commit().await?;
    Ok((view, false))
}

/// A `question.answered` event for each of the asker's connections that
/// reaches the project. The event names the question, never the answer: the
/// asker reads it with question_list.
async fn tell_asker(
    transaction: &mut Transaction<'_, Postgres>,
    row: &QuestionRow,
) -> Result<(), AppError> {
    let connections = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM external_agent_connections
        WHERE actor_id = $1 AND status = 'active' AND agent_connection_can_access(id, $2)
        ORDER BY id
        "#,
    )
    .bind(row.asked_by_actor_id)
    .bind(row.project_id)
    .fetch_all(&mut **transaction)
    .await?;
    for connection_id in connections {
        enqueue_external_event(
            transaction,
            connection_id,
            row.project_id,
            ANSWERED_EVENT,
            "Your question was answered; read it with question_list",
            "person_question",
            row.id,
            "/inbox",
        )
        .await?;
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/questions/{id}/answer", post(answer_http))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnswerResponse {
    question: QuestionView,
}

async fn answer_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(question_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<AnswerRequest>,
) -> Result<Json<AnswerResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let (question, _) = answer(
        &state.pool,
        principal.actor_id,
        None,
        question_id,
        request,
        &key,
    )
    .await?;
    Ok(Json(AnswerResponse { question }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_numbered_distinct_and_few() {
        let options = normalize_options(&[" Yes ".to_owned(), "No".to_owned()]).expect("options");
        assert_eq!(
            options[0],
            QuestionOption {
                id: "1".into(),
                label: "Yes".into()
            }
        );
        assert_eq!(options[1].id, "2");
        assert!(normalize_options(&["Yes".to_owned(), "yes".to_owned()]).is_err());
        assert!(normalize_options(&vec!["a".to_owned(); 5]).is_err());
        assert!(normalize_options(&["two\nlines".to_owned()]).is_err());
        assert!(normalize_options(&[" ".to_owned()]).is_err());
        assert!(normalize_options(&[]).expect("none").is_empty());
    }

    #[test]
    fn questions_and_answers_keep_line_breaks_but_no_other_controls() {
        assert_eq!(
            normalize_question("  Ship it?\nOr wait? ").as_deref(),
            Some("Ship it?\nOr wait?")
        );
        assert!(normalize_question("   ").is_none());
        assert!(normalize_question("bell\u{7}").is_none());
        assert!(normalize_question(&"x".repeat(2_001)).is_none());
        assert!(normalize_answer(&"x".repeat(4_000)).is_some());
        assert!(normalize_answer(&"x".repeat(4_001)).is_none());
    }
}
