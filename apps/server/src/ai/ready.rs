//! AI5: whether a task is ready to start and what to do next, and V7: the
//! dependency graph.
//!
//! Nothing here calls a model: readiness is a set of checks a person can
//! read (blockers done, not marked blocked, has criteria, says enough, has a
//! repository to work in), and the ranking is a score from priority, how
//! much a task unblocks, and how close its milestone or due date is. The
//! graph adds the AI's pending "blocks" suggestions as dashed edges, for
//! the reader only when they can see every source.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    error::AppError,
    settings::{self, SettingsContext},
    work_item_planning::Reader,
};

/// The most tasks one ranking or graph loads.
const MAX_TASKS: i64 = 5000;
/// The most nodes a graph returns.
pub const MAX_NODES: usize = 1500;
/// A description shorter than this doesn't say enough for an agent.
const MIN_DESCRIPTION: i32 = 40;

#[derive(Clone, Debug, FromRow)]
struct Row {
    id: Uuid,
    project_id: Uuid,
    key: String,
    title: String,
    status_name: String,
    status_category: String,
    finished: bool,
    priority: String,
    estimate: Option<f64>,
    due_date: Option<NaiveDate>,
    milestone_id: Option<Uuid>,
    milestone_name: Option<String>,
    milestone_date: Option<NaiveDate>,
    blocked_reason: Option<String>,
    criteria: i64,
    description_length: i32,
    repositories: i64,
    assignee_name: Option<String>,
    claimed: bool,
}

const ROW_SELECT: &str = r#"
    SELECT item.id, item.project_id, project.project_key || '-' || item.number AS key,
           item.title, status.name AS status_name, status.semantic_category AS status_category,
           status.semantic_category IN ('completed', 'canceled') AS finished,
           item.priority, item.estimate, item.due_date,
           item.milestone_id, milestone.name AS milestone_name,
           milestone.target_date AS milestone_date,
           item.blocked_reason,
           (SELECT count(*) FROM work_item_acceptance_criteria criterion
             WHERE criterion.project_id = item.project_id AND criterion.work_item_id = item.id)
             AS criteria,
           char_length(btrim(item.description)) AS description_length,
           (SELECT count(*) FROM work_item_repository_links link
             WHERE link.project_id = item.project_id AND link.work_item_id = item.id)
             AS repositories,
           assignee.display_name AS assignee_name,
           EXISTS (SELECT 1 FROM work_item_claims claim
                    WHERE claim.work_item_id = item.id AND claim.expires_at > now()) AS claimed
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_milestones milestone
      ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
"#;

/// One readiness check.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// `dependencies`, `not_blocked`, `criteria`, `description`,
    /// `repository` or `environment`.
    pub id: &'static str,
    pub ok: bool,
    pub label: String,
    /// Whether an agent needs it to start; the rest only inform.
    pub for_agents: bool,
}

/// Whether a task can start, for a person and for an agent.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// Its blockers are done and it isn't marked blocked.
    pub ready: bool,
    /// Ready, and says enough for an agent to start: criteria, a
    /// description, a repository, and nobody holds a claim on it.
    pub ready_for_agents: bool,
    pub checks: Vec<Check>,
}

/// A task in the ranking.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedTask {
    pub id: Uuid,
    pub key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: String,
    pub milestone_id: Option<Uuid>,
    pub milestone_name: Option<String>,
    pub assignee_name: Option<String>,
    pub claimed: bool,
    pub score: i64,
    /// Open tasks that wait on this one, directly or further down.
    pub unblocks: usize,
    /// Why it ranks where it does, in words.
    pub reasons: Vec<String>,
    pub readiness: Readiness,
    pub href: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RankScope {
    /// Not started, blockers done, not marked blocked.
    #[default]
    Ready,
    /// Ready, and ready for an agent to pick up.
    Agents,
    /// Every open task, ranked, ready or not.
    All,
}

