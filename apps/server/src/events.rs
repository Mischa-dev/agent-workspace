//! W14: the workspace events outgoing webhooks (X4) and automations (X5)
//! hear about, drawn from the outbox.
//!
//! The outbox publisher calls [`queue_route`] in the transaction that marks
//! an event published, as it does for notifications. When the event's
//! project has a webhook or an event-triggered rule, that queues one
//! `events.route` job; the job turns the outbox event (IDs only) into the
//! events below and hands them to both. Everything after that goes through
//! the job queue, so it survives restarts and runs on any server.
//!
//! Each event has a key that names what happened (`run:<id>:completed`), so
//! the same change seen twice, from a redelivered outbox event or from two
//! events about one pull request, is delivered and acted on once.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    domain::RunState,
    error::AppError,
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    mcp::McpPrincipal,
    outbox::{ClaimedOutboxEvent, OutboxEventPayload},
    policy::ActorKind,
};

pub const ROUTE_KIND: &str = "events.route";

/// Every event type, as webhooks name them.
pub const EVENT_TYPES: &[(&str, &str)] = &[
    ("task.created", "A task was created"),
    ("task.updated", "A task changed"),
    ("run.updated", "A run changed state"),
    ("run.finished", "A run finished, failed or was stopped"),
    (
        "pull_request.checks_failed",
        "Checks failed on a linked pull request",
    ),
    (
        "pull_request.checks_passed",
        "Checks passed on a linked pull request",
    ),
    ("pull_request.merged", "A linked pull request was merged"),
    ("pull_request.closed", "A linked pull request was closed"),
    ("message.created", "A message was posted"),
];

pub fn is_event_type(value: &str) -> bool {
    EVENT_TYPES.iter().any(|(name, _)| *name == value)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(ROUTE_KIND, |context: JobContext| async move {
        let payload: RoutePayload = context.job.payload_as()?;
        route(&context.state, &payload)
            .await
            .map_err(|error| JobError::retry(error.log_category()))?;
        Ok(())
    });
}

/// What one event is about. IDs only.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_id: Option<Uuid>,
    /// A run's state, or a pull request's checks state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// For task.updated: what changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
}

