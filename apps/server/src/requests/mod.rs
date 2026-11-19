//! Requests, something an account needs from someone: an approval, an
//! answer or acceptance of a proposal (A1 section 3.8). One record for all
//! of them: a run's `interrupt` activity opens one (A1.7), the request
//! operations raise, answer and withdraw them by role (A1.8), and the
//! built-in host and a sponsored run request raise theirs here (A1.14).

use chrono::{DateTime, Duration, Utc};
use rmcp::schemars;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, runs::store::RunRow};

pub mod ops;

/// The most requests a run may have open at once.
pub const MAX_OPEN_PER_RUN: i64 = 200;
/// The most requests one account may have open for one addressee.
pub const MAX_OPEN_PER_ADDRESSEE: i64 = 20;
const MAX_OPTIONS: usize = 8;
const MAX_TITLE_CHARS: usize = 2_000;
const MAX_DETAIL_CHARS: usize = 8_000;

/// Every column of `requests` the service reads.
pub const REQUEST_COLUMNS: &str = r#"
    request.id, request.workspace_id, request.space_id, request.kind,
    request.raised_by_account_id, request.raised_via_host_id, request.run_id,
    request.host_request_ref, request.client_request_id, request.subject_kind,
    request.subject_id, request.conversation_id, request.work_item_id, request.title,
    request.detail, request.options, request.allow_text, request.answer_permission,
    request.addressed_account_ids, request.addressed_role_keys, request.addressed_group_ids,
    request.status, request.answer, request.answered_by_account_id, request.answered_via_app_id,
    request.answered_at, request.delivered_at, request.expires_at, request.on_expiry,
    request.version, request.created_at, request.updated_at
"#;

/// One `requests` row.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct RequestRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub kind: String,
    pub raised_by_account_id: Uuid,
    pub raised_via_host_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub host_request_ref: Option<String>,
    pub client_request_id: Option<String>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    pub title: String,
    pub detail: Option<String>,
    pub options: Value,
    pub allow_text: bool,
    pub answer_permission: String,
    pub addressed_account_ids: Vec<Uuid>,
    pub addressed_role_keys: Vec<String>,
    pub addressed_group_ids: Vec<Uuid>,
    pub status: String,
    pub answer: Option<Value>,
    pub answered_by_account_id: Option<Uuid>,
    pub answered_via_app_id: Option<Uuid>,
    pub answered_at: Option<DateTime<Utc>>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub on_expiry: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Why a request couldn't be opened: the field and what to do.
#[derive(Debug)]
pub enum OpenError {
    Refused {
        field: &'static str,
        message: String,
    },
    App(AppError),
}

impl From<AppError> for OpenError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for OpenError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

fn refused(field: &'static str, message: impl Into<String>) -> OpenError {
    OpenError::Refused {
        field,
        message: message.into(),
    }
}

/// A request a run raised, and how the run knows it.
#[derive(Clone, Debug)]
pub struct Raised {
    pub request_id: Uuid,
    pub client_request_id: Option<String>,
    pub created: bool,
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Text a caller typed (a title, a detail, an option, an id): secrets in
/// it are removed, as in run titles and activities.
fn prose(value: Option<&Value>) -> Option<String> {
    text(value)
        .map(|value| crate::run_log::redact(&value))
        .filter(|value| !value.trim().is_empty())
}

/// Who raises a request, where, and about what.
#[derive(Clone, Copy, Debug)]
pub struct Origin<'a> {
    pub account_id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    /// The id of the run activity that raised it.
    pub host_request_ref: Option<&'a str>,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    /// What it is about, by kind and id: a sponsored run request.
    pub subject: Option<(&'a str, Uuid)>,
    /// The field errors name.
    pub field: &'static str,
}

/// Open the request an `interrupt` activity's `payload.request` describes,
/// raised by the run's account in the run's space. `activity_ref` is the
/// activity's id: a repeat of the same activity, or of the same
/// `clientRequestId`, returns the request already open.
pub async fn open_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    activity_ref: &str,
    request: &Value,
) -> Result<Raised, OpenError> {
    let raised = open(
        transaction,
        Origin {
            account_id: run.account_id,
            workspace_id: run.workspace_id,
            space_id: run.space_id,
            run_id: Some(run.id),
            host_request_ref: Some(activity_ref),
            conversation_id: run.conversation_id,
            work_item_id: run.work_item_id,
            subject: None,
            field: "activities.payload.request",
        },
        request,
    )
    .await?;
    if raised.created {
        announce_raised(transaction, raised.request_id).await?;
    }
    Ok(raised)
}