#[derive(Clone, Debug, Default)]
pub struct RankFilter {
    pub scope: RankScope,
    pub milestone_id: Option<Uuid>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ranking {
    pub tasks: Vec<RankedTask>,
    pub next_offset: Option<usize>,
    pub total: usize,
}

/// What the project's run settings say about where agents would work.
struct RunPlace {
    /// The project has one active repository, or `run.repository` names one.
    project_repository: bool,
    /// `run.environment`, when set.
    environment: Option<String>,
}

struct Loaded {
    rows: Vec<Row>,
    /// Blocker to blocked, between tasks of this project and blockers in
    /// other projects the reader can see.
    edges: Vec<(Uuid, Uuid)>,
    /// Blockers the reader can't see, per blocked task: they still block.
    hidden_blockers: HashMap<Uuid, usize>,
}

async fn load(pool: &PgPool, reader: Reader, project_id: Uuid) -> Result<Loaded, AppError> {
    let mut rows = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE item.project_id = $1 AND item.archived_at IS NULL ORDER BY item.number LIMIT $2"
    ))
    .bind(project_id)
    .bind(MAX_TASKS)
    .fetch_all(pool)
    .await?;
    let edges = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, bool)>(
        r#"
        SELECT dependency.blocking_work_item_id, dependency.blocked_work_item_id,
               dependency.blocking_project_id, dependency.project_id,
               dependency.blocking_project_id = dependency.project_id
                 OR actor_can_access_project($2, $3,
                      CASE WHEN dependency.project_id = $1
                           THEN dependency.blocking_project_id ELSE dependency.project_id END)
                 AS visible
        FROM work_item_dependencies dependency
        JOIN work_items blocking
          ON blocking.project_id = dependency.blocking_project_id
         AND blocking.id = dependency.blocking_work_item_id
        JOIN work_items blocked
          ON blocked.project_id = dependency.project_id
         AND blocked.id = dependency.blocked_work_item_id
        WHERE (dependency.project_id = $1 OR dependency.blocking_project_id = $1)
          AND blocking.archived_at IS NULL AND blocked.archived_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(pool)
    .await?;
    let mut visible_edges = Vec::new();
    let mut hidden_blockers: HashMap<Uuid, usize> = HashMap::new();
    let mut external = HashSet::new();
    for (blocking, blocked, blocking_project, blocked_project, visible) in edges {
        if !visible {
            if blocked_project == project_id {
                *hidden_blockers.entry(blocked).or_default() += 1;
            }
            continue;
        }
        if blocking_project != project_id {
            external.insert(blocking);
        }
        if blocked_project != project_id {
            external.insert(blocked);
        }
        visible_edges.push((blocking, blocked));
    }
    if !external.is_empty() {
        let ids = external.into_iter().collect::<Vec<_>>();
        rows.extend(
            sqlx::query_as::<_, Row>(&format!("{ROW_SELECT} WHERE item.id = ANY($1)"))
                .bind(&ids)
                .fetch_all(pool)
                .await?,
        );
    }
    Ok(Loaded {
        rows,
        edges: visible_edges,
        hidden_blockers,
    })
}

async fn run_place(pool: &PgPool, project_id: Uuid) -> Result<RunPlace, AppError> {
    let (organization_id, repositories) = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT project.organization_id,
               (SELECT count(*) FROM repositories repository
                 WHERE repository.project_id = project.id AND repository.enabled
                   AND repository.archived_at IS NULL)
        FROM projects project WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    let mut connection = pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    Ok(RunPlace {
        project_repository: repositories == 1
            || (repositories > 1 && effective.string("run.repository").is_some()),
        environment: effective.string("run.environment"),
    })
}

