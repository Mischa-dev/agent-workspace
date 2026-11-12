//! Orientation tools: `briefing` answers "what needs me?", `projects_list`
//! shows where the caller can work, and `project_open` gets them oriented in
//! one project.
//!
//! The briefing pushes pointers, not whole conversations: each item is one
//! line the caller can open. What must not be missed (direct asks, mentions)
//! comes first.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, accessible_project_ids, parse_id, project_role, public_url, truncate_text,
    },
    replies::{InboxItem, InboxListArguments, ReplyToolResult, inbox_list},
    talk::{ConversationSummary, ListFilter, list_rows, summary},
};
use crate::{
    AppState, error::AppError, layouts, memory, modules::state::ProjectModules, policy::ActorKind,
    work_item_labels, work_item_planning::HAS_OPEN_BLOCKER,
};

const GUIDE_CHARACTERS: usize = 20_000;
/// The most label names project_open lists.
const PROJECT_LABELS: i64 = 100;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BriefingArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// Only this project. Leave it out for everything.
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct Briefing {
    pub you: You,
    /// Agents only: messages people asked you to answer. Claim one with
    /// reply_claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_requests: Vec<InboxItem>,
    /// Unread notifications from your inbox: mentions, assignments,
    /// approvals, runs and pull requests. Change them with
    /// notification_update; notification_list shows the rest.
    pub notifications: Vec<super::notifications::NotificationLine>,
    /// Messages that @mention you and that you haven't read.
    pub mentions: Vec<Mention>,
    /// Conversations with messages you haven't read, most recent first.
    pub unread: Vec<ConversationSummary>,
    /// Open tasks assigned to you, most recently changed first.
    pub tasks: Vec<TaskLine>,
    /// Tasks you've claimed and are working on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<TaskLine>,
    /// Threads whose status or outcome changed in the last week.
    pub thread_updates: Vec<ThreadUpdate>,
    pub next_steps: &'static str,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct You {
    pub actor_id: String,
    pub name: String,
    pub handle: String,
    /// person or agent
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_through: Option<String>,
    /// full or read
    pub access: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct Mention {
    pub conversation_id: String,
    pub conversation: String,
    pub project: String,
    pub sequence: i64,
    pub message_id: String,
    pub author: String,
    pub text: String,
    pub sent_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskLine {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub project: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadUpdate {
    pub thread_id: String,
    pub name: String,
    pub status: String,
    pub outcome: Option<String>,
    pub under: String,
    pub changed_by: Option<String>,
    pub changed_at: String,
}

const NEXT_STEPS: &str = "Open anything here with conversation_open or task_open. Answer reply requests with reply_claim then reply_send. When a discussion settles, set the thread's status and outcome with thread_update or record a decision with decision_record, so nobody has to reread it. Claim a task with task_claim before working on it.";

pub(super) async fn briefing(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: BriefingArguments,
) -> ToolResult<Briefing> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let projects = if principal.project_explicit {
            vec![principal.project_id]
        } else {
            accessible_project_ids(state, principal).await?
        };
        // AWD-75: each section reads only where its module is on.
        let modules = {
            let mut connection = state.pool.acquire().await?;
            ProjectModules::load(&mut connection, principal.actor_id, &projects).await?
        };
        let talk_projects = modules.with_conversations(&projects);
        let task_projects = modules.with(&projects, "tasks");
        let (name, handle) = sqlx::query_as::<_, (String, String)>(
            "SELECT display_name, handle FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(principal.project_id)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or_else(|| (principal.client_name.clone(), String::new()));
        let reply_requests = if principal.actor_kind == ActorKind::ExternalAgent {
            match inbox_list(state, principal, InboxListArguments { limit: Some(20) }).await {
                ReplyToolResult::Ok { value } => value.requests,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let mentions = sqlx::query_as::<_, Mention>(
            r#"
            SELECT conversation.id::text AS conversation_id,
                   CASE WHEN conversation.kind = 'dm' THEN 'DM' ELSE conversation.name END
                       AS conversation,
                   CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END
                       AS project,
                   entry.sequence, message.id::text AS message_id,
                   COALESCE(author.display_name, 'Someone') AS author,
                   left(message.plain_text, 280) AS text,
                   message.created_at::text AS sent_at
            FROM message_mentions mention
            JOIN messages message ON message.id = mention.message_id
            JOIN conversations conversation ON conversation.id = message.conversation_id
            JOIN projects project ON project.id = conversation.project_id
            JOIN conversation_entries entry ON entry.message_id = message.id
            LEFT JOIN conversation_read_states read_state
              ON read_state.conversation_id = conversation.id AND read_state.actor_id = $1
            LEFT JOIN project_memberships author
              ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
            WHERE mention.actor_id = $1
              AND message.project_id = ANY($2)
              AND message.author_actor_id <> $1
              AND message.removed_at IS NULL
              AND message.created_at > now() - interval '30 days'
              AND entry.sequence > COALESCE(read_state.highest_seen_sequence, 0)
              AND conversation.archived_at IS NULL
              AND conversation_visible_to(conversation.id, $1)
            ORDER BY message.created_at DESC
            LIMIT 20
            "#,
        )
        .bind(principal.actor_id)
        .bind(&talk_projects)
        .fetch_all(&state.pool)
        .await?;
        let unread = list_rows(
            state,
            principal.actor_id,
            &talk_projects,
            ListFilter {
                kinds: None,
                with_person: None,
                unread_only: true,
                status: None,
                query: String::new(),
                only_id: None,
            },
            0,
            15,
        )
        .await?
        .into_iter()
        .map(|row| summary(state, principal.actor_id, row))
        .collect();
        let notifications =
            super::notifications::unread_lines(state, principal, &projects).await?;
        let tasks = task_lines(state, &task_projects, principal.actor_id, TaskFilter::Assigned).await?;
        let claims = task_lines(state, &task_projects, principal.actor_id, TaskFilter::Claimed).await?;
        let thread_updates = sqlx::query_as::<_, ThreadUpdate>(
            r#"
            SELECT thread.id::text AS thread_id, thread.name, thread.thread_status AS status,
                   thread.thread_outcome AS outcome,
                   CASE WHEN parent.kind = 'dm' THEN 'a DM' ELSE parent.name END AS under,
                   changer.display_name AS changed_by,
                   thread.thread_status_changed_at::text AS changed_at
            FROM conversations thread
            JOIN conversations parent ON parent.id = thread.parent_channel_id
            LEFT JOIN project_memberships changer
              ON changer.project_id = thread.project_id
             AND changer.actor_id = thread.thread_status_changed_by_actor_id
            WHERE thread.kind = 'thread'
              AND thread.project_id = ANY($2)
              AND thread.thread_status_changed_at > now() - interval '7 days'
              AND thread.thread_status_changed_by_actor_id IS DISTINCT FROM $1
              AND thread.archived_at IS NULL
              AND conversation_visible_to(thread.id, $1)
            ORDER BY thread.thread_status_changed_at DESC
            LIMIT 10
            "#,
        )
        .bind(principal.actor_id)
        .bind(&talk_projects)
        .fetch_all(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(Briefing {
            you: You {
                actor_id: principal.actor_id.to_string(),
                name,
                handle,
                kind: if principal.actor_kind == ActorKind::ExternalAgent {
                    "agent".to_owned()
                } else {
                    "person".to_owned()
                },
                connected_through: principal
                    .via_grant_id()
                    .map(|_| principal.client_name.clone()),
                access: if principal.can_write() {
                    "full".to_owned()
                } else {
                    "read".to_owned()
                },
            },
            reply_requests,
            notifications,
            mentions,
            unread,
            tasks,
            claims,
            thread_updates,
            next_steps: NEXT_STEPS,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "briefing"))
}

pub(super) enum TaskFilter {
    Assigned,
    Claimed,
}

pub(super) async fn task_lines(
    state: &AppState,
    projects: &[Uuid],
    actor_id: Uuid,
    filter: TaskFilter,
) -> Result<Vec<TaskLine>, AppError> {
    let condition = match filter {
        TaskFilter::Assigned => {
            "item.assignee_actor_id = $2 AND status.semantic_category NOT IN ('completed', 'canceled')"
        }
        TaskFilter::Claimed => "claim.actor_id = $2",
    };
    let projects = crate::search::projects_showing_tasks(&state.pool, actor_id, projects).await?;
    Ok(sqlx::query_as::<_, TaskLine>(&format!(
        r#"
        SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
               item.title, status.name AS status, item.priority, project.name AS project,
               item.updated_at::text AS updated_at, claimer.display_name AS claimed_by
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN work_item_claims claim
          ON claim.work_item_id = item.id AND claim.expires_at > now()
        LEFT JOIN project_memberships claimer
          ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
        WHERE item.project_id = ANY($1) AND item.archived_at IS NULL AND {condition}
        ORDER BY item.updated_at DESC
        LIMIT 20
        "#
    ))
    .bind(&projects)
    .bind(actor_id)
    .fetch_all(&state.pool)
    .await?)
}

// ---------------------------------------------------------------------------
// projects_list

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectLine {
    pub id: String,
    pub key: String,
    pub name: String,
    pub role: String,
    /// True for the Messages space that holds DMs and groups.
    pub messages_space: bool,
    /// Only listed with includeArchived. Restore one with project_archive.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Pass as expectedVersion to project_update or project_archive.
    pub version: i64,
    pub open_tasks: i64,
    pub unread_conversations: i64,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectsValue {
    pub projects: Vec<ProjectLine>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectsListArguments {
    /// Also list archived projects you belong to.
    pub include_archived: Option<bool>,
}

pub(super) async fn projects_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectsListArguments,
) -> ToolResult<ProjectsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let mut projects = accessible_project_ids(state, principal).await?;
        if arguments.include_archived.unwrap_or(false) && principal.account_wide {
            // Archived projects are out of reach for everything except
            // project_archive, which brings them back.
            projects.extend(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT project.id
                    FROM projects project
                    JOIN project_memberships membership
                      ON membership.project_id = project.id AND membership.actor_id = $1
                     AND membership.enabled
                    JOIN organization_memberships organization_membership
                      ON organization_membership.organization_id = project.organization_id
                     AND organization_membership.actor_id = $1
                     AND organization_membership.enabled
                    WHERE project.archived_at IS NOT NULL
                    LIMIT 200
                    "#,
                )
                .bind(principal.actor_id)
                .fetch_all(&state.pool)
                .await?,
            );
        }
        // AWD-75: a project shows only where Projects is on for the caller;
        // the Messages space stays, as it always has.
        let projects = {
            let mut connection = state.pool.acquire().await?;
            ProjectModules::load(&mut connection, principal.actor_id, &projects)
                .await?
                .keep(&projects, |set, direct| direct || set.contains("projects"))
        };
        let mut rows = sqlx::query_as::<_, ProjectLine>(
            r#"
            SELECT project.id::text AS id, project.project_key AS key,
                   CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS name,
                   membership.role, project.kind = 'direct' AS messages_space,
                   project.archived_at IS NOT NULL AS archived, project.version,
                   (SELECT count(*) FROM work_items item
                    JOIN work_item_statuses status
                      ON status.project_id = item.project_id AND status.id = item.status_id
                    WHERE item.project_id = project.id AND item.archived_at IS NULL
                      AND status.semantic_category NOT IN ('completed', 'canceled')) AS open_tasks,
                   (SELECT count(*) FROM conversations conversation
                    LEFT JOIN conversation_read_states read_state
                      ON read_state.conversation_id = conversation.id AND read_state.actor_id = $1
                    WHERE conversation.project_id = project.id AND conversation.archived_at IS NULL
                      AND conversation.next_entry_sequence - 1
                          > COALESCE(read_state.highest_seen_sequence, 0)
                      AND conversation_visible_to(conversation.id, $1)) AS unread_conversations,
                   '' AS url
            FROM projects project
            JOIN project_memberships membership
              ON membership.project_id = project.id AND membership.actor_id = $1
            WHERE project.id = ANY($2)
            ORDER BY project.archived_at IS NOT NULL, (project.kind = 'direct'), project.name,
                     project.id
            "#,
        )
        .bind(principal.actor_id)
        .bind(&projects)
        .fetch_all(&state.pool)
        .await?;
        for row in &mut rows {
            row.url = if row.messages_space {
                public_url(state, "/messages")
            } else {
                public_url(state, &format!("/projects/{}", row.id))
            };
        }
        Ok::<_, AppError>(ToolResult::ok(ProjectsValue { projects: rows }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "projects_list"))
}

// ---------------------------------------------------------------------------
// project_open

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectOpenArguments {
    /// The project's ID, link or key (such as "AWD"). projects_list shows them.
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectOverview {
    pub project: ProjectHeader,
    /// The project's guide page: how this project works. Read it first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide: Option<GuidePage>,
    /// Skills: instructions for a kind of work, each with when to use it.
    /// Load the ones that fit your work with skill_get.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<super::memory_tools::SkillLine>,
    /// Other pages, titles only. Read one with page_get.
    pub pages: Vec<PageLine>,
    /// Recent decisions that still hold.
    pub decisions: Vec<DecisionLine>,
    pub tasks: TaskOverview,
    /// Threads still being worked out, across the project's channels.
    pub open_threads: Vec<ConversationSummary>,
    pub channels: Vec<ConversationSummary>,
    pub repositories: Vec<RepositoryLine>,
    /// Labels used here; label_list has colors and counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// The project's layout: what it's for, how to write for the people in
    /// it and the nouns it renames. Match its tone and words. Missing when
    /// nobody chose one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutLine>,
    /// Modules that are off here (AWD-75); what they hold is left out, and
    /// their tools say they're off. module_list says why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules_off: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LayoutLine {
    /// The preset's name, such as "Personal life" or "Software team".
    pub preset: String,
    /// How to write for people in this project.
    pub tone: String,
    /// Nouns this project calls something else, such as tasks called
    /// "to-dos". Use these words when you write to people here; tool names
    /// and arguments stay the same.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordLine>,
    /// True when the project follows the workspace's default layout.
    pub from_workspace: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WordLine {
    /// The app's own noun, such as "task".
    pub noun: String,
    pub one: String,
    pub many: String,
}

impl From<crate::layouts::ProjectTone> for LayoutLine {
    fn from(tone: crate::layouts::ProjectTone) -> Self {
        Self {
            preset: tone.preset,
            tone: tone.tone,
            words: tone
                .words
                .into_iter()
                .map(|(noun, word)| WordLine {
                    noun,
                    one: word.one,
                    many: word.many,
                })
                .collect(),
            from_workspace: tone.from_workspace,
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectHeader {
    pub id: String,
    pub key: String,
    pub name: String,
    pub role: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct GuidePage {
    pub page_id: String,
    pub title: String,
    pub body: String,
    pub truncated: bool,
    pub version: i64,
    pub updated_at: String,
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageLine {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecisionLine {
    pub id: String,
    pub title: String,
    pub decided_by: String,
    pub decided_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskOverview {
    /// Counts by status.
    pub by_status: Vec<StatusCount>,
    /// The first open tasks in board order: what's next.
    pub next: Vec<TaskLine>,
    /// Marked blocked, or waiting on a task that isn't finished.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<TaskLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claimed: Vec<TaskLine>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusCount {
    pub status: String,
    pub category: String,
    pub count: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepositoryLine {
    pub id: String,
    pub name: String,
    pub default_branch: String,
}

pub(super) async fn project_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectOpenArguments,
) -> ToolResult<ProjectOverview> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(project_id) = parse_id(&arguments.project_id) else {
        return ToolResult::invalid("projectId", "Pass a project ID or link.");
    };
    if project_id != principal.project_id {
        return ToolResult::not_found("That project");
    }
    let result = async {
        let Some(role) = project_role(state, principal, project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        let (key, name, kind) = sqlx::query_as::<_, (String, String, String)>(
            "SELECT project_key, name, kind FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(&state.pool)
        .await?;
        let guide = memory::guide_page(&state.pool, project_id, principal.actor_id)
            .await?
            .map(|page| {
                let (body, truncated) = truncate_text(&page.body, GUIDE_CHARACTERS);
                GuidePage {
                    page_id: page.id.to_string(),
                    title: page.title,
                    body,
                    truncated,
                    version: page.version,
                    updated_at: page.updated_at,
                    stale: page.stale,
                }
            });
        let skills =
            super::memory_tools::skill_lines(memory::list_skills(&state.pool, project_id).await?);
        let pages = memory::list_pages(&state.pool, project_id, false)
            .await?
            .into_iter()
            .filter(|page| !page.is_guide && page.skill_description.is_none())
            .take(50)
            .map(|page| PageLine {
                id: page.id.to_string(),
                title: page.title,
                updated_at: page.updated_at,
                stale: page.stale,
            })
            .collect();
        let decisions = memory::list_decisions(&state.pool, project_id, principal.actor_id, false, 15)
            .await?
            .into_iter()
            .map(|decision| DecisionLine {
                id: decision.id.to_string(),
                title: decision.title,
                decided_by: decision.decided_by,
                decided_at: decision.created_at,
            })
            .collect();
        let by_status = sqlx::query_as::<_, StatusCount>(
            r#"
            SELECT status.name AS status, status.semantic_category AS category, count(item.id) AS count
            FROM work_item_statuses status
            LEFT JOIN work_items item
              ON item.project_id = status.project_id AND item.status_id = status.id
             AND item.archived_at IS NULL
            WHERE status.project_id = $1 AND status.enabled
            GROUP BY status.id, status.name, status.semantic_category, status.position
            ORDER BY status.position
            "#,
        )
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
        // Tasks only for those whose role (and limit) lets them see tasks.
        let sees_tasks = !crate::search::projects_showing_tasks(
            &state.pool,
            principal.actor_id,
            &[project_id],
        )
        .await?
        .is_empty();
        let (next, blocked, claimed) = if sees_tasks {
            (
                project_tasks(state, project_id, ProjectTaskFilter::Next).await?,
                project_tasks(state, project_id, ProjectTaskFilter::Blocked).await?,
                project_tasks(state, project_id, ProjectTaskFilter::Claimed).await?,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        let by_status = if sees_tasks { by_status } else { Vec::new() };
        let conversations = list_rows(
            state,
            principal.actor_id,
            &[project_id],
            ListFilter {
                kinds: Some(vec!["channel".to_owned(), "thread".to_owned()]),
                with_person: None,
                unread_only: false,
                status: None,
                query: String::new(),
                only_id: None,
            },
            0,
            200,
        )
        .await?;
        let (mut channels, mut open_threads) = (Vec::new(), Vec::new());
        for row in conversations {
            let item = summary(state, principal.actor_id, row);
            if item.kind == "channel" {
                channels.push(item);
            } else if item.status.as_deref().is_some_and(|status| {
                !matches!(
                    status.to_ascii_lowercase().as_str(),
                    "done" | "dropped" | "decided"
                )
            }) && open_threads.len() < 15
            {
                open_threads.push(item);
            }
        }
        // Repositories only for those whose role (and limit) lets them see code.
        let sees_code = !crate::search::projects_allowing(
            &state.pool,
            principal.actor_id,
            &[project_id],
            crate::policy::Permission::CodeView,
        )
        .await?
        .is_empty();
        let repositories = if sees_code {
            sqlx::query_as::<_, RepositoryLine>(
                r#"
                SELECT id::text AS id, owner || '/' || name AS name, default_branch
                FROM repositories
                WHERE project_id = $1 AND archived_at IS NULL
                ORDER BY owner, name
                LIMIT 20
                "#,
            )
            .bind(project_id)
            .fetch_all(&state.pool)
            .await?
        } else {
            Vec::new()
        };
        let labels = work_item_labels::project_names(&state.pool, project_id, PROJECT_LABELS).await?;
        let layout = {
            let mut connection = state.pool.acquire().await?;
            layouts::project_tone(&mut connection, project_id)
                .await?
                .map(LayoutLine::from)
        };
        // AWD-75: leave out what belongs to modules that are off here.
        let modules = {
            let mut connection = state.pool.acquire().await?;
            crate::modules::state::for_project(&mut connection, project_id, principal.actor_id)
                .await?
                .unwrap_or_default()
        };
        let on = |module: &str| modules.contains(module);
        let (guide, skills, pages, decisions) = if on("pages") {
            (guide, skills, pages, decisions)
        } else {
            Default::default()
        };
        let (by_status, next, blocked, claimed, labels) = if on("tasks") {
            (by_status, next, blocked, claimed, labels)
        } else {
            Default::default()
        };
        let (channels, open_threads) = if on("channels") {
            (channels, open_threads)
        } else {
            Default::default()
        };
        let repositories = if on("code") { repositories } else { Vec::new() };
        let modules_off = modules.off().map(|module| module.id.clone()).collect();
        Ok::<_, AppError>(ToolResult::ok(ProjectOverview {
            project: ProjectHeader {
                id: project_id.to_string(),
                key,
                name: if kind == "direct" {
                    "Messages".to_owned()
                } else {
                    name
                },
                role: role.role,
                url: public_url(
                    state,
                    &if kind == "direct" {
                        "/messages".to_owned()
                    } else {
                        format!("/projects/{project_id}")
                    },
                ),
            },
            guide,
            skills,
            pages,
            decisions,
            tasks: TaskOverview {
                by_status,
                next,
                blocked,
                claimed,
            },
            open_threads,
            channels,
            repositories,
            labels,
            layout,
            modules_off,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_open"))
}

enum ProjectTaskFilter {
    Next,
    Blocked,
    Claimed,
}

async fn project_tasks(
    state: &AppState,
    project_id: Uuid,
    filter: ProjectTaskFilter,
) -> Result<Vec<TaskLine>, AppError> {
    // A task waiting on an unfinished dependency isn't next, however high
    // it sits on the board; it shows with the blocked ones.
    let (condition, limit) = match filter {
        ProjectTaskFilter::Next => (
            format!(
                "status.semantic_category IN ('unstarted', 'started', 'review', 'verification') AND item.blocked_reason IS NULL AND NOT {HAS_OPEN_BLOCKER}"
            ),
            10,
        ),
        ProjectTaskFilter::Blocked => (
            format!("(item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER})"),
            10,
        ),
        ProjectTaskFilter::Claimed => ("claim.actor_id IS NOT NULL".to_owned(), 10),
    };
    Ok(sqlx::query_as::<_, TaskLine>(&format!(
        r#"
        SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
               item.title, status.name AS status, item.priority, project.name AS project,
               item.updated_at::text AS updated_at, claimer.display_name AS claimed_by
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN work_item_claims claim
          ON claim.work_item_id = item.id AND claim.expires_at > now()
        LEFT JOIN project_memberships claimer
          ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
          AND {condition}
        ORDER BY status.position, item.board_rank, item.id
        LIMIT {limit}
        "#
    ))
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?)
}
