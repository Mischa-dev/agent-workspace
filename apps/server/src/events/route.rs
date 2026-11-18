//! W14: the workspace events automations (X5) hear about, read from the
//! event log (K3.4). A person's outgoing webhooks are connections now, told
//! by `delivery::push` (A2.6).
//!
//! The [`Router`] consumer reads the log after its own cursor. For each
//! event whose project has an event-triggered rule, it queues one
//! `events.route` job in the same transaction that advances its cursor, so
//! a router that stops mid-batch loses and repeats nothing; the job turns
//! the event (IDs only) into the events below and hands them to the rules.
//! Everything after that goes through the job queue, so it survives
//! restarts and runs on any server. The router runs under its
//! `consumer:router` lease on whichever instance holds it.
//!
//! Each event has a key that names what happened (`run:<id>:completed`), so
//! the same change seen twice, from a retried batch or from two events
//! about one pull request, is delivered and acted on once.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    domain::RunState,
    error::AppError,
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    mcp::McpPrincipal,
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
    ("record.created", "A record was added to a collection"),
    ("record.updated", "A record in a collection changed"),
    ("page.updated", "A page was saved as a new version"),
    ("sheet.updated", "A sheet's cells, tabs or name changed"),
    (
        "whiteboard.updated",
        "A whiteboard was saved as a new version",
    ),
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
    super::upkeep::register_jobs(registry);
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
    /// A collection's record (AWD-110).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<Uuid>,
    /// A page, sheet or whiteboard (K3.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whiteboard_id: Option<Uuid>,
    /// For task.updated and record.updated: what changed.
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
    /// The thing it is about, as the registry names it (K3.4). The
    /// `subject` fields stay beside it for one release, for receivers that
    /// read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing: Option<ThingRef>,
    pub subject: Subject,
}

/// A thing by kind and id.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThingRef {
    pub kind: String,
    pub id: Uuid,
}

/// The thing an event is about, from its subject.
fn thing_of(event_type: &str, subject: &Subject) -> Option<ThingRef> {
    let thing = |kind: &str, id: Option<Uuid>| {
        id.map(|id| ThingRef {
            kind: kind.to_owned(),
            id,
        })
    };
    match event_type.split('.').next() {
        Some("message") => thing("message", subject.message_id),
        Some("run") => thing("run", subject.agent_run_id),
        Some("pull_request") => thing("pull_request", subject.pull_request_id),
        Some("record") => thing("record", subject.record_id),
        Some("task") => thing("task", subject.work_item_id),
        Some("page") => thing("page", subject.page_id),
        Some("sheet") => thing("sheet", subject.sheet_id),
        Some("whiteboard") => thing("whiteboard", subject.whiteboard_id),
        _ => None,
    }
}

/// One logged event's routing job: the events webhooks and rules hear for
/// it, made when the router read it. IDs only.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutePayload {
    pub event_id: Uuid,
    pub project_id: Uuid,
    pub organization_id: Uuid,
    pub events: Vec<Event>,
}