/// One thing that happened in a project.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub event_type: String,
    /// Names what happened; the same change always has the same key.
    pub key: String,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    /// Who caused it, when someone did.
    pub actor_id: Option<Uuid>,
    pub subject: Subject,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutePayload {
    pub event_id: Uuid,
    pub project_id: Uuid,
    pub organization_id: Uuid,
    pub source: RouteSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RouteSource {
    Message {
        conversation_id: Uuid,
        message_id: Uuid,
        author_actor_id: Uuid,
    },
    Run {
        conversation_id: Uuid,
        agent_run_id: Uuid,
        state: RunState,
    },
    WorkItem {
        work_item_id: Uuid,
        version: i64,
    },
}

/// Queues the routing job for a published outbox event, when anything in
/// its project listens. Called by the outbox publisher inside the
/// transaction that marks the event published.
pub async fn queue_route(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &ClaimedOutboxEvent,
) -> Result<bool, AppError> {
    let Ok(payload) = claim.decode() else {
        return Ok(false);
    };
    let source = match payload {
        OutboxEventPayload::MessageCreated(created) => RouteSource::Message {
            conversation_id: created.conversation_id,
            message_id: created.message_id,
            author_actor_id: created.author_actor_id,
        },
        OutboxEventPayload::AgentRunUpdated(updated) => match updated.state {
            Some(state) => RouteSource::Run {
                conversation_id: updated.conversation_id,
                agent_run_id: updated.agent_run_id,
                state,
            },
            None => return Ok(false),
        },
        OutboxEventPayload::WorkItemUpdated(updated) => RouteSource::WorkItem {
            work_item_id: updated.work_item_id,
            version: updated.version,
        },
        _ => return Ok(false),
    };
    let project_id = claim.project_id();
    let listening = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.organization_id
        FROM projects project
        WHERE project.id = $1
          AND (
              EXISTS (
                  SELECT 1 FROM webhook_subscriptions subscription
                  WHERE subscription.organization_id = project.organization_id
                    AND subscription.state = 'active'
                    AND (subscription.project_id IS NULL OR subscription.project_id = project.id)
              )
              OR EXISTS (
                  SELECT 1 FROM automation_rules rule
                  WHERE rule.project_id = project.id AND rule.enabled
                    AND rule.trigger_kind NOT IN ('schedule', 'capacity_window')
              )
          )
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(organization_id) = listening else {
        return Ok(false);
    };
    jobs::enqueue(
        &mut **transaction,
        NewJob::new(ROUTE_KIND)
            .project(organization_id, project_id)
            .payload(RoutePayload {
                event_id: claim.event_id(),
                project_id,
                organization_id,
                source,
            })
            .unique(format!("events.route:{}", claim.event_id())),
    )
    .await?;
    Ok(true)
}

/// Turns one outbox event into events and hands them to webhooks and
/// automations. Public so tests can route without the job queue.
pub(crate) async fn route(state: &AppState, payload: &RoutePayload) -> Result<usize, AppError> {
    let events = expand(&state.pool, payload).await?;
    for event in &events {
        crate::webhooks::fan_out(state, event).await?;
        crate::automations::on_event(state, event).await?;
    }
    Ok(events.len())
}

// ---------------------------------------------------------------------------
// Who sets up a webhook or a rule

/// The person, app or agent making a webhook or rule, which then acts as
/// them. Resolved on the server from the session or the bearer token.
#[derive(Clone, Debug)]
pub(crate) struct Caller {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    pub session_id: Option<Uuid>,
    /// The connected app a person acts through.
    pub via_grant_id: Option<Uuid>,
    pub external_agent_connection_id: Option<Uuid>,
    /// A token that reaches one project only.
    pub token_project_id: Option<Uuid>,
    pub can_write: bool,
    pub channel: &'static str,
    pub authentication_class: &'static str,
    pub client_name: Option<String>,
}

impl Caller {
    pub(crate) fn browser(principal: &AuthenticatedActor) -> Result<Self, AppError> {
        Ok(Self {
            actor_id: principal.actor_id,
            actor_kind: crate::environments::principal_kind(principal)?,
            session_id: Some(principal.session_id),
            via_grant_id: None,
            external_agent_connection_id: None,
            token_project_id: None,
            can_write: true,
            channel: "browser",
            authentication_class: "password_session",
            client_name: None,
        })
    }

    pub(crate) fn mcp(principal: &McpPrincipal) -> Self {
        let account_wide = principal.actor_kind == ActorKind::Human && principal.account_wide;
        Self {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind,
            session_id: None,
            via_grant_id: principal.via_grant_id(),
            external_agent_connection_id: principal.external_agent_connection_id,
            // Agents reach the projects their connection allows; a person's
            // token or project-scoped grant reaches its one project.
            token_project_id: (!account_wide && principal.actor_kind == ActorKind::Human)
                .then_some(principal.project_id),
            can_write: principal.can_write(),
            channel: "mcp",
            authentication_class: principal.authentication_class,
            client_name: Some(principal.client_name.chars().take(80).collect()),
        }
    }

    /// Whether the caller may act in `project_id` at all through this token.
    pub(crate) fn reaches(&self, project_id: Uuid) -> bool {
        self.token_project_id.is_none_or(|only| only == project_id)
    }
}

/// One audit row for a webhook or rule change.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit(
    executor: impl sqlx::PgExecutor<'_>,
    caller: &Caller,
    organization_id: Uuid,
    project_id: Option<Uuid>,
    action: &'static str,
    target_kind: &'static str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, organization_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, mcp_oauth_grant_id,
            channel, client_name
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'succeeded', $10, $11, $12, $13)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(caller.actor_id)
    .bind(organization_id)
    .bind(project_id)
    .bind(caller.session_id)
    .bind(caller.authentication_class)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(metadata)
    .bind(caller.via_grant_id)
    .bind(caller.channel)
    .bind(&caller.client_name)
    .execute(executor)
    .await?;
    Ok(())
}