/// Open a request: `request` holds its kind, title, detail, options,
/// allowText, audience ({accounts, roles}), expiresInSeconds, onExpiry and
/// clientRequestId. A repeat of the same `clientRequestId` (or run
/// activity) returns the request already open.
pub async fn open(
    transaction: &mut Transaction<'_, Postgres>,
    origin: Origin<'_>,
    request: &Value,
) -> Result<Raised, OpenError> {
    let field = origin.field;
    let client_request_id = prose(request.get("clientRequestId"));
    if client_request_id
        .as_deref()
        .is_some_and(|value| value.chars().count() > 200 || value.chars().any(char::is_control))
    {
        return Err(refused(
            field,
            "Keep clientRequestId to 200 characters on one line.",
        ));
    }
    if let Some(existing) = sqlx::query_as::<_, (Uuid, Option<String>)>(
        r#"
        SELECT id, client_request_id FROM requests
        WHERE (run_id = $1 AND host_request_ref = $2)
           OR ($4::text IS NOT NULL AND raised_by_account_id = $3 AND client_request_id = $4)
        LIMIT 1
        "#,
    )
    .bind(origin.run_id)
    .bind(origin.host_request_ref)
    .bind(origin.account_id)
    .bind(&client_request_id)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(Raised {
            request_id: existing.0,
            client_request_id: existing.1,
            created: false,
        });
    }
    let kind = match text(request.get("kind")).as_deref() {
        None | Some("question") => "question",
        Some("approval") => "approval",
        Some("proposal") => "proposal",
        Some(_) => {
            return Err(refused(
                field,
                "Ask a question, an approval or a proposal (kind).",
            ));
        }
    };
    let title =
        prose(request.get("title")).ok_or_else(|| refused(field, "Say what you need in title."))?;
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(refused(field, "Keep title to 2,000 characters."));
    }
    let detail = prose(request.get("detail"));
    if detail
        .as_deref()
        .is_some_and(|detail| detail.chars().count() > MAX_DETAIL_CHARS)
    {
        return Err(refused(field, "Keep detail to 8,000 characters."));
    }
    let options = match request.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(options)) if options.len() <= MAX_OPTIONS => {
            let mut clean = Vec::with_capacity(options.len());
            for option in options {
                let id = prose(option.get("id"));
                let label = prose(option.get("label"));
                let style = text(option.get("style"));
                let (Some(id), Some(label)) = (id, label) else {
                    return Err(refused(field, "Each option has an id and a label."));
                };
                if id.chars().count() > 40
                    || label.chars().count() > 200
                    || style
                        .as_deref()
                        .is_some_and(|style| !matches!(style, "approve" | "deny"))
                {
                    return Err(refused(
                        field,
                        "Keep option ids to 40 characters and labels to 200; style is approve or deny.",
                    ));
                }
                let mut entry = serde_json::Map::new();
                entry.insert("id".to_owned(), Value::String(id));
                entry.insert("label".to_owned(), Value::String(label));
                if let Some(style) = style {
                    entry.insert("style".to_owned(), Value::String(style));
                }
                clean.push(Value::Object(entry));
            }
            clean
        }
        Some(_) => return Err(refused(field, "Give at most 8 options.")),
    };
    let allow_text = request
        .get("allowText")
        .and_then(Value::as_bool)
        .unwrap_or(kind == "question" || options.is_empty());
    if options.is_empty() && !allow_text {
        return Err(refused(
            field,
            "Give options to pick from, or allow a typed answer (allowText).",
        ));
    }
    let audience = request.get("audience");
    let accounts = match audience.and_then(|audience| audience.get("accounts")) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(accounts)) if accounts.len() <= 20 => accounts
            .iter()
            .map(|account| {
                account
                    .as_str()
                    .and_then(|id| id.trim().parse::<Uuid>().ok())
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| refused(field, "Name people and agents in audience.accounts by ID."))?,
        Some(_) => {
            return Err(refused(
                field,
                "Name at most 20 accounts in audience.accounts.",
            ));
        }
    };
    let roles = match audience.and_then(|audience| audience.get("roles")) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(roles)) if roles.len() <= 10 => roles
            .iter()
            .map(|role| {
                role.as_str()
                    .map(str::trim)
                    .filter(|key| {
                        !key.is_empty()
                            && key.len() <= 40
                            && key
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    })
                    .map(str::to_owned)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| refused(field, "Name roles in audience.roles by key, like owner."))?,
        Some(_) => return Err(refused(field, "Name at most 10 roles in audience.roles.")),
    };
    let Some(space_id) = origin.space_id else {
        return Err(refused(
            field,
            "This isn't in a project, so there's nobody to ask yet.",
        ));
    };
    if origin.run_id.is_none() && accounts.is_empty() && roles.is_empty() {
        return Err(refused(
            field,
            "Say who to ask: people or agents (audience.accounts) or roles (audience.roles).",
        ));
    }
    if !accounts.is_empty() {
        let members = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(DISTINCT membership.actor_id) FROM project_memberships membership
            WHERE membership.project_id = $1 AND membership.actor_id = ANY($2)
              AND membership.enabled AND membership.actor_id <> $3
            "#,
        )
        .bind(space_id)
        .bind(&accounts)
        .bind(origin.account_id)
        .fetch_one(&mut **transaction)
        .await?;
        let distinct = {
            let mut sorted = accounts.clone();
            sorted.sort_unstable();
            sorted.dedup();
            sorted.len()
        };
        if usize::try_from(members).unwrap_or(0) != distinct {
            return Err(refused(
                field,
                "Address people and agents in this project, other than yourself.",
            ));
        }
    }
    let expires_at = match request.get("expiresInSeconds").and_then(Value::as_i64) {
        None => None,
        Some(seconds) if (60..=7 * 86_400).contains(&seconds) => {
            Some(Utc::now() + Duration::seconds(seconds))
        }
        Some(_) => {
            return Err(refused(
                field,
                "expiresInSeconds is 60 (a minute) to 604,800 (a week).",
            ));
        }
    };

    let on_expiry = match text(request.get("onExpiry")).as_deref() {
        None | Some("expire") => "expire",
        Some("deny") if expires_at.is_some() && deny_option(&options).is_some() => "deny",
        Some(_) => {
            return Err(refused(
                field,
                "onExpiry is expire, or deny with expiresInSeconds and an option styled deny.",
            ));
        }
    };

    // Limits, counted under a lock on the run (or the raiser) so two
    // calls count each other.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('requests:' || $1::text, 0))")
        .bind(origin.run_id.unwrap_or(origin.account_id))
        .execute(&mut **transaction)
        .await?;
    if let Some(run_id) = origin.run_id
        && open_count(transaction, run_id).await? >= MAX_OPEN_PER_RUN
    {
        return Err(refused(
            field,
            "This run already has 200 requests waiting. Wait for answers first.",
        ));
    }
    if !accounts.is_empty() {
        let crowded = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM unnest($2::uuid[]) AS addressee(id)
                WHERE (SELECT count(*) FROM requests
                       WHERE raised_by_account_id = $1 AND status = 'open'
                         AND addressed_account_ids @> ARRAY[addressee.id]) >= $3)
            "#,
        )
        .bind(origin.account_id)
        .bind(&accounts)
        .bind(MAX_OPEN_PER_ADDRESSEE)
        .fetch_one(&mut **transaction)
        .await?;
        if crowded {
            return Err(refused(
                field,
                "Someone you're asking already has 20 of your requests waiting. Wait for answers first.",
            ));
        }
    }
    let request_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO requests (
            id, workspace_id, space_id, kind, raised_by_account_id, run_id, host_request_ref,
            client_request_id, conversation_id, work_item_id, title, detail, options,
            allow_text, answer_permission, addressed_account_ids, addressed_role_keys,
            expires_at, on_expiry, subject_kind, subject_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                $19, $20, $21)
        "#,
    )
    .bind(request_id)
    .bind(origin.workspace_id)
    .bind(space_id)
    .bind(kind)
    .bind(origin.account_id)
    .bind(origin.run_id)
    .bind(origin.host_request_ref)
    .bind(&client_request_id)
    .bind(origin.conversation_id)
    .bind(origin.work_item_id)
    .bind(&title)
    .bind(&detail)
    .bind(Value::Array(options))
    .bind(allow_text)
    .bind(
        // A join (A2.18) is answered by whoever its roles name; an agent a
        // host made (A2.19) needs `agents.create`; a run's approval needs
        // `runs.approve`.
        match origin.subject {
            Some((subject, _)) if subject == crate::hosts::register::REGISTER_SUBJECT => {
                "agents.create"
            }
            Some((subject, _))
                if kind == "approval" && subject != crate::accounts::join::JOIN_SUBJECT =>
            {
                "runs.approve"
            }
            _ if kind == "approval" && origin.run_id.is_some() => "runs.approve",
            _ => "requests.answer",
        },
    )
    .bind(&accounts)
    .bind(&roles)
    .bind(expires_at)
    .bind(on_expiry)
    .bind(origin.subject.map(|(kind, _)| kind))
    .bind(origin.subject.map(|(_, id)| id))
    .execute(&mut **transaction)
    .await?;
    stage_event(
        transaction,
        &crate::events::catalog::REQUEST_RAISED,
        request_id,
        origin.workspace_id,
        Some(space_id),
        1,
        origin.account_id,
    )
    .await?;
    Ok(Raised {
        request_id,
        client_request_id,
        created: true,
    })
}

