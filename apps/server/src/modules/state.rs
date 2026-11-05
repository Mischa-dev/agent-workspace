//! Which modules are on.
//!
//! A workspace switches each module with a `modules.<id>` setting at
//! workspace scope (unset, it's on). A module is on when its switch is and
//! every module it needs is on too, so turning Projects off turns Channels,
//! Tasks and the rest off with it. Roles narrow that further for the people
//! who hold them. Off means hidden, never deleted: nothing is removed, and
//! turning a module back on brings everything back.

use std::collections::BTreeMap;

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::manifest::{self, Manifest};
use crate::error::AppError;

/// The setting that switches a module.
pub fn setting_key(module: &str) -> String {
    format!("modules.{module}")
}

/// A set of core modules, one bit each.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModuleSet(u32);

impl Default for ModuleSet {
    fn default() -> Self {
        Self::all()
    }
}

impl ModuleSet {
    pub fn all() -> Self {
        let count = manifest::core().len();
        Self(if count >= 32 {
            u32::MAX
        } else {
            (1_u32 << count) - 1
        })
    }

    /// Only the modules that can't be switched off.
    pub fn always_on() -> Self {
        let mut set = Self(0);
        for (index, module) in manifest::core().iter().enumerate() {
            if !module.switchable {
                set.0 |= 1 << index;
            }
        }
        set
    }

    /// Whether a module is in the set. A module this server doesn't know
    /// counts as on: switches fail open, so they never hide more than meant.
    pub fn contains(self, module: &str) -> bool {
        manifest::index_of(module).is_none_or(|index| self.0 & (1 << index) != 0)
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    fn without(self, index: usize) -> Self {
        Self(self.0 & !(1 << index))
    }

    /// The modules that are on, in manifest order.
    pub fn on(self) -> impl Iterator<Item = &'static Manifest> {
        manifest::core()
            .iter()
            .enumerate()
            .filter(move |(index, _)| self.0 & (1 << index) != 0)
            .map(|(_, module)| module)
    }

    /// The modules that are off, in manifest order.
    pub fn off(self) -> impl Iterator<Item = &'static Manifest> {
        manifest::core()
            .iter()
            .enumerate()
            .filter(move |(index, _)| self.0 & (1 << index) == 0)
            .map(|(_, module)| module)
    }

    pub fn is_all(self) -> bool {
        self == Self::all()
    }

    /// The set a workspace's switches give: every module whose switch isn't
    /// off and whose needs are all on. Modules that can't be switched off
    /// are always on.
    pub fn from_switches(switched_off: impl Fn(&str) -> bool) -> Self {
        let mut set = Self::all();
        for (index, module) in manifest::core().iter().enumerate() {
            if module.switchable && switched_off(&module.id) {
                set = set.without(index);
            }
        }
        set.closed()
    }

    /// Turns off every module that needs one that's off, until nothing
    /// changes.
    pub fn closed(mut self) -> Self {
        loop {
            let before = self;
            for (index, module) in manifest::core().iter().enumerate() {
                if module.switchable && module.needs.iter().any(|need| !self.contains(need)) {
                    self = self.without(index);
                }
            }
            if self == before {
                return self.union(Self::always_on());
            }
        }
    }

    /// Whether an agent tool may be listed and called.
    pub fn allows_tool(self, tool: &str) -> bool {
        manifest::of_tool(tool).is_none_or(|module| self.contains(&module.id))
    }

    pub fn allows_part(self, part: &str) -> bool {
        manifest::of_part(part).is_none_or(|module| self.contains(&module.id))
    }

    pub fn allows_setup_step(self, step: &str) -> bool {
        manifest::of_setup_step(step).is_none_or(|module| self.contains(&module.id))
    }

    pub fn allows_notification(self, kind: &str) -> bool {
        manifest::of_notification(kind).is_none_or(|module| self.contains(&module.id))
    }

    /// Whether conversations of a project of this kind show: DMs and groups
    /// (in the hidden direct project) need Messages; a project's channels
    /// and threads need Channels.
    pub fn allows_conversations_in(self, project_kind: &str) -> bool {
        if project_kind == "direct" {
            self.contains("messages")
        } else {
            self.contains("channels")
        }
    }

    /// Why a module is off in this set, in words, or none when it's on.
    pub fn why_off(self, module: &Manifest, switched_off: bool) -> Option<String> {
        if self.contains(&module.id) {
            return None;
        }
        if switched_off {
            return Some(format!("{} is turned off in this workspace.", module.name));
        }
        if let Some(need) = module
            .needs
            .iter()
            .filter_map(|need| manifest::get(need))
            .find(|need| !self.contains(&need.id))
        {
            return Some(format!(
                "{} needs {}, which is off in this workspace.",
                module.name, need.name
            ));
        }
        Some(format!("{} is off for your role.", module.name))
    }
}