/// Where people open the app, for links in payloads and messages.
pub(crate) fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}

#[derive(FromRow)]
struct ActivityRow {
    kind: String,
    actor_id: Option<Uuid>,
    changed_fields: Option<serde_json::Value>,
}

#[derive(FromRow)]
struct LinkedPullRequest {
    id: Uuid,
    head_sha: String,
    state: String,
    checks_state: String,
}

async fn expand(pool: &PgPool, payload: &RoutePayload) -> Result<Vec<Event>, AppError> {
    let occurred_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT created_at FROM outbox_events WHERE event_id = $1",
    )
    .bind(payload.event_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(Utc::now);
    let event = |event_type: &str, key: String, actor_id: Option<Uuid>, subject: Subject| Event {
        event_type: event_type.to_owned(),
        key,
        organization_id: payload.organization_id,
        project_id: payload.project_id,
        occurred_at,
        actor_id,
        subject,
    };
    let mut events = Vec::new();
    match &payload.source {
        RouteSource::Message {
            conversation_id,
            message_id,
            author_actor_id,
        } => {
            events.push(event(
                "message.created",
                format!("message:{message_id}"),
                Some(*author_actor_id),
                Subject {
                    conversation_id: Some(*conversation_id),
                    message_id: Some(*message_id),
                    ..Subject::default()
                },
            ));
        }
        RouteSource::Run {
            conversation_id,
            agent_run_id,
            state,
        } => {
            let (requester, work_item_id) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
                r#"
                SELECT run.requesting_actor_id,
                       (SELECT launch.work_item_id FROM launches launch
                        WHERE launch.agent_run_id = run.id LIMIT 1)
                FROM agent_runs run WHERE run.id = $1
                "#,
            )
            .bind(agent_run_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or((Uuid::nil(), None));
            let state_name = state.to_string();
            let subject = Subject {
                agent_run_id: Some(*agent_run_id),
                conversation_id: Some(*conversation_id),
                work_item_id,
                state: Some(state_name.to_owned()),
                ..Subject::default()
            };
            let actor = (!requester.is_nil()).then_some(requester);
            events.push(event(
                "run.updated",
                format!("run:{agent_run_id}:{state_name}"),
                actor,
                subject.clone(),
            ));
            if state.is_terminal() {
                events.push(event(
                    "run.finished",
                    format!("run.finished:{agent_run_id}"),
                    actor,
                    subject,
                ));
            }
            events.extend(
                linked_pull_requests(pool, payload, PullRequestLink::Run(*agent_run_id))
                    .await?
                    .into_iter()
                    .map(|(event_type, key, subject)| {
                        let subject = Subject {
                            agent_run_id: Some(*agent_run_id),
                            work_item_id,
                            ..subject
                        };
                        event(event_type, key, None, subject)
                    }),
            );
        }
        RouteSource::WorkItem {
            work_item_id,
            version,
        } => {
            let activities = sqlx::query_as::<_, ActivityRow>(
                r#"
                SELECT activity.kind, activity.actor_id,
                       activity.payload->'changedFields' AS changed_fields
                FROM outbox_events event
                JOIN work_item_activity activity
                  ON activity.project_id = event.project_id
                 AND activity.work_item_id = $2
                 AND activity.created_at = event.created_at
                WHERE event.event_id = $1
                ORDER BY activity.created_at, activity.id
                "#,
            )
            .bind(payload.event_id)
            .bind(work_item_id)
            .fetch_all(pool)
            .await?;
            let created = activities
                .iter()
                .find(|activity| activity.kind == "work_item_created");
            if let Some(activity) = created {
                events.push(event(
                    "task.created",
                    format!("task.created:{work_item_id}"),
                    activity.actor_id,
                    Subject {
                        work_item_id: Some(*work_item_id),
                        ..Subject::default()
                    },
                ));
            } else if let Some(activity) = activities.first() {
                let mut changed = activities
                    .iter()
                    .filter_map(|activity| activity.changed_fields.as_ref())
                    .filter_map(serde_json::Value::as_array)
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                for activity in &activities {
                    match activity.kind.as_str() {
                        "work_item_archived" => changed.push("archived".to_owned()),
                        "work_item_restored" => changed.push("restored".to_owned()),
                        _ => {}
                    }
                }
                changed.sort();
                changed.dedup();
                events.push(event(
                    "task.updated",
                    format!("task.updated:{work_item_id}:{version}"),
                    activity.actor_id,
                    Subject {
                        work_item_id: Some(*work_item_id),
                        changed_fields: changed,
                        ..Subject::default()
                    },
                ));
            }
            events.extend(
                linked_pull_requests(pool, payload, PullRequestLink::WorkItem(*work_item_id))
                    .await?
                    .into_iter()
                    .map(|(event_type, key, subject)| {
                        let subject = Subject {
                            work_item_id: Some(*work_item_id),
                            ..subject
                        };
                        event(event_type, key, None, subject)
                    }),
            );
        }
    }
    Ok(events)
}