/// How many requests a run has open.
pub async fn open_count(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM requests WHERE run_id = $1 AND status = 'open'",
    )
    .bind(run_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// The run says it no longer waits on a request (an `interrupt_resolved`
/// activity): named by the request's id, its `clientRequestId`, or the id
/// of the activity that raised it. An open one is superseded.
pub async fn resolved_by_run(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    reference: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE requests
           SET status = 'superseded', version = version + 1, updated_at = now()
         WHERE run_id = $1 AND status = 'open'
           AND (id::text = $2 OR host_request_ref = $2 OR client_request_id = $2)
        "#,
    )
    .bind(run.id)
    .bind(reference.trim())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Withdraw the open requests about these things (a sponsored run request
/// that was itself withdrawn), leaving the things alone.
pub async fn withdraw_about(
    transaction: &mut Transaction<'_, Postgres>,
    subject_kind: &str,
    subject_ids: &[Uuid],
) -> Result<(), AppError> {
    let open = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM requests
        WHERE subject_kind = $1 AND subject_id = ANY($2) AND status = 'open'
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(subject_kind)
    .bind(subject_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for request_id in open {
        settle_only(transaction, request_id, "cancelled", None, None).await?;
    }
    Ok(())
}

/// A run finished: what it still asked for is withdrawn.
pub async fn withdraw_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    let open = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM requests WHERE run_id = $1 AND status = 'open' ORDER BY id FOR UPDATE",
    )
    .bind(run_id)
    .fetch_all(&mut **transaction)
    .await?;
    for request_id in open {
        // The run is ending: nothing on it to move along.
        settle_only(transaction, request_id, "cancelled", None, None).await?;
    }
    Ok(())
}

/// One request, by id.
pub async fn get(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<Option<RequestRow>, AppError> {
    Ok(sqlx::query_as::<_, RequestRow>(&format!(
        "SELECT {REQUEST_COLUMNS} FROM requests request WHERE request.id = $1"
    ))
    .bind(request_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// A host says one of its agents now has a reference from a different
/// lineage (A1.11): it may be a different agent, so the account's managers
/// decide, in each workspace where it is a member. A repeat of the same
/// notice returns the requests already open. Returns their ids.
pub async fn open_for_host_agent(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    host_account_id: Uuid,
    account_id: Uuid,
    host_name: &str,
    old_reference: &str,
    new_reference: &str,
) -> Result<Vec<Uuid>, AppError> {
    use sha2::{Digest, Sha256};
    let fingerprint = Sha256::digest(new_reference.as_bytes())
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let clip = |value: &str, max: usize| value.chars().take(max).collect::<String>();
    // The names come from the host: secrets in them are removed.
    let title = crate::run_log::redact(&format!(
        "{} says the agent it knew as \"{}\" is now \"{}\", from a different line. Is it the same agent?",
        clip(host_name, 80),
        clip(old_reference, 200),
        clip(new_reference, 200),
    ));
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH workspaces AS (
            SELECT membership.organization_id
            FROM organization_memberships membership
            WHERE membership.actor_id = $3 AND membership.enabled
        ),
        inserted AS (
            INSERT INTO requests (
                id, workspace_id, kind, raised_by_account_id, raised_via_host_id,
                client_request_id, subject_kind, subject_id, title, options,
                answer_permission, addressed_role_keys
            )
            SELECT gen_random_uuid(), workspace.organization_id, 'approval', $2, $1,
                   'agent_ref_changed:' || workspace.organization_id || ':' || $3 || ':' || $4,
                   'host_agent', $3, $5,
                   '[{"id": "same", "label": "Same agent", "style": "approve"},
                     {"id": "different", "label": "A different agent", "style": "deny"}]'::jsonb,
                   'agents.manage', ARRAY['owner', 'admin']
            FROM workspaces workspace
            ON CONFLICT (raised_by_account_id, client_request_id) DO NOTHING
            RETURNING id
        )
        SELECT id FROM inserted
        UNION
        SELECT request.id FROM requests request
        JOIN workspaces workspace ON workspace.organization_id = request.workspace_id
        WHERE request.raised_by_account_id = $2 AND request.status = 'open'
          AND request.client_request_id
              = 'agent_ref_changed:' || workspace.organization_id || ':' || $3 || ':' || $4
        "#,
    )
    .bind(host_id)
    .bind(host_account_id)
    .bind(account_id)
    .bind(fingerprint)
    .bind(clip(&title, MAX_TITLE_CHARS))
    .fetch_all(&mut **transaction)
    .await?)
}

/// A host was unlinked: the open requests raised through it are cancelled.
pub async fn cancel_raised_via_host(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE requests
           SET status = 'cancelled', version = version + 1, updated_at = now()
         WHERE raised_via_host_id = $1 AND status = 'open'
        "#,
    )
    .bind(host_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

// ------------------------------------------------------------- A1.8 flow

/// The most characters of a typed answer.
pub const MAX_ANSWER_CHARS: usize = 4_000;

/// The option a `deny` expiry picks: the first styled deny.
fn deny_option(options: &[Value]) -> Option<&Value> {
    options
        .iter()
        .find(|option| option.get("style").and_then(Value::as_str) == Some("deny"))
}

/// One option to pick.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestOption {
    pub id: String,
    pub label: String,
    /// approve or deny, when the option says yes or no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// What was answered.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestAnswerView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// The picked option's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A request as people and agents see it.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// question, approval or proposal.
    pub kind: String,
    /// open, answered, expired, cancelled or superseded.
    pub status: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub options: Vec<RequestOption>,
    /// Whether a typed answer is taken.
    pub allow_text: bool,
    #[schemars(with = "String")]
    pub raised_by: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub conversation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub task_id: Option<Uuid>,
    /// The accounts it is addressed to, by id.
    #[schemars(with = "Vec<String>")]
    pub addressed_accounts: Vec<Uuid>,
    /// The roles it is addressed to, by key.
    pub addressed_roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RequestAnswerView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub answered_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Pass it as expectedVersion to answer only what you read.
    pub version: i64,
}

/// The options stored on a request.
pub fn options_of(options: &Value) -> Vec<RequestOption> {
    serde_json::from_value(options.clone()).unwrap_or_default()
}

/// A stored answer as shown, with the picked option's label.
pub fn answer_of(answer: Option<&Value>, options: &[RequestOption]) -> Option<RequestAnswerView> {
    let answer = answer?;
    let option_id = text(answer.get("optionId"));
    Some(RequestAnswerView {
        label: option_id.as_ref().and_then(|picked| {
            options
                .iter()
                .find(|option| &option.id == picked)
                .map(|option| option.label.clone())
        }),
        option_id,
        text: text(answer.get("text")),
    })
}

impl From<&RequestRow> for RequestView {
    fn from(row: &RequestRow) -> Self {
        let options = options_of(&row.options);
        Self {
            id: row.id,
            kind: row.kind.clone(),
            status: row.status.clone(),
            title: row.title.clone(),
            detail: row.detail.clone(),
            answer: answer_of(row.answer.as_ref(), &options),
            options,
            allow_text: row.allow_text,
            raised_by: row.raised_by_account_id,
            space_id: row.space_id,
            run_id: row.run_id,
            conversation_id: row.conversation_id,
            task_id: row.work_item_id,
            addressed_accounts: row.addressed_account_ids.clone(),
            addressed_roles: row.addressed_role_keys.clone(),
            answered_by: row.answered_by_account_id,
            answered_at: row.answered_at,
            expires_at: row.expires_at,
            created_at: row.created_at,
            version: row.version,
        }
    }
}

/// A request `viewer` may see, locked for a change when `lock`.
pub async fn visible(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    viewer: Uuid,
    lock: bool,
) -> Result<Option<RequestRow>, AppError> {
    Ok(sqlx::query_as::<_, RequestRow>(&format!(
        "SELECT {REQUEST_COLUMNS} FROM requests request \
         WHERE request.id = $1 AND request_visible_to(request.id, $2) {}",
        if lock { "FOR UPDATE" } else { "" }
    ))
    .bind(request_id)
    .bind(viewer)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// A request with whether its reader may answer it, for the places that
/// show one where it was raised (a thread, a run).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestForReader {
    #[serde(flatten)]
    pub request: RequestView,
    pub may_answer: bool,
}

/// The requests raised in a conversation that `caller` may see, oldest
/// first (at most 200), each with whether they may answer it.
pub async fn in_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    caller: &crate::access::Caller,
) -> Result<Vec<RequestForReader>, AppError> {
    let rows = sqlx::query_as::<_, RequestRow>(&format!(
        r#"
        SELECT {REQUEST_COLUMNS} FROM requests request
        WHERE request.conversation_id = $1 AND request_visible_to(request.id, $2)
        ORDER BY request.created_at, request.id
        LIMIT 200
        "#
    ))
    .bind(conversation_id)
    .bind(caller.account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut shown = Vec::with_capacity(rows.len());
    for row in rows {
        let may_answer = may_answer(transaction, caller, &row).await?;
        shown.push(RequestForReader {
            request: RequestView::from(&row),
            may_answer,
        });
    }
    Ok(shown)
}

/// The access scope a request is answered in: its space, or its workspace.
fn scope_of(row: &RequestRow) -> crate::access::Target {
    row.space_id.map_or(
        crate::access::Target::Workspace(row.workspace_id),
        crate::access::Target::Space,
    )
}

/// Whether `account` holds `permission` where the request lives.
async fn holds(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &crate::access::Caller,
    row: &RequestRow,
    writes: bool,
) -> Result<bool, AppError> {
    let Some(permission) = crate::policy::Permission::from_key(&row.answer_permission) else {
        return Ok(false);
    };
    Ok(crate::access::check_locked(
        transaction,
        caller,
        crate::access::Need {
            permission: Some(permission),
            module: None,
            writes,
        },
        scope_of(row),
    )
    .await?
    .is_ok())
}

/// Whether the account holds one of the roles a request is addressed to,
/// where it lives (by the role's key, built in or the workspace's own).
async fn holds_addressed_role(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    row: &RequestRow,
) -> Result<bool, AppError> {
    if row.addressed_role_keys.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships membership
            LEFT JOIN workspace_roles custom ON custom.id = membership.role_id
            WHERE $2::uuid IS NOT NULL AND membership.project_id = $2
              AND membership.actor_id = $1 AND membership.enabled
              AND (membership.role = ANY($4) OR custom.key = ANY($4))
            UNION ALL
            SELECT 1 FROM organization_memberships membership
            LEFT JOIN workspace_roles custom ON custom.id = membership.role_id
            WHERE $2::uuid IS NULL AND membership.organization_id = $3
              AND membership.actor_id = $1 AND membership.enabled
              AND (membership.role = ANY($4) OR custom.key = ANY($4))
        )
        "#,
    )
    .bind(account_id)
    .bind(row.space_id)
    .bind(row.workspace_id)
    .bind(&row.addressed_role_keys)
    .fetch_one(&mut **transaction)
    .await?)
}