fn priority_weight(priority: &str) -> i64 {
    match priority {
        "urgent" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn not_started(category: &str) -> bool {
    matches!(category, "backlog" | "unstarted")
}

fn readiness(
    row: &Row,
    open_blockers: &[&Row],
    hidden: usize,
    place: &RunPlace,
) -> Readiness {
    let blockers_done = open_blockers.is_empty() && hidden == 0;
    let mut checks = vec![
        Check {
            id: "dependencies",
            ok: blockers_done,
            label: if blockers_done {
                "Blockers are done".to_owned()
            } else {
                let mut names = open_blockers
                    .iter()
                    .take(3)
                    .map(|blocker| blocker.key.clone())
                    .collect::<Vec<_>>();
                if hidden > 0 {
                    names.push(format!(
                        "{hidden} task{} you can't see",
                        if hidden == 1 { "" } else { "s" }
                    ));
                }
                format!("Waiting on {}", names.join(", "))
            },
            for_agents: true,
        },
        Check {
            id: "not_blocked",
            ok: row.blocked_reason.is_none(),
            label: match &row.blocked_reason {
                None => "Not marked blocked".to_owned(),
                Some(reason) => format!("Marked blocked: {}", truncate(reason, 120)),
            },
            for_agents: true,
        },
        Check {
            id: "criteria",
            ok: row.criteria > 0,
            label: if row.criteria > 0 {
                format!(
                    "{} acceptance criteri{}",
                    row.criteria,
                    if row.criteria == 1 { "on" } else { "a" }
                )
            } else {
                "No acceptance criteria".to_owned()
            },
            for_agents: true,
        },
        Check {
            id: "description",
            ok: row.description_length >= MIN_DESCRIPTION,
            label: if row.description_length >= MIN_DESCRIPTION {
                "Has a description".to_owned()
            } else {
                "The description is too short for an agent".to_owned()
            },
            for_agents: true,
        },
    ];
    let has_repository = row.repositories > 0 || place.project_repository;
    checks.push(Check {
        id: "repository",
        ok: has_repository,
        label: if row.repositories > 0 {
            "Linked to a repository".to_owned()
        } else if place.project_repository {
            "Uses the project's repository".to_owned()
        } else {
            "No repository to work in".to_owned()
        },
        for_agents: true,
    });
    checks.push(Check {
        id: "environment",
        ok: true,
        label: match &place.environment {
            Some(environment) => format!("Runs in {environment}"),
            None => "Runs on this host".to_owned(),
        },
        for_agents: false,
    });
    let ready = !row.finished && blockers_done && row.blocked_reason.is_none();
    let ready_for_agents = ready
        && !row.claimed
        && checks.iter().filter(|check| check.for_agents).all(|check| check.ok);
    Readiness {
        ready,
        ready_for_agents,
        checks,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// How many open tasks wait on each task, directly or further down.
fn unblock_counts(
    rows: &HashMap<Uuid, &Row>,
    edges: &[(Uuid, Uuid)],
) -> HashMap<Uuid, usize> {
    let mut downstream: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (blocking, blocked) in edges {
        if rows.get(blocked).is_some_and(|row| !row.finished) {
            downstream.entry(*blocking).or_default().push(*blocked);
        }
    }
    let mut counts = HashMap::new();
    for id in rows.keys() {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([*id]);
        while let Some(next) = queue.pop_front() {
            for &blocked in downstream.get(&next).into_iter().flatten() {
                if blocked != *id && seen.insert(blocked) && seen.len() < 200 {
                    queue.push_back(blocked);
                }
            }
        }
        counts.insert(*id, seen.len());
    }
    counts
}

/// Days until the nearer of the task's due date and its milestone's date.
fn days_left(row: &Row, today: NaiveDate) -> Option<(i64, String)> {
    let mut nearest: Option<(NaiveDate, String)> = row.due_date.map(|date| (date, "Due".to_owned()));
    if let (Some(date), Some(name)) = (row.milestone_date, &row.milestone_name)
        && nearest.as_ref().is_none_or(|(due, _)| date < *due)
    {
        nearest = Some((date, format!("Milestone {name} is due")));
    }
    nearest.map(|(date, what)| ((date - today).num_days(), what))
}

fn score(row: &Row, unblocks: usize, today: NaiveDate) -> (i64, Vec<String>) {
    let mut reasons = Vec::new();
    let priority = priority_weight(&row.priority);
    if priority > 0 {
        let mut label = row.priority.clone();
        if let Some(first) = label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        reasons.push(format!("{label} priority"));
    }
    if unblocks > 0 {
        reasons.push(format!(
            "Unblocks {unblocks} task{}",
            if unblocks == 1 { "" } else { "s" }
        ));
    }
    let mut urgency = 0;
    if let Some((days, what)) = days_left(row, today) {
        urgency = match days {
            ..0 => 40,
            0..=7 => 30,
            8..=30 => 15,
            _ => 0,
        };
        if days < 0 {
            reasons.push(format!("{what} {} days ago", -days));
        } else if days <= 30 {
            reasons.push(match days {
                0 => format!("{what} today"),
                1 => format!("{what} tomorrow"),
                _ => format!("{what} in {days} days"),
            });
        }
    }
    let score = priority * 100 + (unblocks.min(50) as i64) * 10 + urgency;
    (score, reasons)
}

fn href(row: &Row) -> String {
    format!("/projects/{}/work-items/{}", row.project_id, row.id)
}

/// The project's open tasks, ranked, with each one's readiness.
pub async fn rank(
    pool: &PgPool,
    reader: Reader,
    project_id: Uuid,
    filter: &RankFilter,
) -> Result<Ranking, AppError> {
    let loaded = load(pool, reader, project_id).await?;
    let place = run_place(pool, project_id).await?;
    let by_id = loaded
        .rows
        .iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let unblocks = unblock_counts(&by_id, &loaded.edges);
    let mut blockers: HashMap<Uuid, Vec<&Row>> = HashMap::new();
    for (blocking, blocked) in &loaded.edges {
        if let Some(row) = by_id.get(blocking).filter(|row| !row.finished) {
            blockers.entry(*blocked).or_default().push(row);
        }
    }
    let today = Utc::now().date_naive();
    let mut ranked = loaded
        .rows
        .iter()
        .filter(|row| row.project_id == project_id && !row.finished)
        .filter(|row| filter.milestone_id.is_none_or(|id| row.milestone_id == Some(id)))
        .map(|row| {
            let readiness = readiness(
                row,
                blockers.get(&row.id).map(Vec::as_slice).unwrap_or(&[]),
                loaded.hidden_blockers.get(&row.id).copied().unwrap_or(0),
                &place,
            );
            let count = unblocks.get(&row.id).copied().unwrap_or(0);
            let (score, reasons) = score(row, count, today);
            RankedTask {
                id: row.id,
                key: row.key.clone(),
                title: row.title.clone(),
                status_name: row.status_name.clone(),
                status_category: row.status_category.clone(),
                priority: row.priority.clone(),
                milestone_id: row.milestone_id,
                milestone_name: row.milestone_name.clone(),
                assignee_name: row.assignee_name.clone(),
                claimed: row.claimed,
                score,
                unblocks: count,
                reasons,
                readiness,
                href: href(row),
            }
        })
        .filter(|task| match filter.scope {
            RankScope::Ready => task.readiness.ready && not_started(&task.status_category),
            RankScope::Agents => {
                task.readiness.ready_for_agents && not_started(&task.status_category)
            }
            RankScope::All => true,
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| natural_key(&a.key).cmp(&natural_key(&b.key)))
    });
    let total = ranked.len();
    let limit = filter.limit.clamp(1, 200);
    let tasks = ranked
        .into_iter()
        .skip(filter.offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next_offset = (filter.offset + tasks.len() < total).then_some(filter.offset + tasks.len());
    Ok(Ranking {
        tasks,
        next_offset,
        total,
    })
}

/// Sorts `ACME-9` before `ACME-10`.
fn natural_key(key: &str) -> (String, u64) {
    match key.rsplit_once('-') {
        Some((prefix, number)) => (prefix.to_owned(), number.parse().unwrap_or(u64::MAX)),
        None => (key.to_owned(), u64::MAX),
    }
}

/// One task's readiness, for its panel. None when it isn't in the project.
pub async fn readiness_of(
    pool: &PgPool,
    reader: Reader,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Readiness>, AppError> {
    let Some(row) = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE item.project_id = $1 AND item.id = $2 AND item.archived_at IS NULL"
    ))
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let blockers = sqlx::query_as::<_, Row>(&format!(
        r#"{ROW_SELECT}
        JOIN work_item_dependencies dependency
          ON dependency.blocking_project_id = item.project_id
         AND dependency.blocking_work_item_id = item.id
        WHERE dependency.project_id = $1 AND dependency.blocked_work_item_id = $2
          AND item.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
          AND (item.project_id = $1 OR actor_can_access_project($3, $4, item.project_id))
        "#
    ))
    .bind(project_id)
    .bind(task_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(pool)
    .await?;
    let hidden = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM work_item_dependencies dependency
        JOIN work_items item
          ON item.project_id = dependency.blocking_project_id
         AND item.id = dependency.blocking_work_item_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE dependency.project_id = $1 AND dependency.blocked_work_item_id = $2
          AND item.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
          AND item.project_id <> $1
          AND NOT actor_can_access_project($3, $4, item.project_id)
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_one(pool)
    .await?;
    let place = run_place(pool, project_id).await?;
    let blockers = blockers.iter().collect::<Vec<_>>();
    Ok(Some(readiness(
        &row,
        &blockers,
        usize::try_from(hidden).unwrap_or(0),
        &place,
    )))
}

// ---------------------------------------------------------------------------
// The graph

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub finished: bool,
    pub priority: String,
    pub estimate: Option<f64>,
    pub milestone_id: Option<Uuid>,
    pub milestone_name: Option<String>,
    pub assignee_name: Option<String>,
    /// In another project of the workspace.
    pub external: bool,
    /// Open, blockers done, not marked blocked.
    pub ready: bool,
    pub critical: bool,
    pub href: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    /// The blocking task.
    pub from: Uuid,
    /// The task it blocks.
    pub to: Uuid,
    /// A pending AI suggestion, not a dependency yet.
    pub suggestion_id: Option<Uuid>,
    pub confidence: Option<f32>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Open tasks that can start now, best first.
    pub frontier: Vec<Uuid>,
    /// The longest chain of open work, by estimate (1 where unset).
    pub critical_path: Vec<Uuid>,
    /// More tasks than the graph shows.
    pub truncated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct GraphFilter {
    pub milestone_id: Option<Uuid>,
    /// Show finished tasks that aren't blocking anything open.
    pub include_finished: bool,
}

#[derive(FromRow)]
struct SuggestedEdge {
    id: Uuid,
    work_item_id: Uuid,
    other_work_item_id: Uuid,
    confidence: f32,
    reason: String,
}

/// The project's dependency graph as `reader` may see it.
pub async fn graph(
    pool: &PgPool,
    reader: Reader,
    project_id: Uuid,
    filter: &GraphFilter,
) -> Result<Graph, AppError> {
    let loaded = load(pool, reader, project_id).await?;
    let by_id = loaded
        .rows
        .iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let suggested = sqlx::query_as::<_, SuggestedEdge>(
        r#"
        SELECT suggestion.id, suggestion.work_item_id, suggestion.other_work_item_id,
               suggestion.confidence, suggestion.reason
        FROM ai_suggestions suggestion
        WHERE suggestion.project_id = $1 AND suggestion.kind = 'blocks'
          AND suggestion.status = 'pending'
          AND ai_suggestion_visible_to(suggestion.id, $2, $3)
        ORDER BY suggestion.confidence DESC
        LIMIT 500
        "#,
    )
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(pool)
    .await?;
    // Which tasks to show: open ones in this project (in the milestone, when
    // filtered), the tasks linked to them, and finished ones on request.
    let in_scope = |row: &Row| {
        row.project_id == project_id
            && (!row.finished || filter.include_finished)
            && filter.milestone_id.is_none_or(|id| row.milestone_id == Some(id))
    };
    let mut shown = loaded
        .rows
        .iter()
        .filter(|row| in_scope(row))
        .map(|row| row.id)
        .collect::<HashSet<_>>();
    let core = shown.clone();
    for (blocking, blocked) in &loaded.edges {
        if core.contains(blocked) || core.contains(blocking) {
            shown.insert(*blocking);
            shown.insert(*blocked);
        }
    }
    for edge in &suggested {
        if core.contains(&edge.work_item_id) && by_id.contains_key(&edge.other_work_item_id) {
            shown.insert(edge.other_work_item_id);
        }
    }
    let truncated = shown.len() > MAX_NODES;
    if truncated {
        // Keep open tasks first, then by key.
        let mut ordered = shown.iter().filter_map(|id| by_id.get(id)).collect::<Vec<_>>();
        ordered.sort_by(|a, b| {
            a.finished
                .cmp(&b.finished)
                .then_with(|| natural_key(&a.key).cmp(&natural_key(&b.key)))
        });
        shown = ordered.into_iter().take(MAX_NODES).map(|row| row.id).collect();
    }
    let edges = loaded
        .edges
        .iter()
        .filter(|(blocking, blocked)| shown.contains(blocking) && shown.contains(blocked))
        .copied()
        .collect::<Vec<_>>();
    let mut open_blockers: HashMap<Uuid, usize> = HashMap::new();
    for (blocking, blocked) in &loaded.edges {
        if by_id.get(blocking).is_some_and(|row| !row.finished) {
            *open_blockers.entry(*blocked).or_default() += 1;
        }
    }
    let critical = critical_path(&by_id, &edges, &shown);
    let critical_set = critical.iter().copied().collect::<HashSet<_>>();
    let mut nodes = shown
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|row| GraphNode {
            id: row.id,
            project_id: row.project_id,
            key: row.key.clone(),
            title: row.title.clone(),
            status_name: row.status_name.clone(),
            status_category: row.status_category.clone(),
            finished: row.finished,
            priority: row.priority.clone(),
            estimate: row.estimate,
            milestone_id: row.milestone_id,
            milestone_name: row.milestone_name.clone(),
            assignee_name: row.assignee_name.clone(),
            external: row.project_id != project_id,
            ready: !row.finished
                && row.blocked_reason.is_none()
                && open_blockers.get(&row.id).copied().unwrap_or(0) == 0
                && loaded.hidden_blockers.get(&row.id).copied().unwrap_or(0) == 0,
            critical: critical_set.contains(&row.id),
            href: href(row),
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|a, b| natural_key(&a.key).cmp(&natural_key(&b.key)));
    let unblocks = unblock_counts(&by_id, &loaded.edges);
    let today = Utc::now().date_naive();
    let mut frontier = nodes
        .iter()
        .filter(|node| node.ready && !node.external && not_started(&node.status_category))
        .map(|node| {
            let row = by_id[&node.id];
            (score(row, unblocks.get(&node.id).copied().unwrap_or(0), today).0, node.id)
        })
        .collect::<Vec<_>>();
    frontier.sort_by(|a, b| b.0.cmp(&a.0));
    let mut graph_edges = edges
        .iter()
        .map(|(from, to)| GraphEdge {
            from: *from,
            to: *to,
            suggestion_id: None,
            confidence: None,
            reason: None,
        })
        .collect::<Vec<_>>();
    graph_edges.extend(
        suggested
            .into_iter()
            .filter(|edge| shown.contains(&edge.work_item_id) && shown.contains(&edge.other_work_item_id))
            .map(|edge| GraphEdge {
                from: edge.other_work_item_id,
                to: edge.work_item_id,
                suggestion_id: Some(edge.id),
                confidence: Some(edge.confidence),
                reason: Some(edge.reason),
            }),
    );
    Ok(Graph {
        nodes,
        edges: graph_edges,
        frontier: frontier.into_iter().map(|(_, id)| id).collect(),
        critical_path: critical,
        truncated,
    })
}

/// The longest chain of open tasks through the dependencies, weighting each
/// task by its estimate (1 where unset).
fn critical_path(
    rows: &HashMap<Uuid, &Row>,
    edges: &[(Uuid, Uuid)],
    shown: &HashSet<Uuid>,
) -> Vec<Uuid> {
    let open = |id: &Uuid| shown.contains(id) && rows.get(id).is_some_and(|row| !row.finished);
    let mut next: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut incoming: HashMap<Uuid, usize> = shown.iter().filter(|id| open(id)).map(|id| (*id, 0)).collect();
    for (from, to) in edges {
        if open(from) && open(to) {
            next.entry(*from).or_default().push(*to);
            *incoming.entry(*to).or_default() += 1;
        }
    }
    let weight = |id: &Uuid| rows[id].estimate.filter(|estimate| *estimate > 0.0).unwrap_or(1.0);
    let mut queue = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    queue.sort();
    let mut queue = VecDeque::from(queue);
    let mut best: HashMap<Uuid, (f64, Option<Uuid>)> = HashMap::new();
    while let Some(id) = queue.pop_front() {
        let here = best.get(&id).map_or(weight(&id), |(total, _)| *total);
        best.entry(id).or_insert((here, None));
        for &to in next.get(&id).into_iter().flatten() {
            let candidate = here + weight(&to);
            let entry = best.entry(to).or_insert((0.0, None));
            if candidate > entry.0 {
                *entry = (candidate, Some(id));
            }
            if let Some(count) = incoming.get_mut(&to) {
                *count -= 1;
                if *count == 0 {
                    queue.push_back(to);
                }
            }
        }
    }
    // Only a chain of at least two tasks is worth calling out.
    let Some((&end, _)) = best
        .iter()
        .filter(|(_, (_, previous))| previous.is_some())
        .max_by(|a, b| a.1.0.total_cmp(&b.1.0).then_with(|| b.0.cmp(a.0)))
    else {
        return Vec::new();
    };
    let mut path = vec![end];
    let mut current = end;
    while let Some((_, Some(previous))) = best.get(&current) {
        path.push(*previous);
        current = *previous;
        if path.len() > rows.len() {
            break;
        }
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: Uuid, estimate: Option<f64>, finished: bool) -> Row {
        Row {
            id,
            project_id: Uuid::nil(),
            key: format!("T-{}", id.as_u128()),
            title: String::new(),
            status_name: String::new(),
            status_category: if finished { "completed" } else { "unstarted" }.to_owned(),
            finished,
            priority: "no_priority".to_owned(),
            estimate,
            due_date: None,
            milestone_id: None,
            milestone_name: None,
            milestone_date: None,
            blocked_reason: None,
            criteria: 0,
            description_length: 0,
            repositories: 0,
            assignee_name: None,
            claimed: false,
        }
    }

    #[test]
    fn the_critical_path_follows_the_heaviest_open_chain() {
        let ids = (1..=5).map(Uuid::from_u128).collect::<Vec<_>>();
        let rows = vec![
            row(ids[0], None, false),
            row(ids[1], Some(5.0), false),
            row(ids[2], None, false),
            row(ids[3], None, false),
            row(ids[4], None, true),
        ];
        let by_id = rows.iter().map(|row| (row.id, row)).collect::<HashMap<_, _>>();
        let shown = ids.iter().copied().collect::<HashSet<_>>();
        // 1 -> 2 (5 points) and 1 -> 3 -> 4; 5 is finished.
        let edges = vec![
            (ids[0], ids[1]),
            (ids[0], ids[2]),
            (ids[2], ids[3]),
            (ids[4], ids[0]),
        ];
        assert_eq!(critical_path(&by_id, &edges, &shown), vec![ids[0], ids[1]]);
        let counts = unblock_counts(&by_id, &edges);
        assert_eq!(counts[&ids[0]], 3);
        assert_eq!(counts[&ids[2]], 1);
    }

    #[test]
    fn score_weighs_priority_unblocking_and_dates() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 16).expect("date");
        let mut urgent = row(Uuid::from_u128(1), None, false);
        urgent.priority = "urgent".to_owned();
        let (urgent_score, reasons) = score(&urgent, 2, today);
        assert_eq!(urgent_score, 420);
        assert_eq!(reasons, vec!["Urgent priority", "Unblocks 2 tasks"]);
        let mut due = row(Uuid::from_u128(2), None, false);
        due.milestone_name = Some("Beta".to_owned());
        due.milestone_date = NaiveDate::from_ymd_opt(2026, 10, 20);
        let (due_score, reasons) = score(&due, 0, today);
        assert_eq!(due_score, 30);
        assert_eq!(reasons, vec!["Milestone Beta is due in 4 days"]);
        assert!(natural_key("A-9") < natural_key("A-10"));
    }
}
