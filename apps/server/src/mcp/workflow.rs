//! One reading of a project's workflow: its statuses, types and allowed
//! status changes, how names given by agents resolve against them, and the
//! plain-words explanation when a status change isn't allowed. workflow_get,
//! the task writes and the batch tools all use it, so they agree.

use std::collections::{HashMap, HashSet, VecDeque};

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

#[derive(Clone, Debug, FromRow)]
pub(super) struct StatusDef {
    pub id: Uuid,
    pub name: String,
    /// backlog, unstarted, started, review, verification, completed, or canceled.
    pub category: String,
    pub is_default: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
pub(super) struct TypeDef {
    pub id: Uuid,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Workflow {
    pub project_key: String,
    /// Every status, retired ones too (so a task sitting in one still has a
    /// name), in board order.
    pub statuses: Vec<StatusDef>,
    /// Enabled types, in order.
    pub types: Vec<TypeDef>,
    /// Every allowed change, ordered by the board order of its source, then
    /// its target.
    pub transitions: Vec<(Uuid, Uuid)>,
    allowed: HashSet<(Uuid, Uuid)>,
}

impl Workflow {
    pub(super) async fn load(
        connection: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<Self, AppError> {
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_optional(&mut *connection)
                .await?
                .ok_or(AppError::NotFound("project"))?;
        let statuses = sqlx::query_as::<_, StatusDef>(
            r#"
            SELECT id, name, semantic_category AS category, is_default, enabled
            FROM work_item_statuses
            WHERE project_id = $1
            ORDER BY position, id
            "#,
        )
        .bind(project_id)
        .fetch_all(&mut *connection)
        .await?;
        let types = sqlx::query_as::<_, TypeDef>(
            "SELECT id, name, is_default FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id",
        )
        .bind(project_id)
        .fetch_all(&mut *connection)
        .await?;
        let transitions = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT transition.from_status_id, transition.to_status_id
            FROM work_item_status_transitions transition
            JOIN work_item_statuses source
              ON source.project_id = transition.project_id AND source.id = transition.from_status_id
            JOIN work_item_statuses target
              ON target.project_id = transition.project_id AND target.id = transition.to_status_id
            WHERE transition.project_id = $1
            ORDER BY source.position, target.position, source.id, target.id
            "#,
        )
        .bind(project_id)
        .fetch_all(&mut *connection)
        .await?;
        Ok(Self::new(project_key, statuses, types, transitions))
    }

    pub(super) fn new(
        project_key: String,
        statuses: Vec<StatusDef>,
        types: Vec<TypeDef>,
        transitions: Vec<(Uuid, Uuid)>,
    ) -> Self {
        let allowed = transitions.iter().copied().collect();
        Self {
            project_key,
            statuses,
            types,
            transitions,
            allowed,
        }
    }

    fn enabled_statuses(&self) -> impl Iterator<Item = &StatusDef> {
        self.statuses.iter().filter(|status| status.enabled)
    }