/// The space a request lives in, unlocked and unchecked: only for holding
/// its workspace open before the request's row is locked.
pub async fn space_of(
    executor: impl sqlx::PgExecutor<'_>,
    request_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT space_id FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(executor)
            .await?
            .flatten(),
    )
}

/// Who may answer an open request they can see: an account it names; else
/// a holder of one of the roles it names who has its permission; else,
/// when it names nobody, anyone with its permission there. Never the
/// account that raised it.
pub async fn may_answer(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &crate::access::Caller,
    row: &RequestRow,
) -> Result<bool, AppError> {
    if row.status != "open" || row.raised_by_account_id == caller.account_id {
        return Ok(false);
    }
    // Being asked by name still needs what answering it needs now: a
    // sponsor who has lost `runs.approve` no longer approves their run.
    if row.addressed_account_ids.contains(&caller.account_id) {
        return holds(transaction, caller, row, true).await;
    }
    if !row.addressed_role_keys.is_empty() {
        return Ok(
            holds_addressed_role(transaction, caller.account_id, row).await?
                && holds(transaction, caller, row, true).await?,
        );
    }
    if !row.addressed_account_ids.is_empty() {
        return Ok(false);
    }
    holds(transaction, caller, row, true).await
}

/// Why an answer was refused.
#[derive(Debug)]
pub enum AnswerError {
    Invalid {
        field: &'static str,
        message: &'static str,
    },
    /// Not open any more: answered, expired or withdrawn.
    Settled(String),
    NotYours,
    App(AppError),
}