/// The events webhooks and rules hear for one logged event; none for most.
/// Writers emit these facts themselves (K3.7), so this only renames them
/// into the webhook shape: a run that ended is heard as `run.updated` and
/// `run.finished`, and each event gets the key that names what happened.
async fn heard(
    connection: &mut sqlx::PgConnection,
    logged: &super::LoggedEvent,
) -> Result<Vec<Event>, AppError> {
    let Some(project_id) = logged.space_id else {
        return Ok(Vec::new());
    };
    let id_in = |key: &str| {
        logged
            .payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
    };
    let text_in = |key: &str| {
        logged
            .payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let event = |event_type: &str, key: String, actor_id: Option<Uuid>, subject: Subject| Event {
        event_type: event_type.to_owned(),
        key,
        organization_id: logged.workspace_id,
        project_id,
        occurred_at: logged.created_at,
        actor_id,
        thing: thing_of(event_type, &subject),
        subject,
    };
    let Some(thing) = logged.thing_id else {
        return Ok(Vec::new());
    };
    let event_type = logged.event_type.as_str();
    Ok(match event_type {
        "message.created" => vec![event(
            event_type,
            format!("message:{thing}"),
            id_in("authorActorId").or(logged.actor_account_id),
            Subject {
                conversation_id: id_in("conversationId"),
                message_id: Some(thing),
                ..Subject::default()
            },
        )],
        // A page, sheet or board kept as a new version (K3.5): one per
        // version, so a redelivered event never fires twice.
        "page.updated" | "sheet.updated" | "whiteboard.updated" => {
            let version = logged.thing_version.unwrap_or_default();
            let mut subject = Subject::default();
            match event_type {
                "page.updated" => subject.page_id = Some(thing),
                "sheet.updated" => subject.sheet_id = Some(thing),
                _ => subject.whiteboard_id = Some(thing),
            }
            vec![event(
                event_type,
                format!("{event_type}:{thing}:{version}"),
                logged.actor_account_id,
                subject,
            )]
        }
        "task.created" => vec![event(
            event_type,
            format!("task.created:{thing}"),
            logged.actor_account_id,
            Subject {
                work_item_id: Some(thing),
                ..Subject::default()
            },
        )],
        "task.updated" => vec![event(
            event_type,
            format!(
                "task.updated:{thing}:{}",
                logged.thing_version.unwrap_or_default()
            ),
            logged.actor_account_id,
            Subject {
                work_item_id: Some(thing),
                changed_fields: logged.changes.clone(),
                ..Subject::default()
            },
        )],
        "record.created" | "record.updated" => vec![event(
            event_type,
            format!("record:{}", logged.id),
            logged.actor_account_id,
            Subject {
                record_id: Some(thing),
                collection_id: id_in("collectionId"),
                changed_fields: logged.changes.clone(),
                ..Subject::default()
            },
        )],
        "pull_request.merged"
        | "pull_request.closed"
        | "pull_request.checks_failed"
        | "pull_request.checks_passed" => {
            let state = text_in("state").unwrap_or_default();
            let key = match event_type {
                "pull_request.merged" | "pull_request.closed" => format!("pr:{thing}:{state}"),
                _ => format!(
                    "pr:{thing}:{}:{state}",
                    text_in("headSha").unwrap_or_default()
                ),
            };
            vec![event(
                event_type,
                key,
                None,
                Subject {
                    pull_request_id: Some(thing),
                    work_item_id: id_in("workItemId"),
                    agent_run_id: id_in("agentRunId"),
                    state: Some(state),
                    ..Subject::default()
                },
            )]
        }
        // The conversation's run (its own notice); A1's run record is
        // heard through it until runs move to one record (lane E).
        "run.updated" | "run.finished"
            if logged.legacy_type.as_deref() == Some("conversation.agent_run.updated") =>
        {
            let Some(state) = text_in("state").and_then(|state| state.parse::<RunState>().ok())
            else {
                return Ok(Vec::new());
            };
            let (requester, work_item_id) =
                crate::runs::reads::requester_and_task(&mut *connection, thing)
                    .await?
                    .unwrap_or((Uuid::nil(), None));
            let actor = (!requester.is_nil()).then_some(requester);
            let state_name = state.to_string();
            let subject = Subject {
                agent_run_id: Some(thing),
                conversation_id: id_in("conversationId"),
                work_item_id,
                state: Some(state_name.clone()),
                ..Subject::default()
            };
            let mut events = vec![event(
                "run.updated",
                format!("run:{thing}:{state_name}"),
                actor,
                subject.clone(),
            )];
            if state.is_terminal() {
                events.push(event(
                    "run.finished",
                    format!("run.finished:{thing}"),
                    actor,
                    subject,
                ));
            }
            events
        }
        _ => Vec::new(),
    })
}

/// The project's workspace, when a rule in the project is triggered by its
/// events.
async fn listening(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.organization_id
        FROM projects project
        WHERE project.id = $1
          AND EXISTS (
              SELECT 1 FROM automation_rules rule
              WHERE rule.project_id = project.id AND rule.enabled
                AND rule.trigger_kind NOT IN ('schedule', 'capacity_window')
          )
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The router: a consumer of the log that queues one `events.route` job
/// per event that has somewhere to go.
pub struct Router;

/// The router's cursor.
pub const ROUTER: &str = "router";

#[async_trait::async_trait]
impl super::Consumer for Router {
    fn name(&self) -> &'static str {
        ROUTER
    }

    /// From the log's start, so nothing written before the router's first
    /// round is skipped.
    fn starts_at_head(&self) -> bool {
        false
    }

    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        batch: &[super::LoggedEvent],
    ) -> Result<(), super::ConsumerError> {
        for event in batch {
            let Some(project_id) = event.space_id else {
                continue;
            };
            if !is_event_type(&event.event_type) {
                continue;
            }
            let Some(organization_id) = listening(transaction, project_id).await? else {
                continue;
            };
            let events = heard(transaction, event).await?;
            if events.is_empty() {
                continue;
            }
            jobs::enqueue(
                &mut **transaction,
                NewJob::new(ROUTE_KIND)
                    .project(organization_id, project_id)
                    .payload(RoutePayload {
                        event_id: event.id,
                        project_id,
                        organization_id,
                        events,
                    })
                    .unique(format!("events.route:{}", event.id)),
            )
            .await?;
        }
        Ok(())
    }
}