    /// A status by ID, name or group, among the enabled ones. An exact name
    /// wins over a group, so "Review" finds the status called Review even
    /// when another status is in the review group. The error is a message
    /// for the agent.
    pub(super) fn status(&self, value: &str) -> Result<&StatusDef, String> {
        let value = value.trim();
        if let Ok(id) = value.parse::<Uuid>()
            && let Some(status) = self.enabled_statuses().find(|status| status.id == id)
        {
            return Ok(status);
        }
        let named = self
            .enabled_statuses()
            .filter(|status| status.name.to_lowercase() == value.to_lowercase())
            .collect::<Vec<_>>();
        if let [status] = named.as_slice() {
            return Ok(status);
        }
        let grouped = self
            .enabled_statuses()
            .filter(|status| status.category == value.to_lowercase())
            .collect::<Vec<_>>();
        match grouped.as_slice() {
            [status] => Ok(status),
            [] if named.is_empty() => Err(format!(
                "No status called \"{value}\" in {}. Statuses: {}.",
                self.project_key,
                self.enabled_statuses()
                    .map(|status| status.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            _ => {
                let matches = if named.is_empty() { grouped } else { named };
                Err(format!(
                    "\"{value}\" matches more than one status ({}). Use the status's name.",
                    matches
                        .iter()
                        .map(|status| status.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }

    /// A type by ID or name, among the enabled ones.
    #[allow(dead_code)] // task_create and the batch tools resolve types next.
    pub(super) fn work_type(&self, value: &str) -> Result<&TypeDef, String> {
        let value = value.trim();
        let id = value.parse::<Uuid>().ok();
        self.types
            .iter()
            .find(|kind| Some(kind.id) == id || kind.name.to_lowercase() == value.to_lowercase())
            .ok_or_else(|| {
                format!(
                    "No type called \"{value}\" in {}. Types: {}.",
                    self.project_key,
                    self.types
                        .iter()
                        .map(|kind| kind.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    #[allow(dead_code)] // The batch tools fill in defaults next.
    pub(super) fn default_status(&self) -> Option<&StatusDef> {
        self.enabled_statuses().find(|status| status.is_default)
    }

    #[allow(dead_code)] // The batch tools fill in defaults next.
    pub(super) fn default_type(&self) -> Option<&TypeDef> {
        self.types.iter().find(|kind| kind.is_default)
    }

    /// Whether the workflow allows the change, exactly as the shared update
    /// command checks it.
    pub(super) fn allows(&self, from: Uuid, to: Uuid) -> bool {
        self.allowed.contains(&(from, to))
    }

    /// The enabled statuses a task can move to from `from`, in board order.
    pub(super) fn allowed_from(&self, from: Uuid) -> impl Iterator<Item = &StatusDef> {
        self.enabled_statuses()
            .filter(move |status| self.allows(from, status.id))
    }

    /// The fewest status changes from `from` to `to`, not counting `from`.
    /// Every step is an enabled status, since a retired one can't be chosen.
    pub(super) fn shortest_path(&self, from: Uuid, to: Uuid) -> Option<Vec<Uuid>> {
        if from == to {
            return Some(Vec::new());
        }
        let enabled = self
            .enabled_statuses()
            .map(|status| status.id)
            .collect::<HashSet<_>>();
        let mut came_from = HashMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(status) = queue.pop_front() {
            for &(source, target) in &self.transitions {
                if source != status
                    || !enabled.contains(&target)
                    || target == from
                    || came_from.contains_key(&target)
                {
                    continue;
                }
                came_from.insert(target, status);
                if target == to {
                    let mut path = vec![to];
                    let mut step = status;
                    while step != from {
                        path.push(step);
                        step = came_from[&step];
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(target);
            }
        }
        None
    }

    pub(super) fn status_name(&self, id: Uuid) -> &str {
        self.statuses
            .iter()
            .find(|status| status.id == id)
            .map_or("an unknown status", |status| status.name.as_str())
    }

    /// Why a task can't go straight from `from` to `to`, and the way there,
    /// in plain words. None when it can.
    pub(super) fn transition_problem(&self, from: Uuid, to: Uuid) -> Option<String> {
        if from == to || self.allows(from, to) {
            return None;
        }
        let (from_name, to_name) = (self.status_name(from), self.status_name(to));
        let options = self
            .allowed_from(from)
            .map(|status| status.name.as_str())
            .collect::<Vec<_>>();
        let mut message = format!("{from_name} can't move straight to {to_name}.");
        if options.is_empty() {
            message.push_str(&format!(
                " Nothing follows {from_name} in {}'s workflow.",
                self.project_key
            ));
        } else {
            message.push_str(&format!(
                " From {from_name} it can go to {}.",
                join_or(&options)
            ));
        }
        match self.shortest_path(from, to) {
            Some(path) => message.push_str(&format!(
                " To reach {to_name}, go {}, one step per update.",
                path.iter()
                    .map(|id| self.status_name(*id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            None => message.push_str(&format!(
                " {to_name} can't be reached from {from_name} in {}'s workflow (see workflow_get).",
                self.project_key
            )),
        }
        Some(message)
    }
}

/// "A", "A or B", "A, B or C".
fn join_or(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{StatusDef, TypeDef, Workflow};

    /// The workflow every new project starts with (work_items.rs).
    fn default_workflow() -> Workflow {
        let statuses = [
            ("Backlog", "backlog"),
            ("Ready", "unstarted"),
            ("In Progress", "started"),
            ("Review", "review"),
            ("Verification", "verification"),
            ("Done", "completed"),
            ("Canceled", "canceled"),
        ]
        .iter()
        .enumerate()
        .map(|(index, (name, category))| StatusDef {
            id: Uuid::now_v7(),
            name: (*name).to_owned(),
            category: (*category).to_owned(),
            is_default: index == 0,
            enabled: true,
        })
        .collect::<Vec<_>>();
        let id = |name: &str| {
            statuses
                .iter()
                .find(|status| status.name == name)
                .expect("status")
                .id
        };
        let transitions = [
            ("Backlog", "Ready"),
            ("Backlog", "Canceled"),
            ("Ready", "Backlog"),
            ("Ready", "In Progress"),
            ("Ready", "Canceled"),
            ("In Progress", "Ready"),
            ("In Progress", "Review"),
            ("In Progress", "Canceled"),
            ("Review", "In Progress"),
            ("Review", "Verification"),
            ("Review", "Done"),
            ("Review", "Canceled"),
            ("Verification", "In Progress"),
            ("Verification", "Done"),
            ("Verification", "Canceled"),
            ("Done", "In Progress"),
            ("Canceled", "Backlog"),
        ]
        .iter()
        .map(|(from, to)| (id(from), id(to)))
        .collect();
        let types = vec![TypeDef {
            id: Uuid::now_v7(),
            name: "Bug".to_owned(),
            is_default: true,
        }];
        Workflow::new("AWD".to_owned(), statuses, types, transitions)
    }

    fn names(workflow: &Workflow, path: &[Uuid]) -> Vec<String> {
        path.iter()
            .map(|id| workflow.status_name(*id).to_owned())
            .collect()
    }

    fn id(workflow: &Workflow, name: &str) -> Uuid {
        workflow.status(name).expect("status").id
    }

    #[test]
    fn the_shortest_way_uses_only_allowed_enabled_steps() {
        let mut workflow = default_workflow();
        let (backlog, done) = (id(&workflow, "Backlog"), id(&workflow, "Done"));
        assert_eq!(
            names(
                &workflow,
                &workflow.shortest_path(backlog, done).expect("path")
            ),
            ["Ready", "In Progress", "Review", "Done"]
        );
        assert_eq!(workflow.shortest_path(done, done), Some(Vec::new()));
        // Retiring Review leaves no way through to Done but Verification,
        // which only Review leads to.
        let review = id(&workflow, "Review");
        for status in &mut workflow.statuses {
            if status.id == review {
                status.enabled = false;
            }
        }
        assert_eq!(workflow.shortest_path(backlog, done), None);
    }

    #[test]
    fn transition_problems_name_the_options_and_the_way_there() {
        let workflow = default_workflow();
        let (backlog, ready, done) = (
            id(&workflow, "Backlog"),
            id(&workflow, "Ready"),
            id(&workflow, "Done"),
        );
        assert_eq!(workflow.transition_problem(backlog, ready), None);
        assert_eq!(workflow.transition_problem(done, done), None);
        assert_eq!(
            workflow.transition_problem(backlog, done).expect("problem"),
            "Backlog can't move straight to Done. From Backlog it can go to Ready or Canceled. \
             To reach Done, go Ready, In Progress, Review, Done, one step per update."
        );
        assert_eq!(
            workflow
                .allowed_from(backlog)
                .map(|status| status.name.as_str())
                .collect::<Vec<_>>(),
            ["Ready", "Canceled"]
        );
    }

    #[test]
    fn names_win_over_groups_and_unknown_names_list_the_choices() {
        let mut workflow = default_workflow();
        assert_eq!(
            workflow.status(" in progress ").expect("name").name,
            "In Progress"
        );
        assert_eq!(workflow.status("completed").expect("group").name, "Done");
        let ready = id(&workflow, "Ready");
        assert_eq!(
            workflow.status(&ready.to_string()).expect("id").name,
            "Ready"
        );
        assert_eq!(
            workflow.status("Doing").expect_err("unknown"),
            "No status called \"Doing\" in AWD. Statuses: Backlog, Ready, In Progress, Review, Verification, Done, Canceled."
        );
        assert_eq!(
            workflow.work_type("epic").expect_err("unknown type"),
            "No type called \"epic\" in AWD. Types: Bug."
        );
        assert_eq!(workflow.work_type("bug").expect("type").name, "Bug");
        // A second status in the review group: the group is now ambiguous,
        // but the exact name still resolves.
        workflow.statuses.push(StatusDef {
            id: Uuid::now_v7(),
            name: "Design review".to_owned(),
            category: "review".to_owned(),
            is_default: false,
            enabled: true,
        });
        assert_eq!(workflow.status("Review").expect("name").name, "Review");
        assert_eq!(
            workflow.status("review").expect("exact name").name,
            "Review"
        );
        workflow.statuses.retain(|status| status.name != "Review");
        assert_eq!(
            workflow.status("review").expect("only one left").name,
            "Design review"
        );
        workflow.statuses.push(StatusDef {
            id: Uuid::now_v7(),
            name: "Code review".to_owned(),
            category: "review".to_owned(),
            is_default: false,
            enabled: true,
        });
        assert_eq!(
            workflow.status("review").expect_err("ambiguous"),
            "\"review\" matches more than one status (Design review, Code review). Use the status's name."
        );
    }
}