impl From<AppError> for AnswerError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for AnswerError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

/// The answer to store: an option of the request, words, or both.
fn clean_answer(
    row: &RequestRow,
    option_id: Option<&str>,
    typed: Option<&str>,
) -> Result<Value, AnswerError> {
    let options = options_of(&row.options);
    let option_id = match option_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(picked) => Some(
            options
                .iter()
                .find(|option| option.id == picked)
                .map(|option| option.id.clone())
                .ok_or(AnswerError::Invalid {
                    field: "optionId",
                    message: "Pick one of the request's options by its id.",
                })?,
        ),
        None => None,
    };
    let typed = typed.map(str::trim).filter(|typed| !typed.is_empty());
    if typed.is_some() && !row.allow_text {
        return Err(AnswerError::Invalid {
            field: "text",
            message: "This request takes one of its options, not a typed answer.",
        });
    }
    if typed.is_some_and(|typed| {
        typed.chars().count() > MAX_ANSWER_CHARS
            || typed
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
    }) {
        return Err(AnswerError::Invalid {
            field: "text",
            message: "Keep the answer to 4,000 characters; line breaks are fine.",
        });
    }
    if option_id.is_none() && typed.is_none() {
        return Err(AnswerError::Invalid {
            field: "optionId",
            message: "Pick an option (optionId), type an answer (text), or both.",
        });
    }
    let mut answer = serde_json::Map::new();
    if let Some(option_id) = option_id {
        answer.insert("optionId".into(), Value::String(option_id));
    }
    if let Some(typed) = typed {
        answer.insert("text".into(), Value::String(typed.to_owned()));
    }
    Ok(Value::Object(answer))
}