/// Hands one logged event's events to the automations that listen.
pub(crate) async fn route(state: &AppState, payload: &RoutePayload) -> Result<usize, AppError> {
    let events = &payload.events;
    // AWD-75: with Automations off, no rule follows the workspace's events;
    // nor do events of a module that's off. A space that turns a module off
    // narrows it for its own events (AWD-186).
    let modules = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        crate::modules::state::in_space(
            &mut connection,
            payload.organization_id,
            payload.project_id,
        )
        .await?
    };
    if !modules.contains("automations") {
        return Ok(0);
    }
    for event in events {
        let direct =
            sqlx::query_scalar::<_, bool>("SELECT kind = 'direct' FROM projects WHERE id = $1")
                .bind(event.project_id)
                .fetch_one(crate::db::scoped(&state.pool))
                .await?;
        if shut_out(modules, &event.event_type, direct) {
            continue;
        }
        crate::automations::on_event(state, event).await?;
    }
    Ok(events.len())
}

/// AWD-75: whether `modules` leave an event out: Automations is off, or
/// the event's own module is. A conversation's event follows Channels in a
/// project and Messages in the direct space, as conversations do everywhere
/// else.
pub(crate) fn shut_out(modules: crate::modules::ModuleSet, event_type: &str, direct: bool) -> bool {
    !modules.contains("automations")
        || crate::automations::in_project_kind(
            crate::modules::manifest::of_event(event_type),
            direct,
        )
        .is_some_and(|module| !modules.contains(&module.id))
}

// ---------------------------------------------------------------------------
// Who sets up a rule

/// The person, app or agent making a rule, which then acts as
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

    #[test]
    fn events_follow_automations_and_their_own_module() {
        use crate::modules::ModuleSet;
        let all_on = ModuleSet::from_switches(|_| false);
        assert!(!super::shut_out(all_on, "task.updated", false));
        assert!(!super::shut_out(all_on, "message.created", false));
        let automations_off = ModuleSet::from_switches(|module| module == "automations");
        assert!(super::shut_out(automations_off, "task.updated", false));
        let tasks_off = ModuleSet::from_switches(|module| module == "tasks");
        assert!(super::shut_out(tasks_off, "task.updated", false));
        // A channel's message follows Channels; a DM's, Messages.
        let channels_off = ModuleSet::from_switches(|module| module == "channels");
        assert!(super::shut_out(channels_off, "message.created", false));
        assert!(!super::shut_out(channels_off, "message.created", true));
    }
}