/// A workspace's stored switches: module id to on or off, with each
/// setting's version.
#[derive(Clone, Debug, Default)]
pub struct Switches {
    pub values: BTreeMap<String, (bool, i64)>,
}

impl Switches {
    pub fn is_off(&self, module: &str) -> bool {
        self.values.get(module).is_some_and(|(on, _)| !on)
    }

    pub fn version(&self, module: &str) -> i64 {
        self.values.get(module).map_or(0, |(_, version)| *version)
    }

    pub fn set(&self) -> ModuleSet {
        ModuleSet::from_switches(|module| self.is_off(module))
    }
}

#[derive(FromRow)]
struct SwitchRow {
    organization_id: Uuid,
    key: String,
    value: serde_json::Value,
    version: i64,
}

/// The switches stored for some workspaces.
async fn switches_of(
    connection: &mut PgConnection,
    organization_ids: &[Uuid],
) -> Result<BTreeMap<Uuid, Switches>, AppError> {
    let rows = sqlx::query_as::<_, SwitchRow>(
        r#"
        SELECT organization_id, key, value, version
        FROM settings
        WHERE organization_id = ANY($1) AND scope = 'workspace' AND key LIKE 'modules.%'
        "#,
    )
    .bind(organization_ids)
    .fetch_all(&mut *connection)
    .await?;
    let mut found: BTreeMap<Uuid, Switches> = organization_ids
        .iter()
        .map(|id| (*id, Switches::default()))
        .collect();
    for row in rows {
        let (Some(module), Some(on)) = (row.key.strip_prefix("modules."), row.value.as_bool())
        else {
            continue;
        };
        found
            .entry(row.organization_id)
            .or_default()
            .values
            .insert(module.to_owned(), (on, row.version));
    }
    Ok(found)
}