/// Answer an open request as `caller`, once. Its notices are marked read,
/// its raiser is told, and a run waiting on it is told through its host.
pub async fn answer(
    transaction: &mut Transaction<'_, Postgres>,
    state: &crate::AppState,
    caller: &crate::access::Caller,
    row: &RequestRow,
    option_id: Option<&str>,
    typed: Option<&str>,
) -> Result<RequestRow, AnswerError> {
    if row.status != "open" {
        return Err(AnswerError::Settled(format!(
            "This request is {} already.",
            match row.status.as_str() {
                "answered" => "answered",
                "expired" => "past its time",
                _ => "withdrawn",
            }
        )));
    }
    if !may_answer(transaction, caller, row).await? {
        return Err(AnswerError::NotYours);
    }
    if raiser_left(transaction, row).await? {
        return Err(AnswerError::Settled(
            "The one who asked no longer has access here, so it can't be answered.".to_owned(),
        ));
    }
    let answer = clean_answer(row, option_id, typed)?;
    let settled = settle(
        transaction,
        state,
        row.id,
        "answered",
        Some(&answer),
        Some(caller.account_id),
    )
    .await?
    .ok_or_else(|| AnswerError::Settled("This request was settled meanwhile.".to_owned()))?;
    Ok(settled)
}

/// Whether the account that raised the request has since lost the
/// workspace (A1.8): its membership there was turned off. A host, which
/// raises requests without being a member, never has.
async fn raiser_left(
    transaction: &mut Transaction<'_, Postgres>,
    row: &RequestRow,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM organization_memberships membership
            LEFT JOIN actors account ON account.id = membership.actor_id
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND (NOT membership.enabled OR account.status IS DISTINCT FROM 'active')
        )
        "#,
    )
    .bind(row.workspace_id)
    .bind(row.raised_by_account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Withdraw an open request its raiser no longer needs.
pub async fn cancel(
    transaction: &mut Transaction<'_, Postgres>,
    state: &crate::AppState,
    account_id: Uuid,
    row: &RequestRow,
) -> Result<RequestRow, AnswerError> {
    if row.raised_by_account_id != account_id {
        return Err(AnswerError::NotYours);
    }
    if row.status != "open" {
        return Ok(row.clone());
    }
    settle(transaction, state, row.id, "cancelled", None, None)
        .await?
        .ok_or_else(|| AnswerError::Settled("This request was settled meanwhile.".to_owned()))
}

/// Move an open request to `status` and do what follows: its notices are
/// read, the raiser hears, and a run waiting on it goes on. `None` when it
/// was no longer open.
async fn settle(
    transaction: &mut Transaction<'_, Postgres>,
    state: &crate::AppState,
    request_id: Uuid,
    status: &str,
    answer: Option<&Value>,
    by: Option<Uuid>,
) -> Result<Option<RequestRow>, AppError> {
    let Some(row) = settle_only(transaction, request_id, status, answer, by).await? else {
        return Ok(None);
    };
    follow(transaction, state, &row, status, by).await?;
    Ok(Some(row))
}

/// [`settle`] leaving the run and what the request is about alone (a run
/// that is ending withdraws what it asked).
async fn settle_only(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    status: &str,
    answer: Option<&Value>,
    by: Option<Uuid>,
) -> Result<Option<RequestRow>, AppError> {
    let Some(row) = sqlx::query_as::<_, RequestRow>(&format!(
        r#"
        UPDATE requests request
           SET status = $2, answer = $3, answered_by_account_id = $4,
               answered_at = CASE WHEN $2 = 'answered' THEN now() END,
               version = request.version + 1, updated_at = now()
         WHERE request.id = $1 AND request.status = 'open'
        RETURNING {REQUEST_COLUMNS}
        "#
    ))
    .bind(request_id)
    .bind(status)
    .bind(answer)
    .bind(by)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    let ty = match status {
        "answered" => &crate::events::catalog::REQUEST_ANSWERED,
        "expired" => &crate::events::catalog::REQUEST_EXPIRED,
        _ => &crate::events::catalog::REQUEST_CANCELLED,
    };
    stage_event(
        transaction,
        ty,
        row.id,
        row.workspace_id,
        row.space_id,
        row.version,
        by.unwrap_or(row.raised_by_account_id),
    )
    .await?;
    // Its cards have been dealt with.
    let read = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        UPDATE notifications SET read_at = now(), updated_at = now()
         WHERE subject_kind = 'request' AND subject_id = $1 AND read_at IS NULL
        RETURNING project_id, recipient_actor_id
        "#,
    )
    .bind(row.id)
    .fetch_all(&mut **transaction)
    .await?;
    for (project_id, recipient) in read {
        crate::notifications::announce(transaction, project_id, recipient).await?;
    }
    if status != "cancelled" {
        tell_raiser(transaction, &row, by).await?;
    }
    Ok(Some(row))
}

