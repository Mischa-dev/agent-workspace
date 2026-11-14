//! A1.7: requests, something an account needs from someone: an approval, an
//! answer or acceptance of a proposal (A1 section 3.8). This holds the
//! record; the operations to raise, answer and withdraw one by role come
//! in A1.8. Until then a request is opened by a run's `interrupt` activity
//! (or copied from the old approval and question tables, kept equal by
//! `legacy`).

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, runs::store::RunRow};

pub mod legacy;

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
    request.legacy_source, request.version, request.created_at, request.updated_at
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
    pub legacy_source: Option<String>,
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
    const FIELD: &str = "activities.payload.request";
    let client_request_id = prose(request.get("clientRequestId"));
    if client_request_id
        .as_deref()
        .is_some_and(|value| value.chars().count() > 200 || value.chars().any(char::is_control))
    {
        return Err(refused(
            FIELD,
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
    .bind(run.id)
    .bind(activity_ref)
    .bind(run.account_id)
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
        Some(_) => {
            return Err(refused(
                FIELD,
                "A run raises a question or an approval (request.kind).",
            ));
        }
    };
    let title = prose(request.get("title"))
        .ok_or_else(|| refused(FIELD, "Say what you need in request.title."))?;
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(refused(FIELD, "Keep request.title to 2,000 characters."));
    }
    let detail = prose(request.get("detail"));
    if detail
        .as_deref()
        .is_some_and(|detail| detail.chars().count() > MAX_DETAIL_CHARS)
    {
        return Err(refused(FIELD, "Keep request.detail to 8,000 characters."));
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
                    return Err(refused(FIELD, "Each option has an id and a label."));
                };
                if id.chars().count() > 40
                    || label.chars().count() > 200
                    || style
                        .as_deref()
                        .is_some_and(|style| !matches!(style, "approve" | "deny"))
                {
                    return Err(refused(
                        FIELD,
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
        Some(_) => return Err(refused(FIELD, "Give at most 8 options.")),
    };
    let allow_text = request
        .get("allowText")
        .and_then(Value::as_bool)
        .unwrap_or(kind == "question" || options.is_empty());
    if options.is_empty() && !allow_text {
        return Err(refused(
            FIELD,
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
            .ok_or_else(|| refused(FIELD, "Name people and agents in audience.accounts by ID."))?,
        Some(_) => {
            return Err(refused(
                FIELD,
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
            .ok_or_else(|| refused(FIELD, "Name roles in audience.roles by key, like owner."))?,
        Some(_) => return Err(refused(FIELD, "Name at most 10 roles in audience.roles.")),
    };
    let Some(space_id) = run.space_id else {
        return Err(refused(
            FIELD,
            "This run isn't in a project, so it can't ask anyone yet.",
        ));
    };
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
        .bind(run.account_id)
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
                FIELD,
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
                FIELD,
                "expiresInSeconds is 60 (a minute) to 604,800 (a week).",
            ));
        }
    };

    // Limits, counted under a lock on the run so two batches count each other.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('requests_run:' || $1::text, 0))")
        .bind(run.id)
        .execute(&mut **transaction)
        .await?;
    let open_on_run = open_count(transaction, run.id).await?;
    if open_on_run >= MAX_OPEN_PER_RUN {
        return Err(refused(
            FIELD,
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
        .bind(run.account_id)
        .bind(&accounts)
        .bind(MAX_OPEN_PER_ADDRESSEE)
        .fetch_one(&mut **transaction)
        .await?;
        if crowded {
            return Err(refused(
                FIELD,
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
            expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
        "#,
    )
    .bind(request_id)
    .bind(run.workspace_id)
    .bind(space_id)
    .bind(kind)
    .bind(run.account_id)
    .bind(run.id)
    .bind(activity_ref)
    .bind(&client_request_id)
    .bind(run.conversation_id)
    .bind(run.work_item_id)
    .bind(&title)
    .bind(&detail)
    .bind(Value::Array(options))
    .bind(allow_text)
    .bind(if kind == "approval" {
        "runs.approve"
    } else {
        "requests.answer"
    })
    .bind(&accounts)
    .bind(&roles)
    .bind(expires_at)
    .execute(&mut **transaction)
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

/// A run finished: what it still asked for is withdrawn.
pub async fn withdraw_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE requests
           SET status = 'cancelled', version = version + 1, updated_at = now()
         WHERE run_id = $1 AND status = 'open' AND legacy_source IS NULL
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
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
         WHERE raised_via_host_id = $1 AND status = 'open' AND legacy_source IS NULL
        "#,
    )
    .bind(host_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}
