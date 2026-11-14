//! AWD-62 through MCP: ask a person something on an inbox card
//! (`ask_person`), read what you asked and what you were asked
//! (`question_list`), and answer a question asked of you
//! (`question_answer`, so a person's app can do what the card does).
//!
//! The rules are `crate::questions`, the same the web app's Answer uses.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, accessible_project_ids, insert_audit, mark_via, parse_id, project_is_chosen,
        public_url, resolve_people,
    },
    talk::can_write_in,
    tasks::resolve_task,
    writes::{REQUEST_ID_MESSAGE, parse_request_id},
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{IdempotencyOutcome, RequestHash, reserve, resource_bound_scope},
    policy::Action,
    questions::{self, AnswerRequest, NewQuestion, QuestionView},
};

const ASK_MESSAGE: &str =
    "Your role here can't send messages, so it can't ask people things either.";
const PERSON_MESSAGE: &str = "Ask one person in this project, by @handle or actor ID. They must be a person (not an agent), active here, and not you.";
const QUESTION_MESSAGE: &str = "Ask in 1 to 2,000 characters; line breaks are fine.";
const ANSWER_MESSAGE: &str = "Pick one of the question's options (optionId), type an answer of 1 to 4,000 characters (text), or both.";

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AskPersonArguments {
    /// Who to ask: an @handle or actor ID of a person in the project
    /// (people_search finds them).
    pub person: String,
    /// The question, 1 to 2,000 characters. Say what you need and why, so it
    /// can be answered from the card alone.
    pub question: String,
    /// Up to 4 answers to pick from, each 1 to 120 characters on one line,
    /// such as ["Ship it", "Wait for review"]. The person can always type
    /// their own answer instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// The conversation it's about, if any; you and they must both see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The task it's about, if any (ID, link or key such as AWD-12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The project to ask in, when neither conversationId nor taskId says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// A stable ID so a retry never asks twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AskPersonValue {
    pub question_id: String,
    /// pending until they answer.
    pub status: String,
    /// Who it went to.
    pub person: String,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct QuestionListArguments {
    /// asked (default: questions you asked others) or for_me (questions
    /// others asked you).
    pub role: Option<String>,
    /// pending, answered or all (default).
    pub status: Option<String>,
    /// Just this question.
    pub question_id: Option<String>,
    /// How many, newest first (default 20, at most 50).
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionOptionLine {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionAnswerLine {
    /// The option picked, if one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What they typed, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionLine {
    pub id: String,
    /// pending or answered
    pub status: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<QuestionOptionLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<QuestionAnswerLine>,
    pub asked_by: String,
    pub person: String,
    pub project: String,
    /// The conversation it's about, while you can see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The task it's about (its key), while you can see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub asked_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionListValue {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<QuestionLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct QuestionAnswerArguments {
    /// The question's ID (question_list role for_me lists them).
    pub question_id: String,
    /// The option to pick, by its id ("1" to "4").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// Your answer in words, 1 to 4,000 characters, with or without an
    /// option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A stable ID so a retry never answers twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuestionAnswerValue {
    pub question_id: String,
    pub status: String,
    pub answer: QuestionAnswerLine,
    pub idempotent_replay: bool,
}

fn answer_line(answer: questions::AnswerView) -> QuestionAnswerLine {
    QuestionAnswerLine {
        option_id: answer.option_id,
        label: answer.label,
        text: answer.text,
    }
}

/// The person asked, if they're an active person in the project who isn't
/// the asker.
async fn resolve_person(
    state: &AppState,
    project_id: Uuid,
    asker: Uuid,
    person: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    let Ok(found) = resolve_people(state, project_id, &[person.to_owned()]).await? else {
        return Ok(None);
    };
    let Some(&actor_id) = found.first() else {
        return Ok(None);
    };
    if actor_id == asker {
        return Ok(None);
    }
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT membership.display_name
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1 AND membership.actor_id = $2 AND membership.enabled
          AND actor.kind = 'human' AND actor.status = 'active'
          AND actor_can_access_project($2, NULL, $1)
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await
    .map(|name| name.map(|name| (actor_id, name)))
    .map_err(AppError::from)
}

pub(super) async fn ask_person(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AskPersonArguments,
) -> ToolResult<AskPersonValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(question) = questions::normalize_question(&arguments.question) else {
        return ToolResult::invalid("question", QUESTION_MESSAGE);
    };
    let options = match questions::normalize_options(arguments.options.as_deref().unwrap_or(&[])) {
        Ok(options) => options,
        Err(message) => return ToolResult::invalid("options", message),
    };
    let key = match arguments.client_request_id.as_deref() {
        Some(value) => match parse_request_id(value) {
            Some(key) => key,
            None => return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE),
        },
        None => match parse_request_id(&Uuid::now_v7().to_string()) {
            Some(key) => key,
            None => return ToolResult::unavailable(),
        },
    };
    let result = async {
        // The project: the task's, when one was named, else the one the call
        // is in (from projectId or conversationId).
        let mut project_id = principal.project_id;
        let work_item_id = match arguments.task_id.as_deref() {
            Some(reference) => match resolve_task(state, principal, reference).await? {
                Some((task_id, task_project)) => {
                    if principal.project_explicit && task_project != project_id {
                        return Ok(ToolResult::invalid(
                            "taskId",
                            "That task is in another project than the one you named.",
                        ));
                    }
                    project_id = task_project;
                    Some(task_id)
                }
                None => return Ok(ToolResult::not_found("That task")),
            },
            None => None,
        };
        if work_item_id.is_none() && !project_is_chosen(principal) {
            return Ok(ToolResult::invalid(
                "projectId",
                "Name the project to ask in (projectId), or the conversation or task it's about.",
            ));
        }
        if !can_write_in(state, principal, project_id, Action::MessageSend).await? {
            return Ok(ToolResult::denied(ASK_MESSAGE));
        }
        let Some((person_id, person_name)) =
            resolve_person(state, project_id, principal.actor_id, &arguments.person).await?
        else {
            return Ok(ToolResult::invalid("person", PERSON_MESSAGE));
        };
        let conversation_id = match arguments.conversation_id.as_deref() {
            Some(reference) => {
                let Some(id) = parse_id(reference) else {
                    return Ok(ToolResult::not_found("That conversation"));
                };
                let (asker_sees, person_sees) = sqlx::query_as::<_, (bool, bool)>(
                    r#"
                    SELECT conversation_visible_to(id, $2), conversation_visible_to(id, $3)
                    FROM conversations WHERE id = $1 AND project_id = $4
                    "#,
                )
                .bind(id)
                .bind(principal.actor_id)
                .bind(person_id)
                .bind(project_id)
                .fetch_optional(&state.pool)
                .await?
                .unwrap_or((false, false));
                if !asker_sees {
                    return Ok(ToolResult::not_found("That conversation"));
                }
                if !person_sees {
                    return Ok(ToolResult::invalid(
                        "conversationId",
                        "They can't see that conversation, so the question can't point at it. Ask without it, or somewhere they can see.",
                    ));
                }
                Some(id)
            }
            None => None,
        };
        if work_item_id.is_some() {
            let person_sees_tasks = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM project_memberships
                    WHERE project_id = $1 AND actor_id = $2 AND enabled
                      AND custom_role_allows(role_id, 'tasks.view')
                )
                "#,
            )
            .bind(project_id)
            .bind(person_id)
            .fetch_one(&state.pool)
            .await?;
            if !person_sees_tasks {
                return Ok(ToolResult::invalid(
                    "taskId",
                    "Their role here can't see tasks, so the question can't point at one. Ask without it.",
                ));
            }
        }

        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let scope = resource_bound_scope("mcp_questions", project_id, "ask_person")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation =
            match reserve(&mut transaction, principal.actor_id, &scope, &key, request_hash).await {
                Ok(IdempotencyOutcome::Replay(replay)) => {
                    let mut value = replay.decode::<AskPersonValue>()?;
                    transaction.commit().await?;
                    value.idempotent_replay = true;
                    return Ok(ToolResult::ok(value));
                }
                Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
                Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                    return Ok(ToolResult::invalid(
                        "clientRequestId",
                        "That clientRequestId was already used for a different question. Use a new one.",
                    ));
                }
                Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                    return Ok(ToolResult::conflict(
                        "The same question is still being asked. Try again in a moment.",
                    ));
                }
                Err(error) => return Err(error),
            };
        // One asker can't pile questions on someone: checked under a lock on
        // the pair so two calls at once count each other.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("person_questions:{}:{person_id}", principal.actor_id))
            .execute(&mut *transaction)
            .await?;
        let pending = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*) FROM person_questions
            WHERE asked_by_actor_id = $1 AND person_actor_id = $2 AND status = 'pending'
            "#,
        )
        .bind(principal.actor_id)
        .bind(person_id)
        .fetch_one(&mut *transaction)
        .await?;
        if pending >= questions::MAX_PENDING_PER_PERSON {
            return Ok(ToolResult::conflict(format!(
                "{person_name} already has {pending} questions from you waiting. Wait for answers (question_list) before asking more."
            )));
        }
        let question_id = questions::ask(
            &mut transaction,
            &NewQuestion {
                project_id,
                asked_by_actor_id: principal.actor_id,
                person_actor_id: person_id,
                question,
                options,
                conversation_id,
                work_item_id,
            },
        )
        .await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "question.ask",
            "person_question",
            question_id,
            serde_json::json!({ "personActorId": person_id }),
        )
        .await?;
        let value = AskPersonValue {
            question_id: question_id.to_string(),
            status: "pending".to_owned(),
            person: person_name,
            idempotent_replay: false,
        };
        crate::idempotency::complete(
            &mut transaction,
            &reservation,
            axum::http::StatusCode::OK,
            &value,
        )
        .await?;
        transaction.commit().await?;
        Ok(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "ask_person"))
}