enum PullRequestLink {
    Run(Uuid),
    WorkItem(Uuid),
}

/// Pull request events for the PRs linked to a run or task, keyed by what
/// they say (checks on a commit, merged, closed), so each lands once.
async fn linked_pull_requests(
    pool: &PgPool,
    payload: &RoutePayload,
    link: PullRequestLink,
) -> Result<Vec<(&'static str, String, Subject)>, AppError> {
    let rows = match link {
        PullRequestLink::Run(run_id) => {
            sqlx::query_as::<_, LinkedPullRequest>(
                r#"
                SELECT DISTINCT pr.id, pr.head_sha, pr.state, pr.checks_state
                FROM github_pull_request_workspace_links link
                JOIN github_pull_requests pr
                  ON pr.project_id = link.project_id AND pr.id = link.github_pull_request_id
                WHERE link.project_id = $1 AND link.agent_run_id = $2
                "#,
            )
            .bind(payload.project_id)
            .bind(run_id)
            .fetch_all(pool)
            .await?
        }
        PullRequestLink::WorkItem(work_item_id) => {
            sqlx::query_as::<_, LinkedPullRequest>(
                r#"
                SELECT DISTINCT pr.id, pr.head_sha, pr.state, pr.checks_state
                FROM github_pull_request_work_item_links link
                JOIN github_pull_requests pr
                  ON pr.project_id = link.project_id AND pr.id = link.github_pull_request_id
                WHERE link.project_id = $1 AND link.work_item_id = $2
                "#,
            )
            .bind(payload.project_id)
            .bind(work_item_id)
            .fetch_all(pool)
            .await?
        }
    };
    let mut events = Vec::new();
    for row in rows {
        let subject = |state: &str| Subject {
            pull_request_id: Some(row.id),
            state: Some(state.to_owned()),
            ..Subject::default()
        };
        match row.state.as_str() {
            "merged" => events.push((
                "pull_request.merged",
                format!("pr:{}:merged", row.id),
                subject("merged"),
            )),
            "closed" => events.push((
                "pull_request.closed",
                format!("pr:{}:closed", row.id),
                subject("closed"),
            )),
            _ => match row.checks_state.as_str() {
                "failure" => events.push((
                    "pull_request.checks_failed",
                    format!("pr:{}:{}:failure", row.id, row.head_sha),
                    subject("failure"),
                )),
                "success" => events.push((
                    "pull_request.checks_passed",
                    format!("pr:{}:{}:success", row.id, row.head_sha),
                    subject("success"),
                )),
                _ => {}
            },
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::{EVENT_TYPES, is_event_type};

    #[test]
    fn event_types_are_dotted_and_known() {
        for (name, description) in EVENT_TYPES {
            assert!(name.contains('.'), "{name}");
            assert!(!description.is_empty());
            assert!(is_event_type(name));
        }
        assert!(!is_event_type("task"));
        assert!(!is_event_type("*"));
    }
}