/// What a settled request moves along: the sponsored run request it
/// approves, or the run waiting on it.
async fn follow(
    transaction: &mut Transaction<'_, Postgres>,
    state: &crate::AppState,
    row: &RequestRow,
    status: &str,
    by: Option<Uuid>,
) -> Result<(), AppError> {
    // A sponsored run request goes ahead or stops with its approval.
    if row.subject_kind.as_deref() == Some(crate::external_execution::REQUEST_SUBJECT) {
        crate::external_execution::approval_settled(state, transaction, row).await?;
    }
    // An agent from outside joins when an admin approves (A2.18).
    if row.subject_kind.as_deref() == Some(crate::accounts::join::JOIN_SUBJECT) {
        crate::accounts::join::settled(transaction, row).await?;
    }
    // An agent a host made is added when its approver says so (A2.19).
    if row.subject_kind.as_deref() == Some(crate::hosts::register::REGISTER_SUBJECT) {
        crate::hosts::register::settled(transaction, row, by).await?;
    }
    // A run waiting on it goes on, and its host (the built-in one too,
    // A1.16) hears the answer.
    if let Some(run_id) = row.run_id {
        let summary = match status {
            "answered" => "Answered.",
            "expired" => "No answer came in time.",
            _ => "Withdrawn.",
        };
        let run = crate::runs::ops::request_settled(
            transaction,
            run_id,
            row.id,
            by.unwrap_or(row.raised_by_account_id),
            summary,
        )
        .await
        .map_err(|error| match error {
            crate::runs::ops::RunOpError::App(error) => error,
            _ => AppError::Internal("request settled on its run"),
        })?;
        // A run on a host hears the answer through it.
        if status != "cancelled"
            && let Some(run) = run
            && let Some(host_id) = run.host_id
        {
            use crate::hosts::dispatch::Host;
            crate::hosts::dispatch::RemoteHost { id: host_id }
                .answer(transaction, &run, row.id)
                .await?;
        }
    }
    Ok(())
}

/// Stage a request's event in the log. It names the request and its status,
/// never its words; whoever may see the request may hear it.
async fn stage_event(
    transaction: &mut Transaction<'_, Postgres>,
    ty: &'static crate::events::EventType,
    request_id: Uuid,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    version: i64,
    actor: Uuid,
) -> Result<(), AppError> {
    use crate::events::{Audience, NewEvent};
    let event = NewEvent {
        id: Uuid::now_v7(),
        ty,
        workspace_id: Some(workspace_id),
        space_id,
        thing: Some((request_id, "request")),
        access_thing: None,
        version: Some(version),
        changes: &["status"],
        audience: Audience::Thing,
        payload: serde_json::json!({}),
        actor: Some(actor),
        legacy_type: None,
    };
    crate::events::emit(transaction, event).await?;
    Ok(())
}

/// The raiser hears that a request settled: a card in their inbox (in a
/// space) and a `request.answered` event on each of their connections. Both
/// name the request, never the answer: they read it with request_get.
async fn tell_raiser(
    transaction: &mut Transaction<'_, Postgres>,
    row: &RequestRow,
    by: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(space_id) = row.space_id else {
        return Ok(());
    };
    let notification = crate::notifications::NewNotification {
        project_id: space_id,
        recipient_actor_id: row.raised_by_account_id,
        kind: crate::notifications::kind::REQUEST_ANSWERED,
        subject_kind: "request",
        subject_id: row.id,
        actor_id: by,
        conversation_id: row.conversation_id,
        message_id: None,
        work_item_id: row.work_item_id,
        agent_run_id: row.run_id,
        pull_request_id: None,
        detail: serde_json::json!({}),
        group_key: format!("request:{}", row.id),
        dedupe_key: format!("request_answered:{}", row.id),
    };
    if crate::notifications::deliver(transaction, &notification).await? {
        crate::notifications::announce(transaction, space_id, row.raised_by_account_id).await?;
    }
    Ok(())
}