#[derive(sqlx::FromRow)]
struct ListRow {
    id: Uuid,
    status: String,
    question: String,
    options: serde_json::Value,
    answer_option_id: Option<String>,
    answer_text: Option<String>,
    asked_by: String,
    person: String,
    project: String,
    project_id: Uuid,
    conversation_id: Option<Uuid>,
    conversation_seen: bool,
    task_key: Option<String>,
    work_item_id: Option<Uuid>,
    task_seen: bool,
    asked_at: String,
    answered_at: Option<String>,
}

pub(super) async fn question_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: QuestionListArguments,
) -> ToolResult<QuestionListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let for_me = match arguments.role.as_deref().unwrap_or("asked") {
        "asked" => false,
        "for_me" => true,
        _ => return ToolResult::invalid("role", "Use asked or for_me."),
    };
    let status = match arguments.status.as_deref().unwrap_or("all") {
        "all" => None,
        status @ ("pending" | "answered") => Some(status.to_owned()),
        _ => return ToolResult::invalid("status", "Use pending, answered or all."),
    };
    let question_id = match arguments.question_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::not_found("That question"),
        },
        None => None,
    };
    let limit = i64::from(arguments.limit.unwrap_or(20).clamp(1, 50));
    let result = async {
        let projects = accessible_project_ids(state, principal).await?;
        // Questions for you only while you can still see what they're about;
        // ones you asked always, with a context you can no longer see left
        // out.
        let rows = sqlx::query_as::<_, ListRow>(&format!(
            r#"
            SELECT q.id, q.status, q.question, q.options, q.answer_option_id, q.answer_text,
                   COALESCE(asker.display_name, 'Someone') AS asked_by,
                   COALESCE(person.display_name, 'Someone') AS person,
                   project.name AS project, q.project_id,
                   q.conversation_id,
                   q.conversation_id IS NOT NULL
                       AND conversation_visible_to(q.conversation_id, $2) AS conversation_seen,
                   project.project_key || '-' || item.number::text AS task_key,
                   q.work_item_id,
                   q.work_item_id IS NOT NULL AND EXISTS (
                       SELECT 1 FROM project_memberships sees_task
                       WHERE sees_task.project_id = q.project_id AND sees_task.actor_id = $2
                         AND sees_task.enabled
                         AND custom_role_allows(sees_task.role_id, 'tasks.view')
                   ) AS task_seen,
                   to_char(q.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                       AS asked_at,
                   to_char(q.answered_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                       AS answered_at
            FROM person_questions q
            JOIN projects project ON project.id = q.project_id
            LEFT JOIN project_memberships asker
              ON asker.project_id = q.project_id AND asker.actor_id = q.asked_by_actor_id
            LEFT JOIN project_memberships person
              ON person.project_id = q.project_id AND person.actor_id = q.person_actor_id
            LEFT JOIN task_rows item ON item.id = q.work_item_id
            WHERE q.project_id = ANY($1)
              AND CASE WHEN $4 THEN q.person_actor_id = $2 AND {visible}
                       ELSE q.asked_by_actor_id = $2 END
              AND ($5::text IS NULL OR q.status = $5)
              AND ($6::uuid IS NULL OR q.id = $6)
            ORDER BY q.created_at DESC, q.id DESC
            LIMIT $7
            "#,
            visible = questions::CONTEXT_VISIBLE,
        ))
        .bind(&projects)
        .bind(principal.actor_id)
        .bind(principal.external_agent_connection_id)
        .bind(for_me)
        .bind(status)
        .bind(question_id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?;
        let questions = rows
            .into_iter()
            .map(|row| {
                let options: Vec<questions::QuestionOption> =
                    serde_json::from_value(row.options).unwrap_or_default();
                let answer = (row.status == "answered").then(|| QuestionAnswerLine {
                    label: row.answer_option_id.as_ref().and_then(|picked| {
                        options
                            .iter()
                            .find(|option| &option.id == picked)
                            .map(|option| option.label.clone())
                    }),
                    option_id: row.answer_option_id.clone(),
                    text: row.answer_text.clone(),
                });
                let conversation_id = row.conversation_id.filter(|_| row.conversation_seen);
                let task_id = row.work_item_id.filter(|_| row.task_seen);
                let path = match (conversation_id, task_id) {
                    (Some(id), _) => format!("/messages/{id}"),
                    (None, Some(id)) => format!("/projects/{}/work-items/{id}", row.project_id),
                    _ => "/inbox".to_owned(),
                };
                QuestionLine {
                    id: row.id.to_string(),
                    status: row.status,
                    question: row.question,
                    options: options
                        .into_iter()
                        .map(|option| QuestionOptionLine {
                            id: option.id,
                            label: option.label,
                        })
                        .collect(),
                    answer,
                    asked_by: row.asked_by,
                    person: row.person,
                    project: row.project,
                    conversation_id: conversation_id.map(|id| id.to_string()),
                    task: row.task_key.filter(|_| row.task_seen),
                    asked_at: row.asked_at,
                    answered_at: row.answered_at,
                    url: public_url(state, &path),
                }
            })
            .collect();
        Ok(ToolResult::ok(QuestionListValue { questions }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "question_list"))
}

pub(super) async fn question_answer(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: QuestionAnswerArguments,
) -> ToolResult<QuestionAnswerValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(question_id) = parse_id(&arguments.question_id) else {
        return ToolResult::not_found("That question");
    };
    let key = match arguments.client_request_id.as_deref() {
        Some(value) => match parse_request_id(value) {
            Some(key) => key,
            None => return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE),
        },
        None => match parse_request_id(&Uuid::now_v7().to_string()) {
            Some(key) => key,
            None => return ToolResult::unavailable(),
        },
    };
    let request = AnswerRequest {
        option_id: arguments.option_id.clone(),
        text: arguments.text.clone(),
    };
    let result = async {
        let answered = questions::answer(
            &state.pool,
            principal.actor_id,
            principal.external_agent_connection_id,
            question_id,
            request,
            &key,
        )
        .await;
        let (view, replay): (QuestionView, bool) = match answered {
            Ok(answered) => answered,
            Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That question")),
            Err(AppError::Conflict("question", _)) => {
                return Ok(ToolResult::conflict(
                    "That question was already answered; question_list shows the answer.",
                ));
            }
            Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                return Ok(ToolResult::invalid(
                    "clientRequestId",
                    "That clientRequestId was already used for a different answer. Use a new one.",
                ));
            }
            Err(AppError::Validation(field)) => {
                return Ok(ToolResult::invalid(field, ANSWER_MESSAGE));
            }
            Err(error) => return Err(error),
        };
        if !replay {
            let project_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT project_id FROM person_questions WHERE id = $1",
            )
            .bind(question_id)
            .fetch_one(&state.pool)
            .await?;
            let mut transaction = state.pool.begin().await?;
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "question.answer",
                "person_question",
                question_id,
                serde_json::json!({}),
            )
            .await?;
            transaction.commit().await?;
        }
        Ok(ToolResult::ok(QuestionAnswerValue {
            question_id: view.id.to_string(),
            status: view.status,
            answer: view.answer.map(answer_line).unwrap_or(QuestionAnswerLine {
                option_id: None,
                label: None,
                text: None,
            }),
            idempotent_replay: replay,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "question_answer"))
}