/// A workspace's switches.
pub async fn switches(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Switches, AppError> {
    Ok(switches_of(connection, &[organization_id])
        .await?
        .remove(&organization_id)
        .unwrap_or_default())
}

/// The modules on in a workspace.
pub async fn workspace(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<ModuleSet, AppError> {
    Ok(switches(connection, organization_id).await?.set())
}

/// The modules on for someone in a workspace: the workspace's, narrowed by
/// their role there.
pub async fn for_member(
    connection: &mut PgConnection,
    organization_id: Uuid,
    _actor_id: Uuid,
) -> Result<ModuleSet, AppError> {
    workspace(connection, organization_id).await
}

/// The modules on for someone in the workspace a project belongs to; none
/// when there's no such project.
pub async fn for_project(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<ModuleSet>, AppError> {
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&mut *connection)
            .await?;
    match organization_id {
        Some(organization_id) => Ok(Some(
            for_member(connection, organization_id, actor_id).await?,
        )),
        None => Ok(None),
    }
}

/// The modules on for someone in each workspace they're an active member of.
pub async fn by_workspace(
    connection: &mut PgConnection,
    actor_id: Uuid,
) -> Result<BTreeMap<Uuid, ModuleSet>, AppError> {
    let organizations = sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM organization_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let switches = switches_of(connection, &organizations).await?;
    Ok(switches
        .into_iter()
        .map(|(organization_id, switches)| (organization_id, switches.set()))
        .collect())
}

/// Modules on for someone in at least one of their workspaces: what an
/// agent's tool list follows.
pub async fn anywhere(connection: &mut PgConnection, actor_id: Uuid) -> Result<ModuleSet, AppError> {
    let sets = by_workspace(connection, actor_id).await?;
    if sets.is_empty() {
        return Ok(ModuleSet::all());
    }
    Ok(sets
        .into_values()
        .fold(ModuleSet::always_on(), ModuleSet::union))
}

#[derive(FromRow)]
struct ProjectRow {
    id: Uuid,
    organization_id: Uuid,
    kind: String,
}

/// The modules on for someone in each of some projects' workspaces, with
/// each project's kind (`direct` for the hidden project DMs live in,
/// `standard` otherwise).
#[derive(Clone, Debug, Default)]
pub struct ProjectModules {
    projects: Vec<(Uuid, ModuleSet, bool)>,
}

impl ProjectModules {
    pub async fn load(
        connection: &mut PgConnection,
        actor_id: Uuid,
        projects: &[Uuid],
    ) -> Result<Self, AppError> {
        if projects.is_empty() {
            return Ok(Self::default());
        }
        let rows = sqlx::query_as::<_, ProjectRow>(
            "SELECT id, organization_id, kind FROM projects WHERE id = ANY($1)",
        )
        .bind(projects)
        .fetch_all(&mut *connection)
        .await?;
        let mut sets: BTreeMap<Uuid, ModuleSet> = BTreeMap::new();
        for row in &rows {
            if !sets.contains_key(&row.organization_id) {
                let set = for_member(connection, row.organization_id, actor_id).await?;
                sets.insert(row.organization_id, set);
            }
        }
        Ok(Self {
            projects: rows
                .iter()
                .map(|row| {
                    (
                        row.id,
                        sets.get(&row.organization_id).copied().unwrap_or_default(),
                        row.kind == "direct",
                    )
                })
                .collect(),
        })
    }

    /// Whether every module is on in all of them.
    pub fn all_on(&self) -> bool {
        self.projects.iter().all(|(_, set, _)| set.is_all())
    }

    /// Of `projects`, in their order, the ones where `keep` says yes given
    /// the modules on there and whether it's the direct project.
    pub fn keep(&self, projects: &[Uuid], keep: impl Fn(ModuleSet, bool) -> bool) -> Vec<Uuid> {
        projects
            .iter()
            .copied()
            .filter(|id| {
                self.projects
                    .iter()
                    .find(|(project, _, _)| project == id)
                    .is_none_or(|(_, set, direct)| keep(*set, *direct))
            })
            .collect()
    }

    /// Of `projects`, the ones whose workspace has `module` on. For a
    /// project-level module (tasks, pages, ...) the direct project never
    /// counts; for Messages only it does.
    pub fn with(&self, projects: &[Uuid], module: &str) -> Vec<Uuid> {
        self.keep(projects, |set, direct| {
            set.contains(module) && (direct == (module == "messages"))
        })
    }

    /// Of `projects`, the ones whose conversations show: the direct project
    /// with Messages on, others with Channels on.
    pub fn with_conversations(&self, projects: &[Uuid]) -> Vec<Uuid> {
        self.keep(projects, |set, direct| {
            set.allows_conversations_in(if direct { "direct" } else { "standard" })
        })
    }

    /// The modules on in one of them; all when it isn't one of them.
    pub fn of(&self, project_id: Uuid) -> ModuleSet {
        self.projects
            .iter()
            .find(|(project, _, _)| *project == project_id)
            .map(|(_, set, _)| *set)
            .unwrap_or_default()
    }

    /// Whether a notification of `kind` in a project shows: its module is
    /// on there, and for conversations, the one its project's kind needs.
    pub fn allows_notification(&self, project_id: Uuid, kind: &str) -> bool {
        let Some((_, set, direct)) = self
            .projects
            .iter()
            .find(|(project, _, _)| *project == project_id)
        else {
            return true;
        };
        match manifest::of_notification(kind) {
            Some(module) if module.id == "messages" => {
                set.allows_conversations_in(if *direct { "direct" } else { "standard" })
            }
            Some(module) => set.contains(&module.id),
            None => true,
        }
    }
}

/// The switches a module needs on: its own and, through its needs, every
/// module below it.
pub fn required_switches(module: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut queue = vec![module.to_owned()];
    while let Some(next) = queue.pop() {
        let Some(manifest) = manifest::get(&next) else {
            continue;
        };
        let key = setting_key(&manifest.id);
        if manifest.switchable && !found.contains(&key) {
            found.push(key);
        }
        queue.extend(manifest.needs.iter().cloned());
    }
    found.sort();
    found
}

fn sql_array(keys: &[String]) -> String {
    if keys.is_empty() {
        return "ARRAY[]::text[]".to_owned();
    }
    let quoted = keys
        .iter()
        // Keys come from the compiled-in manifests: lower-case letters,
        // dots and underscores only (a manifest test checks module ids).
        .map(|key| format!("'{}'", key.replace('\'', "")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("ARRAY[{quoted}]::text[]")
}

/// A SQL condition, for notification reads, that is true when the
/// notification `n`'s module is switched on in its project's workspace (and
/// every module that one needs). Generated once from the manifests.
pub fn notification_switches_sql() -> &'static str {
    static SQL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SQL.get_or_init(|| {
        let mut cases = String::new();
        for module in manifest::core() {
            for kind in &module.notifications {
                let kind = kind.replace('\'', "");
                let keys = if module.id == "messages" {
                    format!(
                        "CASE WHEN module_project.kind = 'direct' THEN {} ELSE {} END",
                        sql_array(&required_switches("messages")),
                        sql_array(&required_switches("channels")),
                    )
                } else {
                    sql_array(&required_switches(&module.id))
                };
                cases.push_str(&format!(" WHEN '{kind}' THEN {keys}"));
            }
        }
        format!(
            r#"NOT EXISTS (
                SELECT 1
                FROM projects module_project
                JOIN settings module_switch
                  ON module_switch.organization_id = module_project.organization_id
                 AND module_switch.scope = 'workspace'
                 AND module_switch.value = 'false'::jsonb
                WHERE module_project.id = n.project_id
                  AND module_switch.key = ANY(CASE n.kind{cases} ELSE ARRAY[]::text[] END)
            )"#
        )
    })
}

#[cfg(test)]
mod tests {
    use super::ModuleSet;

    #[test]
    fn module_switch_sql_names_every_notification_kind() {
        let sql = super::notification_switches_sql();
        assert!(sql.contains("WHEN 'assigned' THEN ARRAY['modules.projects', 'modules.tasks']::text[]"));
        assert!(sql.contains("WHEN 'mention' THEN CASE WHEN module_project.kind = 'direct'"));
        assert_eq!(
            super::required_switches("runs"),
            [
                "modules.channels",
                "modules.code",
                "modules.messages",
                "modules.projects",
                "modules.runs",
                "modules.tasks"
            ]
        );
    }

    #[test]
    fn needs_carry_switches_down() {
        let set = ModuleSet::from_switches(|module| module == "projects");
        for module in ["channels", "tasks", "pages", "whiteboards", "runs", "code", "automations"] {
            assert!(!set.contains(module), "{module} should be off with projects");
        }
        for module in ["workspace", "messages", "agents", "inbox", "suggestions"] {
            assert!(set.contains(module), "{module} should stay on");
        }
    }

    #[test]
    fn the_workspace_is_always_on_and_unknown_modules_fail_open() {
        let set = ModuleSet::from_switches(|_| true);
        assert!(set.contains("workspace"));
        assert!(!set.contains("messages"));
        assert!(set.contains("a-module-this-server-has-never-heard-of"));
    }

    #[test]
    fn tools_and_parts_follow_their_module() {
        let set = ModuleSet::from_switches(|module| module == "tasks");
        assert!(!set.allows_tool("task_create"));
        assert!(!set.allows_tool("run_start"), "runs need tasks");
        assert!(set.allows_tool("message_send"));
        assert!(set.allows_tool("module_list"));
        assert!(!set.allows_part("area.board"));
        assert!(set.allows_part("area.pages"));
        assert!(!set.allows_notification("assigned"));
        assert!(set.allows_notification("mention"));
    }

    #[test]
    fn conversations_follow_messages_and_channels() {
        let no_channels = ModuleSet::from_switches(|module| module == "channels");
        assert!(no_channels.allows_conversations_in("direct"));
        assert!(!no_channels.allows_conversations_in("standard"));
        let no_messages = ModuleSet::from_switches(|module| module == "messages");
        assert!(!no_messages.allows_conversations_in("direct"));
        // Channels need messages, so they go too.
        assert!(!no_messages.allows_conversations_in("standard"));
    }
}
