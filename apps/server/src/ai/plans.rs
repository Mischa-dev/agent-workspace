//! AI3: a plan turned into milestones, tasks and dependencies.
//!
//! An import starts from pasted text, a project page, or structured JSON.
//! Structured plans (this repository's `docs/plans/roadmap.json`, or the
//! draft shape itself) are read without a model; text goes to the model in
//! parts. Either way the result is a draft a person reviews and edits.
//!
//! Committing runs as a job in steps, each in its own transaction: the
//! milestones, then tasks in batches of up to 100 (parents first), then the
//! dependencies. Every created thing is recorded by its draft ref in the
//! step that made it, so a step that fails or a server that restarts picks
//! up where it stopped and never creates anything twice. Each step rechecks
//! that the person who committed may still change tasks (principle 9).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    detect::background_actor,
    gateway::{self, AiError, Scope, Tier},
    prompts,
};
use crate::{
    AppState,
    error::AppError,
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    mcp::{ImportTask, create_for_import},
    policy::Action,
    work_item_commands::{CommandActor, authorize_mutation, lock_membership},
    work_item_planning::{self as planning, Direction},
};

pub const DRAFT_KIND: &str = "ai.plan_draft";
pub const COMMIT_KIND: &str = "ai.plan_commit";

pub const MAX_TASKS: usize = 2000;
pub const MAX_MILESTONES: usize = 100;
/// Most plan text one import takes.
pub const MAX_SOURCE_CHARS: usize = 400_000;
/// Text one model call reads.
const PART_CHARS: usize = 40_000;
const TASKS_PER_STEP: usize = 100;
const DEPENDENCIES_PER_STEP: usize = 300;

// ---------------------------------------------------------------------------
// The draft

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    #[serde(default)]
    pub milestones: Vec<DraftMilestone>,
    #[serde(default)]
    pub tasks: Vec<DraftTask>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftMilestone {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target_date: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftTask {
    #[serde(rename = "ref")]
    pub reference: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "no_priority")]
    pub priority: String,
    #[serde(default)]
    pub milestone: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

fn no_priority() -> String {
    "no_priority".to_owned()
}