/// Put a new request in front of who it asks, in its space: the accounts it
/// names; else the holders of the roles it names who may answer it; else
/// whoever asked for the run that raised it.
pub async fn announce_raised(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<usize, AppError> {
    let Some(row) = get(transaction, request_id).await? else {
        return Ok(0);
    };
    let Some(space_id) = row.space_id else {
        return Ok(0);
    };
    let candidates = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT membership.actor_id, actor.kind
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        LEFT JOIN workspace_roles custom ON custom.id = membership.role_id
        WHERE membership.project_id = $1 AND membership.enabled AND membership.actor_id <> $2
          AND (membership.actor_id = ANY($3)
               OR (cardinality($3) = 0
                   AND (membership.role = ANY($4) OR custom.key = ANY($4)))
               OR (cardinality($3) = 0 AND cardinality($4) = 0 AND membership.actor_id = $6))
          AND request_visible_to($5, membership.actor_id)
        ORDER BY membership.created_at, membership.actor_id
        LIMIT 50
        "#,
    )
    .bind(space_id)
    .bind(row.raised_by_account_id)
    .bind(&row.addressed_account_ids)
    .bind(&row.addressed_role_keys)
    .bind(row.id)
    .bind(run_requester(transaction, &row).await?)
    .fetch_all(&mut **transaction)
    .await?;
    let mut told = 0;
    for (account_id, kind) in candidates {
        let named = row.addressed_account_ids.contains(&account_id);
        if !named {
            // By role: only those who may answer it.
            let Some(kind) = crate::ops::context::actor_kind_of(&kind) else {
                continue;
            };
            let caller = crate::access::Caller::person(account_id, kind);
            if !holds(transaction, &caller, &row, false).await? {
                continue;
            }
        }
        let href = match (row.conversation_id, row.work_item_id) {
            (None, Some(task_id)) => {
                crate::addresses::Target::task(space_id, task_id).legacy_path()
            }
            _ => crate::addresses::Target::Inbox.legacy_path(),
        };
        let notification = crate::notifications::NewNotification {
            project_id: space_id,
            recipient_actor_id: account_id,
            kind: crate::notifications::kind::REQUEST_RAISED,
            subject_kind: "request",
            subject_id: row.id,
            actor_id: Some(row.raised_by_account_id),
            conversation_id: row.conversation_id,
            message_id: None,
            work_item_id: row.work_item_id,
            agent_run_id: row.run_id,
            pull_request_id: None,
            detail: serde_json::json!({ "href": href }),
            group_key: format!("request:{}", row.id),
            dedupe_key: format!("request:{}", row.id),
        };
        if crate::notifications::deliver(transaction, &notification).await? {
            crate::notifications::announce(transaction, space_id, account_id).await?;
            told += 1;
        }
    }
    Ok(told)
}

/// Who asked for the run that raised a request, if anyone.
async fn run_requester(
    transaction: &mut Transaction<'_, Postgres>,
    row: &RequestRow,
) -> Result<Option<Uuid>, AppError> {
    let Some(run_id) = row.run_id else {
        return Ok(None);
    };
    crate::runs::ops::requested_by(&mut **transaction, run_id).await
}

/// Requests past their time: `deny` ones are answered with their deny
/// option, the rest expire. Run by the job queue every minute.
pub async fn expire_due(state: &crate::AppState) -> Result<usize, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let due = sqlx::query_as::<_, RequestRow>(&format!(
        r#"
        SELECT {REQUEST_COLUMNS} FROM requests request
        WHERE request.status = 'open' AND request.expires_at < now()
        ORDER BY request.expires_at
        LIMIT 200
        FOR UPDATE SKIP LOCKED
        "#
    ))
    .fetch_all(&mut *transaction)
    .await?;
    let mut settled = 0;
    for row in &due {
        let deny = (row.on_expiry == "deny")
            .then(|| {
                deny_option(row.options.as_array().map_or(&[][..], Vec::as_slice))
                    .and_then(|option| option.get("id").cloned())
            })
            .flatten();
        let done = match deny {
            Some(option_id) => {
                settle(
                    &mut transaction,
                    state,
                    row.id,
                    "answered",
                    Some(&serde_json::json!({"optionId": option_id, "expired": true})),
                    None,
                )
                .await?
            }
            None => settle(&mut transaction, state, row.id, "expired", None, None).await?,
        };
        settled += usize::from(done.is_some());
    }
    transaction.commit().await?;
    Ok(settled)
}

const EXPIRE_KIND: &str = "requests.expire_due";

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(EXPIRE_KIND, |context| async move {
        expire_due(&context.state).await?;
        Ok(())
    });
    registry.every(EXPIRE_KIND, std::time::Duration::from_secs(60));
}

/// A request on an inbox card.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCard {
    pub id: Uuid,
    /// question, approval or proposal.
    pub kind: String,
    /// open, answered, expired, cancelled or superseded.
    pub status: String,
    pub title: String,
    pub options: Vec<RequestOption>,
    pub allow_text: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<RequestAnswerView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
}