fn valid_reference(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

/// Checks a draft a person edited: every ref well formed and used once,
/// every milestone, parent and blocker ref known, no task its own ancestor,
/// and the text within the limits tasks have. Returns the problems, at most
/// ten.
pub fn check(draft: &Draft) -> Vec<String> {
    let mut problems = Vec::new();
    if draft.tasks.is_empty() {
        problems.push("The plan has no tasks.".to_owned());
    }
    if draft.tasks.len() > MAX_TASKS {
        problems.push(format!("A plan holds at most {MAX_TASKS} tasks."));
    }
    if draft.milestones.len() > MAX_MILESTONES {
        problems.push(format!("A plan holds at most {MAX_MILESTONES} milestones."));
    }
    let mut milestones = HashSet::new();
    for milestone in &draft.milestones {
        if !valid_reference(&milestone.reference)
            || !milestones.insert(milestone.reference.as_str())
        {
            problems.push(format!(
                "Milestone ref \"{}\" must be unique and use only letters, digits, dashes, dots or underscores.",
                truncate(&milestone.reference, 64)
            ));
        }
        if planning::normalize_milestone_name(&milestone.name).is_none() {
            problems.push(format!(
                "Milestone {} needs a name of 1 to 120 characters.",
                milestone.reference
            ));
        }
        if milestone.description.chars().count() > 2000 {
            problems.push(format!(
                "Milestone {}'s description is over 2,000 characters.",
                milestone.reference
            ));
        }
        if let Some(date) = &milestone.target_date
            && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        {
            problems.push(format!(
                "Milestone {}'s date should look like 2026-12-01.",
                milestone.reference
            ));
        }
    }
    let mut tasks = HashMap::new();
    for (index, task) in draft.tasks.iter().enumerate() {
        if !valid_reference(&task.reference)
            || tasks.insert(task.reference.as_str(), index).is_some()
        {
            problems.push(format!(
                "Task ref \"{}\" must be unique and use only letters, digits, dashes, dots or underscores.",
                truncate(&task.reference, 64)
            ));
        }
        let title = task.title.trim().chars().count();
        if !(1..=240).contains(&title) {
            problems.push(format!(
                "Task {} needs a title of 1 to 240 characters.",
                task.reference
            ));
        }
        if task.description.chars().count() > 100_000 {
            problems.push(format!(
                "Task {}'s description is too long.",
                task.reference
            ));
        }
        if task.criteria.len() > 50
            || task
                .criteria
                .iter()
                .any(|criterion| !(1..=1000).contains(&criterion.trim().chars().count()))
        {
            problems.push(format!(
                "Task {} may have up to 50 criteria of 1 to 1,000 characters.",
                task.reference
            ));
        }
        if task.labels.len() > 20
            || task
                .labels
                .iter()
                .any(|label| !(1..=50).contains(&label.trim().chars().count()))
        {
            problems.push(format!(
                "Task {} may have up to 20 labels of 1 to 50 characters.",
                task.reference
            ));
        }
        if !matches!(
            task.priority.as_str(),
            "no_priority" | "low" | "medium" | "high" | "urgent"
        ) {
            problems.push(format!(
                "Task {}'s priority isn't one tasks use.",
                task.reference
            ));
        }
        if let Some(milestone) = &task.milestone
            && !milestones.contains(milestone.as_str())
        {
            problems.push(format!(
                "Task {} names milestone {milestone}, which isn't in the plan.",
                task.reference
            ));
        }
    }
    for task in &draft.tasks {
        if let Some(parent) = &task.parent
            && (!tasks.contains_key(parent.as_str()) || parent == &task.reference)
        {
            problems.push(format!(
                "Task {}'s parent {parent} isn't another task in the plan.",
                task.reference
            ));
        }
        for blocker in &task.blocked_by {
            if !tasks.contains_key(blocker.as_str()) || blocker == &task.reference {
                problems.push(format!(
                    "Task {} is blocked by {blocker}, which isn't another task in the plan.",
                    task.reference
                ));
            }
        }
    }
    if problems.is_empty() && parent_order(draft).is_none() {
        problems.push("Some tasks are each other's parents.".to_owned());
    }
    problems.truncate(10);
    problems
}

/// The tasks' indexes with every parent before its children, or none when
/// parents loop.
fn parent_order(draft: &Draft) -> Option<Vec<usize>> {
    let index = draft
        .tasks
        .iter()
        .enumerate()
        .map(|(position, task)| (task.reference.as_str(), position))
        .collect::<HashMap<_, _>>();
    let mut order = Vec::with_capacity(draft.tasks.len());
    let mut state = vec![0u8; draft.tasks.len()];
    fn visit(
        at: usize,
        draft: &Draft,
        index: &HashMap<&str, usize>,
        state: &mut [u8],
        order: &mut Vec<usize>,
    ) -> bool {
        match state[at] {
            2 => return true,
            1 => return false,
            _ => {}
        }
        state[at] = 1;
        if let Some(parent) = draft.tasks[at]
            .parent
            .as_deref()
            .and_then(|parent| index.get(parent))
            && !visit(*parent, draft, index, state, order)
        {
            return false;
        }
        state[at] = 2;
        order.push(at);
        true
    }
    for at in 0..draft.tasks.len() {
        if !visit(at, draft, &index, &mut state, &mut order) {
            return None;
        }
    }
    Some(order)
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A ref made safe and unique.
fn clean_reference(value: &str, taken: &mut HashSet<String>) -> String {
    let mut base = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .take(56)
        .collect::<String>();
    if base.trim_matches('-').is_empty() {
        base = "item".to_owned();
    }
    let mut candidate = base.clone();
    let mut counter = 2;
    while !taken.insert(candidate.clone()) {
        candidate = format!("{base}-{counter}");
        counter += 1;
    }
    candidate
}

/// Reads a structured plan without a model: the draft shape itself, or a
/// roadmap like `docs/plans/roadmap.json` (workstreams become milestones,
/// items become tasks blocked by their `deps`, labelled with their area).
pub fn read_structured(value: &Value) -> Option<Draft> {
    if value.get("workstreams").is_some() && value.get("items").is_some() {
        return Some(read_roadmap(value));
    }
    if value.get("tasks").is_some_and(Value::is_array) {
        return serde_json::from_value::<Draft>(value.clone()).ok();
    }
    None
}

fn read_roadmap(value: &Value) -> Draft {
    let text =
        |item: &Value, field: &str| item[field].as_str().unwrap_or_default().trim().to_owned();
    let areas = value["areas"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|area| (text(area, "code"), truncate(&text(area, "name"), 50)))
        .collect::<HashMap<_, _>>();
    let mut milestone_refs = HashSet::new();
    let mut milestone_of = HashMap::new();
    let milestones = value["workstreams"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|workstream| {
            let id = text(workstream, "id");
            let reference = clean_reference(&id, &mut milestone_refs);
            milestone_of.insert(id.clone(), reference.clone());
            DraftMilestone {
                reference,
                name: truncate(format!("{id} {}", text(workstream, "name")).trim(), 120),
                description: truncate(&text(workstream, "goal"), 2000),
                target_date: None,
            }
        })
        .collect::<Vec<_>>();
    let mut task_refs = HashSet::new();
    let mut task_of = HashMap::new();
    let items = value["items"].as_array().cloned().unwrap_or_default();
    for item in &items {
        let id = text(item, "id");
        task_of.insert(id.clone(), clean_reference(&id, &mut task_refs));
    }
    let tasks = items
        .iter()
        .map(|item| {
            let id = text(item, "id");
            let done = text(item, "done");
            let mut description = text(item, "what");
            if !done.is_empty() {
                description = format!("{description}\n\nDone when: {done}");
            }
            DraftTask {
                reference: task_of[&id].clone(),
                title: truncate(format!("{id} {}", text(item, "title")).trim(), 240),
                description,
                criteria: if done.is_empty() {
                    Vec::new()
                } else {
                    vec![truncate(&done, 1000)]
                },
                labels: areas
                    .get(&text(item, "area"))
                    .filter(|name| !name.is_empty())
                    .map(|name| vec![name.clone()])
                    .unwrap_or_default(),
                priority: no_priority(),
                milestone: milestone_of.get(&text(item, "workstream")).cloned(),
                parent: None,
                blocked_by: item["deps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|dep| task_of.get(dep).cloned())
                    .collect(),
            }
        })
        .collect();
    Draft { milestones, tasks }
}

/// Splits plan text into parts of at most [`PART_CHARS`], at headings where
/// it can, else at lines.
fn split_parts(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let heading = line.trim_start().starts_with('#');
        if !current.is_empty()
            && (current.len() + line.len() + 1 > PART_CHARS
                || (heading && current.len() > PART_CHARS / 2))
        {
            parts.push(std::mem::take(&mut current));
        }
        if line.len() > PART_CHARS {
            parts.push(line.chars().take(PART_CHARS).collect());
            continue;
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// Joins the drafts the model made for each part, keeping refs unique and
/// dropping references to anything that isn't there.
fn merge_parts(parts: Vec<Draft>) -> Draft {
    let several = parts.len() > 1;
    let mut merged = Draft::default();
    let mut milestone_refs = HashSet::new();
    let mut task_refs = HashSet::new();
    for (number, part) in parts.into_iter().enumerate() {
        let prefix = |reference: &str| {
            if several {
                format!("p{}-{reference}", number + 1)
            } else {
                reference.to_owned()
            }
        };
        let mut milestones = HashMap::new();
        for milestone in part.milestones {
            let reference = clean_reference(&prefix(&milestone.reference), &mut milestone_refs);
            milestones.insert(milestone.reference.clone(), reference.clone());
            merged.milestones.push(DraftMilestone {
                reference,
                name: truncate(milestone.name.trim(), 120),
                description: truncate(&milestone.description, 2000),
                target_date: milestone
                    .target_date
                    .filter(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()),
            });
        }
        let mut tasks = HashMap::new();
        for task in &part.tasks {
            tasks.insert(
                task.reference.clone(),
                clean_reference(&prefix(&task.reference), &mut task_refs),
            );
        }
        for task in part.tasks {
            let reference = tasks[&task.reference].clone();
            merged.tasks.push(DraftTask {
                parent: task
                    .parent
                    .and_then(|parent| tasks.get(&parent).cloned())
                    .filter(|parent| parent != &reference),
                blocked_by: task
                    .blocked_by
                    .iter()
                    .filter_map(|blocker| tasks.get(blocker).cloned())
                    .filter(|blocker| blocker != &reference)
                    .collect(),
                milestone: task
                    .milestone
                    .and_then(|milestone| milestones.get(&milestone).cloned()),
                reference,
                title: truncate(task.title.trim(), 240),
                description: task.description,
                criteria: task
                    .criteria
                    .into_iter()
                    .map(|criterion| truncate(criterion.trim(), 1000))
                    .filter(|criterion| !criterion.is_empty())
                    .take(50)
                    .collect(),
                labels: task
                    .labels
                    .into_iter()
                    .map(|label| truncate(label.trim(), 50))
                    .filter(|label| !label.is_empty())
                    .take(20)
                    .collect(),
                priority: if matches!(task.priority.as_str(), "low" | "medium" | "high" | "urgent")
                {
                    task.priority
                } else {
                    no_priority()
                },
            });
        }
    }
    merged.tasks.truncate(MAX_TASKS);
    merged.milestones.truncate(MAX_MILESTONES);
    merged
}

// ---------------------------------------------------------------------------
// Imports

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    pub id: Uuid,
    pub title: String,
    pub source_kind: String,
    pub state: String,
    #[sqlx(json)]
    pub draft: Draft,
    pub error: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub committed_at: Option<DateTime<Utc>>,
    /// Created so far, while committing and after.
    pub created_milestones: i64,
    pub created_tasks: i64,
    pub created_dependencies: i64,
}

const IMPORT_COLUMNS: &str = r#"
    import.id, import.title, import.source_kind, import.state, import.draft, import.error,
    import.version, import.created_at, import.committed_at,
    (SELECT count(*) FROM ai_plan_import_items item
      WHERE item.import_id = import.id AND item.kind = 'milestone') AS created_milestones,
    (SELECT count(*) FROM ai_plan_import_items item
      WHERE item.import_id = import.id AND item.kind = 'task') AS created_tasks,
    (SELECT count(*) FROM ai_plan_import_items item
      WHERE item.import_id = import.id AND item.kind = 'dependency'
        AND item.work_item_id IS NOT NULL) AS created_dependencies
"#;

/// Where a plan comes from.
#[derive(Clone, Debug, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum Source {
    Text { text: String },
    Page { page_id: Uuid },
    Structured { plan: Value },
}

#[derive(Debug)]
pub enum CreateRefusal {
    Invalid(String),
    NotFound(&'static str),
    Ai(AiError),
}

/// Starts an import. Structured plans become a draft at once; text is
/// queued for the model. The caller checked `actor` may edit tasks.
pub async fn create(
    state: &AppState,
    organization_id: Uuid,
    actor: &CommandActor,
    title: &str,
    source: Source,
) -> Result<Result<ImportView, CreateRefusal>, AppError> {
    let title = title.trim();
    if !(1..=200).contains(&title.chars().count()) {
        return Ok(Err(CreateRefusal::Invalid(
            "Give the plan a title of 1 to 200 characters.".to_owned(),
        )));
    }
    let (kind, text) = match source {
        Source::Structured { plan } => ("structured", Err(plan)),
        Source::Text { text } => ("text", Ok(text)),
        Source::Page { page_id } => {
            let Some(body) = sqlx::query_scalar::<_, String>(
                "SELECT body FROM project_pages WHERE id = $1 AND project_id = $2 AND archived_at IS NULL",
            )
            .bind(page_id)
            .bind(actor.project_id)
            .fetch_optional(&state.pool)
            .await?
            else {
                return Ok(Err(CreateRefusal::NotFound("That page wasn't found.")));
            };
            ("page", Ok(body))
        }
    };
    // Text that is itself a structured plan needs no model.
    let structured = match &text {
        Err(plan) => Some(plan.clone()),
        Ok(text) => serde_json::from_str::<Value>(text.trim())
            .ok()
            .filter(|value| read_structured(value).is_some()),
    };
    let id = Uuid::now_v7();
    if let Some(plan) = structured {
        let Some(draft) = read_structured(&plan) else {
            return Ok(Err(CreateRefusal::Invalid(
                "That JSON isn't a plan: pass {\"milestones\": [...], \"tasks\": [...]} or a roadmap with workstreams and items.".to_owned(),
            )));
        };
        let problems = check(&draft);
        if !problems.is_empty() {
            return Ok(Err(CreateRefusal::Invalid(problems.join(" "))));
        }
        sqlx::query(
            r#"
            INSERT INTO ai_plan_imports (id, organization_id, project_id, created_by_actor_id, title, source_kind, state, draft)
            VALUES ($1, $2, $3, $4, $5, $6, 'draft', $7)
            "#,
        )
        .bind(id)
        .bind(organization_id)
        .bind(actor.project_id)
        .bind(actor.actor_id)
        .bind(title)
        .bind(if kind == "page" { "page" } else { "structured" })
        .bind(sqlx::types::Json(&draft))
        .execute(&state.pool)
        .await?;
    } else {
        let text = text.unwrap_or_default();
        let length = text.trim().chars().count();
        if length == 0 || length > MAX_SOURCE_CHARS {
            return Ok(Err(CreateRefusal::Invalid(format!(
                "Paste a plan of 1 to {MAX_SOURCE_CHARS} characters."
            ))));
        }
        let scope = Scope {
            organization_id,
            project_id: Some(actor.project_id),
            actor_id: Some(actor.actor_id),
        };
        if let Err(error) = gateway::resolve(state, scope, Tier::Deep).await {
            return Ok(Err(CreateRefusal::Ai(error)));
        }
        let mut transaction = state.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO ai_plan_imports (id, organization_id, project_id, created_by_actor_id, title, source_kind, state, source_text)
            VALUES ($1, $2, $3, $4, $5, $6, 'drafting', $7)
            "#,
        )
        .bind(id)
        .bind(organization_id)
        .bind(actor.project_id)
        .bind(actor.actor_id)
        .bind(title)
        .bind(kind)
        .bind(&text)
        .execute(&mut *transaction)
        .await?;
        jobs::enqueue(
            &mut *transaction,
            NewJob::new(DRAFT_KIND)
                .project(organization_id, actor.project_id)
                .payload(json!({"importId": id}))
                .unique(format!("{DRAFT_KIND}:{id}"))
                .max_attempts(3)
                .priority(10),
        )
        .await?;
        transaction.commit().await?;
    }
    Ok(get(&state.pool, actor.project_id, id)
        .await?
        .map(Ok)
        .unwrap_or(Err(CreateRefusal::NotFound("That plan wasn't found."))))
}

pub async fn get(
    pool: &PgPool,
    project_id: Uuid,
    id: Uuid,
) -> Result<Option<ImportView>, AppError> {
    Ok(sqlx::query_as::<_, ImportView>(&format!(
        "SELECT {IMPORT_COLUMNS} FROM ai_plan_imports import WHERE import.id = $1 AND import.project_id = $2"
    ))
    .bind(id)
    .bind(project_id)
    .fetch_optional(pool)
    .await?)
}

/// The project's imports, newest first, without their drafts' tasks.
pub async fn list(pool: &PgPool, project_id: Uuid) -> Result<Vec<ImportView>, AppError> {
    let mut imports = sqlx::query_as::<_, ImportView>(&format!(
        "SELECT {IMPORT_COLUMNS} FROM ai_plan_imports import WHERE import.project_id = $1 ORDER BY import.created_at DESC LIMIT 50"
    ))
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    for import in &mut imports {
        import.draft = Draft::default();
    }
    Ok(imports)
}

#[derive(Debug)]
pub enum EditRefusal {
    NotFound,
    /// Not a draft any more: drafting, committing or committed.
    WrongState(String),
    VersionConflict(i64),
    Invalid(Vec<String>),
}

impl EditRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "That plan wasn't found.".to_owned(),
            Self::WrongState(state) => match state.as_str() {
                "drafting" => "The plan is still being drafted.".to_owned(),
                "committing" => "The plan is being created already.".to_owned(),
                "committed" => "The plan was created already.".to_owned(),
                _ => "The plan can't be changed now.".to_owned(),
            },
            Self::VersionConflict(_) => {
                "Someone changed the plan since you opened it. Reload and try again.".to_owned()
            }
            Self::Invalid(problems) => problems.join(" "),
        }
    }
}

async fn lock_import(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    id: Uuid,
) -> Result<Option<(String, i64)>, AppError> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        "SELECT state, version FROM ai_plan_imports WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Saves an edited draft.
pub async fn update_draft(
    pool: &PgPool,
    project_id: Uuid,
    id: Uuid,
    expected_version: i64,
    title: Option<&str>,
    draft: &Draft,
) -> Result<Result<(), EditRefusal>, AppError> {
    let problems = check(draft);
    if !problems.is_empty() {
        return Ok(Err(EditRefusal::Invalid(problems)));
    }
    if title.is_some_and(|title| !(1..=200).contains(&title.trim().chars().count())) {
        return Ok(Err(EditRefusal::Invalid(vec![
            "Give the plan a title of 1 to 200 characters.".to_owned(),
        ])));
    }
    let mut transaction = pool.begin().await?;
    let Some((state, version)) = lock_import(&mut transaction, project_id, id).await? else {
        return Ok(Err(EditRefusal::NotFound));
    };
    if state != "draft" && state != "failed" {
        return Ok(Err(EditRefusal::WrongState(state)));
    }
    if version != expected_version {
        return Ok(Err(EditRefusal::VersionConflict(version)));
    }
    sqlx::query(
        r#"
        UPDATE ai_plan_imports
        SET draft = $2, title = COALESCE($3, title), state = 'draft', error = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(sqlx::types::Json(draft))
    .bind(title.map(str::trim))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Ok(()))
}

/// Queues the draft to be created, as `actor_id`.
pub async fn commit(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
    id: Uuid,
    expected_version: i64,
    actor_id: Uuid,
) -> Result<Result<(), EditRefusal>, AppError> {
    let mut transaction = pool.begin().await?;
    let Some((state, version)) = lock_import(&mut transaction, project_id, id).await? else {
        return Ok(Err(EditRefusal::NotFound));
    };
    // A failed commit can be committed again: it resumes.
    let failed_commit = state == "failed"
        && sqlx::query_scalar::<_, bool>(
            "SELECT committed_by_actor_id IS NOT NULL FROM ai_plan_imports WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    if state != "draft" && !failed_commit {
        return Ok(Err(EditRefusal::WrongState(state)));
    }
    if version != expected_version {
        return Ok(Err(EditRefusal::VersionConflict(version)));
    }
    let draft = sqlx::query_scalar::<_, sqlx::types::Json<Draft>>(
        "SELECT draft FROM ai_plan_imports WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let problems = check(&draft);
    if !problems.is_empty() {
        return Ok(Err(EditRefusal::Invalid(problems)));
    }
    sqlx::query(
        r#"
        UPDATE ai_plan_imports
        SET state = 'committing', committed_by_actor_id = $2, error = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(actor_id)
    .execute(&mut *transaction)
    .await?;
    jobs::enqueue(
        &mut *transaction,
        NewJob::new(COMMIT_KIND)
            .project(organization_id, project_id)
            .payload(json!({"importId": id}))
            .unique(format!("{COMMIT_KIND}:{id}"))
            .max_attempts(5)
            .priority(10),
    )
    .await?;
    transaction.commit().await?;
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// Jobs

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(
        DRAFT_KIND,
        |context| async move { draft_job(context).await },
    );
    registry.register(
        COMMIT_KIND,
        |context| async move { commit_job(context).await },
    );
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportPayload {
    import_id: Uuid,
}

async fn fail(pool: &PgPool, id: Uuid, message: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE ai_plan_imports SET state = 'failed', error = left($2, 500), source_text = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct Drafting {
    organization_id: Uuid,
    project_id: Uuid,
    created_by_actor_id: Uuid,
    title: String,
    source_text: Option<String>,
    state: String,
}

async fn draft_job(context: JobContext) -> Result<(), JobError> {
    let payload: ImportPayload = context.job.payload_as()?;
    let state = &context.state;
    let Some(import) = sqlx::query_as::<_, Drafting>(
        "SELECT organization_id, project_id, created_by_actor_id, title, source_text, state FROM ai_plan_imports WHERE id = $1",
    )
    .bind(payload.import_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(());
    };
    if import.state != "drafting" {
        return Ok(());
    }
    let last_attempt = context.job.attempts >= context.job.max_attempts;
    match draft_from_text(state, &import).await {
        Ok(draft) => {
            sqlx::query(
                r#"
                UPDATE ai_plan_imports
                SET draft = $2, state = 'draft', source_text = NULL, error = NULL,
                    version = version + 1, updated_at = now()
                WHERE id = $1 AND state = 'drafting'
                "#,
            )
            .bind(payload.import_id)
            .bind(sqlx::types::Json(&draft))
            .execute(&state.pool)
            .await?;
            Ok(())
        }
        Err(error) if error.retryable() && !last_attempt => Err(JobError::retry(error.message())),
        Err(error) => {
            fail(&state.pool, payload.import_id, &error.message()).await?;
            Ok(())
        }
    }
}

async fn draft_from_text(state: &AppState, import: &Drafting) -> Result<Draft, AiError> {
    // The person who asked must still see the project.
    let allowed = super::detect::actor_can_access(
        &state.pool,
        import.created_by_actor_id,
        import.project_id,
    )
    .await?;
    if !allowed {
        return Err(AiError::NotConfigured(
            "The person who asked can no longer see this project.".to_owned(),
        ));
    }
    let scope = Scope {
        organization_id: import.organization_id,
        project_id: Some(import.project_id),
        actor_id: Some(import.created_by_actor_id),
    };
    let resolved = gateway::resolve(state, scope, Tier::Deep).await?;
    let parts = split_parts(import.source_text.as_deref().unwrap_or_default());
    let total = parts.len();
    let mut drafts = Vec::with_capacity(total);
    for (number, part) in parts.into_iter().enumerate() {
        let answer = gateway::run_with::<Draft>(
            state,
            scope,
            &prompts::PLAN_TO_TASKS,
            &json!({"title": import.title, "plan": part, "part": number + 1, "parts": total}),
            Tier::Deep,
            &resolved,
        )
        .await?;
        drafts.push(answer.value);
    }
    let draft = merge_parts(drafts);
    if draft.tasks.is_empty() {
        return Err(AiError::NotConfigured(
            "The model found no tasks in that plan.".to_owned(),
        ));
    }
    Ok(draft)
}

async fn commit_job(context: JobContext) -> Result<(), JobError> {
    let payload: ImportPayload = context.job.payload_as()?;
    loop {
        match commit_next_step(&context.state.pool, payload.import_id).await {
            Ok(Step::Progress) => continue,
            Ok(Step::Done) => {
                announce(&context.state.pool, payload.import_id).await?;
                return Ok(());
            }
            Ok(Step::Failed(message)) => {
                fail(&context.state.pool, payload.import_id, &message).await?;
                announce(&context.state.pool, payload.import_id).await?;
                return Ok(());
            }
            Err(error) => {
                if context.job.attempts >= context.job.max_attempts {
                    fail(
                        &context.state.pool,
                        payload.import_id,
                        "Creating the plan failed. Commit it again to pick up where it stopped.",
                    )
                    .await?;
                    return Ok(());
                }
                return Err(error.into());
            }
        }
    }
}

/// Tells the person who committed a plan that it was created, or stopped.
async fn announce(pool: &PgPool, import_id: Uuid) -> Result<(), AppError> {
    let Some((project_id, actor_id, title, state)) = sqlx::query_as::<
        _,
        (Uuid, Option<Uuid>, String, String),
    >(
        "SELECT project_id, committed_by_actor_id, title, state FROM ai_plan_imports WHERE id = $1",
    )
    .bind(import_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };
    let Some(actor_id) = actor_id else {
        return Ok(());
    };
    let message = if state == "committed" {
        format!("Your plan \"{title}\" is now tasks")
    } else {
        format!("Creating your plan \"{title}\" stopped")
    };
    super::notify(
        pool,
        project_id,
        actor_id,
        "ai_plan_import",
        import_id,
        message,
        format!("/projects/{project_id}/plan-import?plan={import_id}"),
    )
    .await
}

/// What one commit step did.
#[derive(Debug, Eq, PartialEq)]
pub enum Step {
    Progress,
    Done,
    /// Stopped for good; the message is for the person.
    Failed(String),
}

#[derive(FromRow)]
struct Committing {
    state: String,
    committed_by_actor_id: Option<Uuid>,
    draft: sqlx::types::Json<Draft>,
}

/// Does the next step of committing an import in one transaction, and says
/// whether there's more. Public for the tests, which stop and resume.
pub async fn commit_next_step(pool: &PgPool, import_id: Uuid) -> Result<Step, AppError> {
    let project_id =
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM ai_plan_imports WHERE id = $1")
            .bind(import_id)
            .fetch_optional(pool)
            .await?;
    let Some(project_id) = project_id else {
        return Ok(Step::Done);
    };
    let committing = sqlx::query_as::<_, Committing>(
        "SELECT state, committed_by_actor_id, draft FROM ai_plan_imports WHERE id = $1",
    )
    .bind(import_id)
    .fetch_one(pool)
    .await?;
    if committing.state != "committing" {
        return Ok(Step::Done);
    }
    let Some(actor_id) = committing.committed_by_actor_id else {
        return Ok(Step::Failed("Nobody committed the plan.".to_owned()));
    };
    let Some(actor) = background_actor(pool, actor_id, project_id).await? else {
        return Ok(Step::Failed(
            "The person who committed the plan can no longer change tasks here.".to_owned(),
        ));
    };
    let mut transaction = pool.begin().await?;
    // Everything below acts as the committer, so stop if they lost the
    // project or the right to change tasks (principle 9).
    let Some(membership) = lock_membership(&mut transaction, actor_id, project_id).await? else {
        return Ok(Step::Failed(
            "The person who committed the plan can no longer change tasks here.".to_owned(),
        ));
    };
    if membership.project_archived_at.is_some()
        || !authorize_mutation(&actor, &membership, Action::WorkItemEdit, None)?.is_allowed()
    {
        return Ok(Step::Failed(
            "The person who committed the plan can no longer change tasks here.".to_owned(),
        ));
    }
    let Some((state, _)) = lock_import(&mut transaction, project_id, import_id).await? else {
        return Ok(Step::Done);
    };
    if state != "committing" {
        return Ok(Step::Done);
    }
    let draft = committing.draft.0;
    let done = sqlx::query_as::<_, (String, String, Option<Uuid>, Option<Uuid>)>(
        "SELECT kind, ref, work_item_id, milestone_id FROM ai_plan_import_items WHERE import_id = $1",
    )
    .bind(import_id)
    .fetch_all(&mut *transaction)
    .await?;
    let mut milestones = HashMap::new();
    let mut tasks = HashMap::new();
    let mut dependencies = HashSet::new();
    for (kind, reference, work_item_id, milestone_id) in done {
        match kind.as_str() {
            "milestone" => {
                milestones.insert(reference, milestone_id);
            }
            "task" => {
                tasks.insert(reference, work_item_id);
            }
            _ => {
                dependencies.insert(reference);
            }
        }
    }

    // 1. Milestones.
    let missing = draft
        .milestones
        .iter()
        .filter(|milestone| !milestones.contains_key(&milestone.reference))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        for milestone in missing {
            let name = planning::normalize_milestone_name(&milestone.name)
                .unwrap_or_else(|| milestone.reference.clone());
            let (id, _) = planning::create_milestone(
                &mut transaction,
                &actor,
                &name,
                planning::normalize_milestone_description(&milestone.description)
                    .as_deref()
                    .unwrap_or_default(),
                milestone
                    .target_date
                    .as_deref()
                    .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()),
            )
            .await?;
            record(
                &mut transaction,
                import_id,
                "milestone",
                &milestone.reference,
                None,
                Some(id),
            )
            .await?;
        }
        transaction.commit().await?;
        return Ok(Step::Progress);
    }

    // 2. Tasks, parents first, up to a hundred at a time.
    let order = parent_order(&draft).unwrap_or_else(|| (0..draft.tasks.len()).collect());
    let mut batch: Vec<&DraftTask> = Vec::new();
    let mut in_batch = HashSet::new();
    for index in order {
        let task = &draft.tasks[index];
        if tasks.contains_key(&task.reference) {
            continue;
        }
        let parent_ready = task.parent.as_ref().is_none_or(|parent| {
            tasks.get(parent).is_some_and(Option::is_some) || in_batch.contains(parent.as_str())
        });
        if !parent_ready {
            continue;
        }
        in_batch.insert(task.reference.as_str());
        batch.push(task);
        if batch.len() == TASKS_PER_STEP {
            break;
        }
    }
    if !batch.is_empty() {
        let inputs = batch
            .iter()
            .map(|task| ImportTask {
                reference: task.reference.clone(),
                title: task.title.trim().to_owned(),
                description: task.description.clone(),
                priority: Some(task.priority.clone()),
                labels: task
                    .labels
                    .iter()
                    .map(|label| label.trim().to_owned())
                    .collect(),
                criteria: task
                    .criteria
                    .iter()
                    .map(|criterion| criterion.trim().to_owned())
                    .collect(),
                parent_in_batch: task
                    .parent
                    .clone()
                    .filter(|parent| in_batch.contains(parent.as_str())),
                parent_existing: task
                    .parent
                    .as_ref()
                    .and_then(|parent| tasks.get(parent).copied().flatten()),
            })
            .collect::<Vec<_>>();
        let created = match create_for_import(&mut transaction, &actor, &inputs).await? {
            Ok(created) => created,
            Err(message) => {
                transaction.rollback().await?;
                return Ok(Step::Failed(format!(
                    "Some tasks couldn't be created. {message}"
                )));
            }
        };
        let mut by_milestone: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for (task, id) in batch.iter().zip(&created) {
            record(
                &mut transaction,
                import_id,
                "task",
                &task.reference,
                Some(*id),
                None,
            )
            .await?;
            if let Some(milestone) = task
                .milestone
                .as_ref()
                .and_then(|milestone| milestones.get(milestone).copied().flatten())
            {
                by_milestone.entry(milestone).or_default().push(*id);
            }
        }
        for (milestone, ids) in by_milestone {
            planning::move_tasks_to_milestone(
                &mut transaction,
                &actor,
                &ids,
                Some(milestone),
                None,
            )
            .await?;
        }
        transaction.commit().await?;
        return Ok(Step::Progress);
    }

    // 3. Dependencies.
    let mut step = 0;
    for task in &draft.tasks {
        let Some(Some(blocked)) = tasks.get(&task.reference).copied() else {
            continue;
        };
        for blocker_ref in &task.blocked_by {
            let key = format!("{}<{}", task.reference, blocker_ref);
            if dependencies.contains(&key) || key.len() > 200 {
                continue;
            }
            let created = match tasks.get(blocker_ref).copied().flatten() {
                Some(blocker) => planning::change_dependencies(
                    &mut transaction,
                    &actor,
                    blocked,
                    &[(blocker, Direction::BlockedBy)],
                    false,
                )
                .await?
                .is_ok(),
                None => false,
            };
            // A refused dependency (it would loop) is recorded without a
            // task, so a resume doesn't try it again.
            record(
                &mut transaction,
                import_id,
                "dependency",
                &key,
                created.then_some(blocked),
                None,
            )
            .await?;
            step += 1;
            if step == DEPENDENCIES_PER_STEP {
                transaction.commit().await?;
                return Ok(Step::Progress);
            }
        }
    }

    // 4. Done.
    sqlx::query(
        r#"
        UPDATE ai_plan_imports
        SET state = 'committed', committed_at = now(), version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(import_id)
    .execute(&mut *transaction)
    .await?;
    crate::work_item_commands::record_audit_for(
        &mut transaction,
        &actor,
        "ai_plan_import",
        "ai_plan_import.committed",
        import_id,
        json!({"tasks": tasks.len(), "milestones": milestones.len()}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Step::Done)
}

async fn record(
    transaction: &mut Transaction<'_, Postgres>,
    import_id: Uuid,
    kind: &str,
    reference: &str,
    work_item_id: Option<Uuid>,
    milestone_id: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO ai_plan_import_items (import_id, kind, ref, work_item_id, milestone_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(import_id)
    .bind(kind)
    .bind(reference)
    .bind(work_item_id)
    .bind(milestone_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roadmap_reads_as_a_valid_draft() {
        let roadmap: Value =
            serde_json::from_str(include_str!("../../../../docs/plans/roadmap.json"))
                .expect("json");
        let draft = read_structured(&roadmap).expect("a plan");
        assert_eq!(
            draft.tasks.len(),
            roadmap["items"].as_array().expect("items").len()
        );
        assert_eq!(
            draft.milestones.len(),
            roadmap["workstreams"]
                .as_array()
                .expect("workstreams")
                .len()
        );
        assert_eq!(check(&draft), Vec::<String>::new());
        assert!(draft.tasks.iter().any(|task| !task.blocked_by.is_empty()));
        assert!(draft.tasks.iter().all(|task| task.milestone.is_some()));
    }

    #[test]
    fn drafts_are_checked() {
        let task = |reference: &str, parent: Option<&str>| DraftTask {
            reference: reference.to_owned(),
            title: "Do it".to_owned(),
            description: String::new(),
            criteria: Vec::new(),
            labels: Vec::new(),
            priority: no_priority(),
            milestone: None,
            parent: parent.map(str::to_owned),
            blocked_by: Vec::new(),
        };
        let looped = Draft {
            milestones: Vec::new(),
            tasks: vec![task("a", Some("b")), task("b", Some("a"))],
        };
        assert_eq!(check(&looped), vec!["Some tasks are each other's parents."]);
        let mut unknown = Draft {
            milestones: Vec::new(),
            tasks: vec![task("a", None), task("a", Some("zz"))],
        };
        unknown.tasks[0].blocked_by = vec!["nope".to_owned()];
        assert_eq!(check(&unknown).len(), 3);
        let fine = Draft {
            milestones: Vec::new(),
            tasks: vec![task("child", Some("parent")), task("parent", None)],
        };
        assert_eq!(check(&fine), Vec::<String>::new());
        assert_eq!(parent_order(&fine), Some(vec![1, 0]));
    }

    #[test]
    fn parts_keep_refs_apart() {
        let part = |name: &str| Draft {
            milestones: vec![DraftMilestone {
                reference: "m1".to_owned(),
                name: name.to_owned(),
                description: String::new(),
                target_date: Some("not a date".to_owned()),
            }],
            tasks: vec![DraftTask {
                reference: "t1".to_owned(),
                title: "Task".to_owned(),
                description: String::new(),
                criteria: vec![" ".to_owned()],
                labels: Vec::new(),
                priority: "whenever".to_owned(),
                milestone: Some("m1".to_owned()),
                parent: Some("t1".to_owned()),
                blocked_by: vec!["missing".to_owned()],
            }],
        };
        let merged = merge_parts(vec![part("One"), part("Two")]);
        assert_eq!(merged.tasks[0].reference, "p1-t1");
        assert_eq!(merged.tasks[1].reference, "p2-t1");
        assert_eq!(merged.tasks[1].milestone.as_deref(), Some("p2-m1"));
        assert_eq!(merged.tasks[0].parent, None);
        assert!(merged.tasks[0].blocked_by.is_empty() && merged.tasks[0].criteria.is_empty());
        assert_eq!(merged.tasks[0].priority, "no_priority");
        assert_eq!(merged.milestones[0].target_date, None);
        assert_eq!(check(&merged), Vec::<String>::new());
        let text = format!(
            "# One\n{}\n# Two\n{}\n",
            "a".repeat(30_000),
            "b".repeat(30_000)
        );
        assert_eq!(split_parts(&text).len(), 2);
    }
}
