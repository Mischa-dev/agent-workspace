use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorKind {
    Human,
    ExternalAgent,
    ExecutionWorker,
    Integration,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
    Worker,
}

impl MembershipRole {
    pub const ALL: [Self; 5] = [
        Self::Owner,
        Self::Admin,
        Self::Member,
        Self::Viewer,
        Self::Worker,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    OrganizationProjectCreate,
    ProjectView,
    ProjectManage,
    InvitationManage,
    MembershipView,
    MembershipManage,
    ExternalAgentManage,
    RealtimeSubscribe,
    McpRead,
    McpWrite,
    CredentialManage,
    ExecutionProfileView,
    ExecutionProfileManage,
    NavigationRead,
    NavigationWrite,
    WorkerInvoke,
    RunInterrupt,
    ApprovalRespond,
    ChannelCreate,
    MessageSend,
    /// Remove anyone's message (owners and admins; people only, checked by
    /// the caller).
    MessageModerate,
    ThreadCreate,
    ConversationArchive,
    ReadStateUpdate,
    MentionSearch,
    WorkItemView,
    WorkItemCreate,
    WorkItemEdit,
    /// Archive a task, or bring one back.
    WorkItemArchive,
    /// Change a project's task workflow: its statuses and their order.
    WorkflowManage,
    /// Write pages, skills and decisions.
    PageEdit,
    /// Draw on a project's whiteboards.
    WhiteboardEdit,
    RepositoryView,
    RepositoryManage,
    RepositoryPublish,
    GithubIntegrationManage,
    PullRequestCreate,
    PullRequestRefresh,
    /// Change settings and keys for a workspace or project (owners and
    /// admins; people only).
    SettingsManage,
    /// Define the workspace's roles (owners and admins; people only).
    RoleManage,
}

impl Action {
    pub const ALL: [Self; 40] = [
        Self::OrganizationProjectCreate,
        Self::ProjectView,
        Self::ProjectManage,
        Self::InvitationManage,
        Self::MembershipView,
        Self::MembershipManage,
        Self::ExternalAgentManage,
        Self::RealtimeSubscribe,
        Self::McpRead,
        Self::McpWrite,
        Self::CredentialManage,
        Self::ExecutionProfileView,
        Self::ExecutionProfileManage,
        Self::NavigationRead,
        Self::NavigationWrite,
        Self::WorkerInvoke,
        Self::RunInterrupt,
        Self::ApprovalRespond,
        Self::ChannelCreate,
        Self::MessageSend,
        Self::MessageModerate,
        Self::ThreadCreate,
        Self::ConversationArchive,
        Self::ReadStateUpdate,
        Self::MentionSearch,
        Self::WorkItemView,
        Self::WorkItemCreate,
        Self::WorkItemEdit,
        Self::WorkItemArchive,
        Self::WorkflowManage,
        Self::PageEdit,
        Self::WhiteboardEdit,
        Self::RepositoryView,
        Self::RepositoryManage,
        Self::RepositoryPublish,
        Self::GithubIntegrationManage,
        Self::PullRequestCreate,
        Self::PullRequestRefresh,
        Self::SettingsManage,
        Self::RoleManage,
    ];

    /// The permission a role needs for this action.
    pub const fn permission(self) -> Permission {
        match self {
            Self::OrganizationProjectCreate => Permission::ProjectsCreate,
            Self::ProjectView
            | Self::RealtimeSubscribe
            | Self::McpRead
            | Self::NavigationRead
            | Self::NavigationWrite
            | Self::ReadStateUpdate => Permission::ProjectsView,
            Self::ProjectManage => Permission::ProjectsManage,
            Self::InvitationManage => Permission::MembersInvite,
            Self::MembershipView => Permission::MembersView,
            Self::MembershipManage => Permission::MembersManage,
            Self::ExternalAgentManage => Permission::AgentsManage,
            Self::McpWrite | Self::WorkItemEdit => Permission::TasksEdit,
            Self::CredentialManage | Self::SettingsManage => Permission::SettingsManage,
            Self::ExecutionProfileView | Self::ExecutionProfileManage => Permission::RunsAccounts,
            Self::WorkerInvoke => Permission::RunsLaunch,
            Self::RunInterrupt => Permission::RunsInterrupt,
            Self::ApprovalRespond => Permission::RunsApprove,
            Self::ChannelCreate | Self::ConversationArchive => Permission::ChannelsCreate,
            Self::MessageSend | Self::ThreadCreate | Self::MentionSearch => {
                Permission::ChannelsPost
            }
            Self::MessageModerate => Permission::ChannelsModerate,
            Self::WorkItemView => Permission::TasksView,
            Self::WorkItemCreate => Permission::TasksCreate,
            Self::WorkItemArchive => Permission::TasksDelete,
            Self::WorkflowManage => Permission::TasksWorkflow,
            Self::PageEdit => Permission::PagesEdit,
            Self::WhiteboardEdit => Permission::WhiteboardsEdit,
            Self::RepositoryView => Permission::CodeView,
            Self::RepositoryManage | Self::GithubIntegrationManage => Permission::CodeManage,
            Self::RepositoryPublish | Self::PullRequestCreate | Self::PullRequestRefresh => {
                Permission::CodePublish
            }
            Self::RoleManage => Permission::RolesManage,
        }
    }
}

/// What a role lets its holders do, in the words people use. Each action
/// needs exactly one (`Action::permission`); a role holds a set of them.
///
/// Adding one: give it a line in `PERMISSIONS` with its default for every
/// built-in role (anything a role isn't given is denied), map the actions
/// that need it, and add its key to the web app's list
/// (`apps/web/src/settings/workspace/roles.ts`). Custom roles made before it
/// don't hold it until an admin gives it to them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Permission {
    ProjectsView,
    ProjectsCreate,
    ProjectsManage,
    TasksView,
    TasksCreate,
    TasksEdit,
    TasksDelete,
    TasksWorkflow,
    ChannelsPost,
    ChannelsCreate,
    ChannelsModerate,
    PagesEdit,
    WhiteboardsEdit,
    RunsLaunch,
    RunsApprove,
    RunsInterrupt,
    RunsAccounts,
    CodeView,
    CodePublish,
    CodeManage,
    AgentsManage,
    MembersView,
    DirectoryView,
    MembersInvite,
    MembersManage,
    RolesManage,
    SettingsManage,
}

/// Where a permission is checked: in a project (by the role someone has
/// there), in the workspace (by their workspace role), or both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionScope {
    Project,
    Workspace,
    Both,
}

impl PermissionScope {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Workspace => "workspace",
            Self::Both => "both",
        }
    }
}

/// One permission as the Roles page and agents read it.
#[derive(Clone, Copy, Debug)]
pub struct PermissionInfo {
    pub permission: Permission,
    /// Stable key, stored in custom roles: `tasks.edit`.
    pub key: &'static str,
    /// The module it belongs to, which the Roles page groups by.
    pub module: &'static str,
    pub label: &'static str,
    /// A short "what this means".
    pub meaning: &'static str,
    pub scope: PermissionScope,
    /// Whether owner, admin, member and viewer hold it, in that order.
    pub built_in: [bool; 4],
    /// Never for an agent, even when a role and its limit allow it.
    pub people_only: bool,
    /// Every role holds it: without it a role could see nothing at all.
    pub required: bool,
}

const fn permission(
    permission: Permission,
    key: &'static str,
    module: &'static str,
    label: &'static str,
    meaning: &'static str,
    scope: PermissionScope,
    built_in: [bool; 4],
) -> PermissionInfo {
    PermissionInfo {
        permission,
        key,
        module,
        label,
        meaning,
        scope,
        built_in,
        people_only: false,
        required: false,
    }
}

const fn people_only(info: PermissionInfo) -> PermissionInfo {
    PermissionInfo {
        people_only: true,
        ..info
    }
}

const fn required(info: PermissionInfo) -> PermissionInfo {
    PermissionInfo {
        required: true,
        ..info
    }
}

const ALL_ROLES: [bool; 4] = [true, true, true, true];
const WORKERS: [bool; 4] = [true, true, true, false];
const MANAGERS: [bool; 4] = [true, true, false, false];

/// Every permission, in the order the Roles page shows them.
pub const PERMISSIONS: [PermissionInfo; 27] = [
    required(permission(
        Permission::ProjectsView,
        "projects.view",
        "projects",
        "Open projects they're in",
        "See a project, its channels, tasks and pages, and get its updates.",
        PermissionScope::Project,
        ALL_ROLES,
    )),
    permission(
        Permission::ProjectsCreate,
        "projects.create",
        "projects",
        "Create projects",
        "Start new projects in the workspace.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    permission(
        Permission::ProjectsManage,
        "projects.manage",
        "projects",
        "Change and archive projects",
        "Rename a project, change its details, archive it or bring it back.",
        PermissionScope::Project,
        MANAGERS,
    ),
    permission(
        Permission::TasksView,
        "tasks.view",
        "tasks",
        "See tasks",
        "Read tasks, the board, milestones and views.",
        PermissionScope::Project,
        ALL_ROLES,
    ),
    permission(
        Permission::TasksCreate,
        "tasks.create",
        "tasks",
        "Create tasks",
        "Add tasks, import plans and use templates.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::TasksEdit,
        "tasks.edit",
        "tasks",
        "Edit tasks",
        "Change tasks: status, fields, comments, labels and dependencies.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::TasksDelete,
        "tasks.delete",
        "tasks",
        "Archive tasks",
        "Archive tasks, or bring archived ones back.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::TasksWorkflow,
        "tasks.workflow",
        "tasks",
        "Change the workflow",
        "Add, rename and reorder the statuses tasks move through, and the fields every task has.",
        PermissionScope::Project,
        MANAGERS,
    ),
    permission(
        Permission::ChannelsPost,
        "channels.post",
        "channels",
        "Post messages",
        "Write in channels and threads, and mention people and agents.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::ChannelsCreate,
        "channels.create",
        "channels",
        "Create channels",
        "Start channels, and archive or restore them.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::ChannelsModerate,
        "channels.moderate",
        "channels",
        "Moderate",
        "Remove anyone's messages and comments.",
        PermissionScope::Project,
        MANAGERS,
    ),
    permission(
        Permission::PagesEdit,
        "pages.edit",
        "pages",
        "Edit pages and decisions",
        "Write pages and skills, and record decisions.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::WhiteboardsEdit,
        "whiteboards.edit",
        "whiteboards",
        "Draw on whiteboards",
        "Change a project's whiteboards. Anyone in the project can still look at them.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::RunsLaunch,
        "runs.launch",
        "runs",
        "Launch runs",
        "Start agents working on tasks.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::RunsApprove,
        "runs.approve",
        "runs",
        "Answer approvals",
        "Approve or turn down what a run asks to do.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::RunsInterrupt,
        "runs.interrupt",
        "runs",
        "Stop runs",
        "Interrupt or cancel a run that's working.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::RunsAccounts,
        "runs.accounts",
        "runs",
        "Use run accounts",
        "Set up and use their own accounts and environments for runs.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::CodeView,
        "code.view",
        "code",
        "See code",
        "See repositories and pull requests.",
        PermissionScope::Project,
        ALL_ROLES,
    ),
    permission(
        Permission::CodePublish,
        "code.publish",
        "code",
        "Publish changes",
        "Push a run's work and open or refresh pull requests.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::CodeManage,
        "code.manage",
        "code",
        "Manage repositories",
        "Connect GitHub, and add or archive repositories.",
        PermissionScope::Project,
        MANAGERS,
    ),
    people_only(permission(
        Permission::AgentsManage,
        "agents.manage",
        "agents",
        "Manage agents",
        "Connect agents to a project and change or remove them.",
        PermissionScope::Project,
        MANAGERS,
    )),
    permission(
        Permission::MembersView,
        "members.view",
        "members",
        "See the member list",
        "See who's in a project and their roles.",
        PermissionScope::Both,
        MANAGERS,
    ),
    // Every built-in role holds it, so the workspace's member list stays as
    // it was for them. A custom role holds it only when given it: without
    // it, that list shows its holders only themselves. Projects' member
    // lists follow "See the member list" (members.view) instead.
    permission(
        Permission::DirectoryView,
        "directory.view",
        "members",
        "See the workspace member directory",
        "See who's in the workspace on its Members page. Without it, only yourself.",
        PermissionScope::Workspace,
        ALL_ROLES,
    ),
    permission(
        Permission::MembersInvite,
        "members.invite",
        "members",
        "Invite people",
        "Send and revoke invitations.",
        PermissionScope::Both,
        MANAGERS,
    ),
    permission(
        Permission::MembersManage,
        "members.manage",
        "members",
        "Change roles and remove people",
        "Give people roles they hold themselves, and remove or bring people back.",
        PermissionScope::Both,
        MANAGERS,
    ),
    people_only(permission(
        Permission::RolesManage,
        "roles.manage",
        "members",
        "Define roles",
        "Create, change and remove the workspace's roles.",
        PermissionScope::Workspace,
        MANAGERS,
    )),
    people_only(permission(
        Permission::SettingsManage,
        "settings.manage",
        "settings",
        "Manage settings",
        "Change workspace and project settings, keys and layouts.",
        PermissionScope::Both,
        MANAGERS,
    )),
];

/// The modules permissions are grouped by, in order, with their names.
pub const PERMISSION_MODULES: [(&str, &str); 10] = [
    ("projects", "Projects"),
    ("tasks", "Tasks"),
    ("channels", "Channels"),
    ("pages", "Pages and decisions"),
    ("whiteboards", "Whiteboards"),
    ("runs", "Runs"),
    ("code", "Code"),
    ("agents", "Agents"),
    ("members", "Members"),
    ("settings", "Settings"),
];

impl Permission {
    pub fn info(self) -> &'static PermissionInfo {
        PERMISSIONS
            .iter()
            .find(|info| info.permission == self)
            .unwrap_or(&PERMISSIONS[0])
    }

    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PERMISSIONS
            .iter()
            .find(|info| info.key == key)
            .map(|info| info.permission)
    }

    const fn bit(self) -> u64 {
        1 << (self as u8)
    }
}

/// A set of permissions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PermissionSet(u64);

impl PermissionSet {
    pub const EMPTY: Self = Self(0);

    pub fn all() -> Self {
        PERMISSIONS
            .iter()
            .fold(Self::EMPTY, |set, info| set.with(info.permission))
    }

    /// What a built-in role holds. Workers hold nothing of their own: they
    /// act only within what a run delegates.
    pub fn built_in(role: MembershipRole) -> Self {
        let index = match role {
            MembershipRole::Owner => 0,
            MembershipRole::Admin => 1,
            MembershipRole::Member => 2,
            MembershipRole::Viewer => 3,
            MembershipRole::Worker => return Self::EMPTY,
        };
        PERMISSIONS
            .iter()
            .filter(|info| info.built_in[index])
            .fold(Self::EMPTY, |set, info| set.with(info.permission))
    }

    /// The permissions every role holds.
    pub fn required() -> Self {
        PERMISSIONS
            .iter()
            .filter(|info| info.required)
            .fold(Self::EMPTY, |set, info| set.with(info.permission))
    }

    /// Stored keys; unknown ones (from a newer or older server) are skipped.
    pub fn from_keys<S: AsRef<str>>(keys: &[S]) -> Self {
        keys.iter()
            .filter_map(|key| Permission::from_key(key.as_ref()))
            .fold(Self::EMPTY, |set, permission| set.with(permission))
    }

    #[must_use]
    pub const fn with(self, permission: Permission) -> Self {
        Self(self.0 | permission.bit())
    }

    #[must_use]
    pub const fn without(self, permission: Permission) -> Self {
        Self(self.0 & !permission.bit())
    }

    pub const fn contains(self, permission: Permission) -> bool {
        self.0 & permission.bit() != 0
    }

    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Permission> {
        PERMISSIONS
            .iter()
            .map(|info| info.permission)
            .filter(move |permission| self.contains(*permission))
    }

    pub fn keys(self) -> Vec<&'static str> {
        self.iter().map(Permission::key).collect()
    }
}

/// The role a membership holds: its built-in role (which every check that
/// reads the plain role sees) and the permissions it grants, which a custom
/// role narrows from its base.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoleGrant {
    role: MembershipRole,
    permissions: PermissionSet,
}

/// A stored role the server doesn't know.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownRole;

impl RoleGrant {
    pub fn built_in(role: MembershipRole) -> Self {
        Self {
            role,
            permissions: PermissionSet::built_in(role),
        }
    }

    /// A membership as stored: its `role`, and the custom role's permissions
    /// when it holds one (`workspace_role_permissions(role_id)`). A custom
    /// role never holds more than its base, whatever was stored.
    pub fn stored<S: AsRef<str>>(role: &str, custom: Option<&[S]>) -> Result<Self, UnknownRole> {
        let role = MembershipRole::parse(role).ok_or(UnknownRole)?;
        let base = PermissionSet::built_in(role);
        Ok(Self {
            role,
            permissions: match custom {
                None => base,
                // Owners always hold everything.
                Some(_) if role == MembershipRole::Owner => base,
                Some(keys) => PermissionSet::from_keys(keys)
                    .union(PermissionSet::required())
                    .intersect(base),
            },
        })
    }

    pub const fn role(self) -> MembershipRole {
        self.role
    }

    pub const fn permissions(self) -> PermissionSet {
        self.permissions
    }
}

impl MembershipRole {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
            Self::Worker => "worker",
        }
    }
}

tokio::task_local! {
    static AGENT_LIMITS: AgentLimits;
}

/// What an app or agent acting for one person may do in each workspace and
/// project of theirs, for the length of one call. Set by the MCP layer (and
/// so the public API and automations); `authorize` narrows that person's
/// permissions by it. A scope missing from it allows nothing.
#[derive(Clone, Debug, Default)]
pub struct AgentLimits {
    pub actor_id: Uuid,
    pub organizations: std::collections::HashMap<Uuid, PermissionSet>,
    pub projects: std::collections::HashMap<Uuid, PermissionSet>,
}

impl AgentLimits {
    fn for_scope(&self, scope: MembershipScope) -> PermissionSet {
        match scope {
            MembershipScope::Organization(id) => self.organizations.get(&id),
            MembershipScope::Project(id) => self.projects.get(&id),
        }
        .copied()
        .unwrap_or(PermissionSet::EMPTY)
    }
}

/// Run `call` with `limits` applied to every check of that person's
/// permissions inside it.
pub async fn with_agent_limits<F: std::future::Future>(limits: AgentLimits, call: F) -> F::Output {
    AGENT_LIMITS.scope(limits, call).await
}

/// `with_agent_limits` when there are limits; otherwise just `call`. For
/// deferred work a person's app set going, which carries the app's limits
/// as they are when it runs.
pub async fn within_agent_limits<F: std::future::Future>(
    limits: Option<AgentLimits>,
    call: F,
) -> F::Output {
    match limits {
        Some(limits) => with_agent_limits(limits, call).await,
        None => call.await,
    }
}

/// The limit on `actor_id` in `scope` while a call acting for them runs.
pub fn current_agent_limit(actor_id: Uuid, scope: MembershipScope) -> Option<PermissionSet> {
    AGENT_LIMITS
        .try_with(|limits| (limits.actor_id == actor_id).then(|| limits.for_scope(scope)))
        .ok()
        .flatten()
}

/// Whether an app acting for `actor_id` is kept from `permission` in
/// `project_id` by its agent limit. False outside such a call.
pub fn agent_limit_hides(actor_id: Uuid, project_id: Uuid, permission: Permission) -> bool {
    current_agent_limit(actor_id, MembershipScope::Project(project_id))
        .is_some_and(|limit| !limit.contains(permission))
}

/// While an app acts for `actor_id`: the projects its agent limit allows
/// `permission` in (every other project allows nothing). None outside such
/// a call, meaning no limit applies. For queries that can't filter rows in
/// Rust, such as counts.
pub fn agent_limit_projects_allowing(actor_id: Uuid, permission: Permission) -> Option<Vec<Uuid>> {
    AGENT_LIMITS
        .try_with(|limits| {
            (limits.actor_id == actor_id).then(|| {
                limits
                    .projects
                    .iter()
                    .filter(|(_, allowed)| allowed.contains(permission))
                    .map(|(project, _)| *project)
                    .collect()
            })
        })
        .ok()
        .flatten()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipScope {
    Organization(Uuid),
    Project(Uuid),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Membership {
    scope: MembershipScope,
    grant: RoleGrant,
    enabled: bool,
}

impl Membership {
    pub const fn organization(organization_id: Uuid, grant: RoleGrant, enabled: bool) -> Self {
        Self {
            scope: MembershipScope::Organization(organization_id),
            grant,
            enabled,
        }
    }

    pub const fn project(project_id: Uuid, grant: RoleGrant, enabled: bool) -> Self {
        Self {
            scope: MembershipScope::Project(project_id),
            grant,
            enabled,
        }
    }

    pub const fn scope(self) -> MembershipScope {
        self.scope
    }

    pub const fn role(self) -> MembershipRole {
        self.grant.role
    }

    pub const fn grant(self) -> RoleGrant {
        self.grant
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectScope {
    pub project_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub repository_id: Option<Uuid>,
    pub approval_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
}

impl ProjectScope {
    pub const fn project(project_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: None,
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn thread(project_id: Uuid, thread_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn run(project_id: Uuid, thread_id: Uuid, run_id: Uuid, repository_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: Some(run_id),
            repository_id: Some(repository_id),
            approval_id: None,
            work_item_id: None,
        }
    }

    pub const fn approval(
        project_id: Uuid,
        thread_id: Uuid,
        run_id: Uuid,
        repository_id: Uuid,
        approval_id: Uuid,
    ) -> Self {
        Self {
            project_id,
            thread_id: Some(thread_id),
            run_id: Some(run_id),
            repository_id: Some(repository_id),
            approval_id: Some(approval_id),
            work_item_id: None,
        }
    }

    pub const fn work_item(project_id: Uuid, work_item_id: Uuid) -> Self {
        Self {
            project_id,
            thread_id: None,
            run_id: None,
            repository_id: None,
            approval_id: None,
            work_item_id: Some(work_item_id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegatedScope {
    scope: ProjectScope,
    allowed_actions: Vec<Action>,
}

impl DelegatedScope {
    pub fn new(scope: ProjectScope, allowed_actions: impl IntoIterator<Item = Action>) -> Self {
        let mut allowed_actions = allowed_actions.into_iter().collect::<Vec<_>>();
        allowed_actions.sort_unstable_by_key(|action| *action as u8);
        allowed_actions.dedup();

        Self {
            scope,
            allowed_actions,
        }
    }

    pub const fn scope(&self) -> ProjectScope {
        self.scope
    }

    pub fn allows(&self, action: Action, scope: ProjectScope) -> bool {
        self.scope == scope && self.allowed_actions.contains(&action)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subject {
    actor_id: Uuid,
    actor_kind: ActorKind,
    actor_active: bool,
    membership: Option<Membership>,
    delegated_scope: Option<DelegatedScope>,
    /// What an app or agent acting for this person may do, when known
    /// without the call's agent limits (`with_agent_limits`).
    agent_limit: Option<PermissionSet>,
}

impl Subject {
    pub const fn new(
        actor_id: Uuid,
        actor_kind: ActorKind,
        actor_active: bool,
        membership: Option<Membership>,
        delegated_scope: Option<DelegatedScope>,
    ) -> Self {
        Self {
            actor_id,
            actor_kind,
            actor_active,
            membership,
            delegated_scope,
            agent_limit: None,
        }
    }

    /// The same subject acting through an app or agent limited to `limit`.
    #[must_use]
    pub const fn limited_to(mut self, limit: PermissionSet) -> Self {
        self.agent_limit = Some(limit);
        self
    }

    pub const fn actor_id(&self) -> Uuid {
        self.actor_id
    }

    pub const fn actor_kind(&self) -> ActorKind {
        self.actor_kind
    }

    pub const fn is_actor_active(&self) -> bool {
        self.actor_active
    }

    pub const fn membership(&self) -> Option<Membership> {
        self.membership
    }

    pub const fn delegated_scope(&self) -> Option<&DelegatedScope> {
        self.delegated_scope.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalEligibility {
    AnyAuthorizedMember,
    Actor(Uuid),
    Role(MembershipRole),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApprovalRequirements {
    pub requesting_actor_id: Uuid,
    pub eligibility: ApprovalEligibility,
    pub human_required: bool,
    pub separation_of_duty: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    Organization {
        organization_id: Uuid,
    },
    Project(ProjectScope),
    Approval {
        scope: ProjectScope,
        requirements: ApprovalRequirements,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    ActorDisabled,
    MembershipMissing,
    MembershipDisabled,
    ActionResourceMismatch,
    MembershipScopeMismatch,
    RoleDenied,
    DelegatedScopeMissing,
    DelegatedScopeMismatch,
    ApprovalEligibilityMismatch,
    HumanRequired,
    SeparationOfDuty,
    /// The person may, but not through an app or agent.
    AgentLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Deny(DenyReason),
}

impl Decision {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

pub fn authorize(subject: &Subject, action: Action, resource: Resource) -> Decision {
    if !subject.actor_active {
        return Decision::Deny(DenyReason::ActorDisabled);
    }

    let Some(membership) = subject.membership else {
        return Decision::Deny(DenyReason::MembershipMissing);
    };
    if !membership.enabled {
        return Decision::Deny(DenyReason::MembershipDisabled);
    }

    let resource_scope = match (action, resource) {
        (
            Action::OrganizationProjectCreate
            | Action::SettingsManage
            | Action::RoleManage
            | Action::InvitationManage
            | Action::MembershipView
            | Action::MembershipManage,
            Resource::Organization { organization_id },
        ) => MembershipScope::Organization(organization_id),
        (
            Action::ProjectView
            | Action::ProjectManage
            | Action::InvitationManage
            | Action::MembershipView
            | Action::MembershipManage
            | Action::ExternalAgentManage
            | Action::RealtimeSubscribe
            | Action::McpRead
            | Action::McpWrite
            | Action::CredentialManage
            | Action::ExecutionProfileView
            | Action::ExecutionProfileManage
            | Action::NavigationRead
            | Action::NavigationWrite
            | Action::WorkerInvoke
            | Action::RunInterrupt
            | Action::ChannelCreate
            | Action::MessageSend
            | Action::MessageModerate
            | Action::ThreadCreate
            | Action::ConversationArchive
            | Action::ReadStateUpdate
            | Action::MentionSearch
            | Action::WorkItemView
            | Action::WorkItemCreate
            | Action::WorkItemEdit
            | Action::WorkItemArchive
            | Action::WorkflowManage
            | Action::PageEdit
            | Action::WhiteboardEdit
            | Action::RepositoryView
            | Action::RepositoryManage
            | Action::RepositoryPublish
            | Action::GithubIntegrationManage
            | Action::PullRequestCreate
            | Action::PullRequestRefresh
            | Action::SettingsManage,
            Resource::Project(scope),
        ) => MembershipScope::Project(scope.project_id),
        (Action::ApprovalRespond, Resource::Approval { scope, .. }) => {
            MembershipScope::Project(scope.project_id)
        }
        _ => return Decision::Deny(DenyReason::ActionResourceMismatch),
    };

    if membership.scope != resource_scope {
        return Decision::Deny(DenyReason::MembershipScopeMismatch);
    }

    let permission = action.permission();
    if permission.info().people_only && subject.actor_kind != ActorKind::Human {
        return Decision::Deny(DenyReason::HumanRequired);
    }

    if membership.role() == MembershipRole::Worker {
        if !worker_action_is_delegatable(action) {
            return Decision::Deny(DenyReason::RoleDenied);
        }

        let Some(delegated_scope) = subject.delegated_scope.as_ref() else {
            return Decision::Deny(DenyReason::DelegatedScopeMissing);
        };
        let Some(project_scope) = project_scope(resource) else {
            return Decision::Deny(DenyReason::RoleDenied);
        };
        if !delegated_scope.allows(action, project_scope) {
            return Decision::Deny(DenyReason::DelegatedScopeMismatch);
        }
    } else if !membership.grant.permissions.contains(permission) {
        return Decision::Deny(DenyReason::RoleDenied);
    }

    // An app or agent acting for a person: that person's permissions, cut
    // down by their agent limit.
    if subject.actor_kind == ActorKind::Human
        && let Some(limit) = subject
            .agent_limit
            .or_else(|| current_agent_limit(subject.actor_id, membership.scope))
        && !limit.contains(permission)
    {
        return Decision::Deny(DenyReason::AgentLimit);
    }

    if let Resource::Approval { requirements, .. } = resource {
        if !approval_eligibility_matches(
            subject.actor_id,
            membership.role(),
            requirements.eligibility,
        ) {
            return Decision::Deny(DenyReason::ApprovalEligibilityMismatch);
        }
        if requirements.human_required && subject.actor_kind != ActorKind::Human {
            return Decision::Deny(DenyReason::HumanRequired);
        }
        if requirements.separation_of_duty && requirements.requesting_actor_id == subject.actor_id {
            return Decision::Deny(DenyReason::SeparationOfDuty);
        }
    }

    Decision::Allow
}

const fn worker_action_is_delegatable(action: Action) -> bool {
    matches!(
        action,
        Action::ProjectView
            | Action::NavigationRead
            | Action::RunInterrupt
            | Action::ApprovalRespond
    )
}

const fn project_scope(resource: Resource) -> Option<ProjectScope> {
    match resource {
        Resource::Project(scope) | Resource::Approval { scope, .. } => Some(scope),
        Resource::Organization { .. } => None,
    }
}

fn approval_eligibility_matches(
    actor_id: Uuid,
    role: MembershipRole,
    eligibility: ApprovalEligibility,
) -> bool {
    match eligibility {
        ApprovalEligibility::AnyAuthorizedMember => true,
        ApprovalEligibility::Actor(eligible_actor_id) => actor_id == eligible_actor_id,
        ApprovalEligibility::Role(eligible_role) => role == eligible_role,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, DelegatedScope,
        DenyReason, Membership, MembershipRole, PERMISSIONS, Permission, PermissionScope,
        PermissionSet, ProjectScope, Resource, RoleGrant, Subject, authorize,
    };
    use uuid::Uuid;

    const ORGANIZATION_ID: Uuid = Uuid::from_u128(1);
    const PROJECT_ID: Uuid = Uuid::from_u128(2);
    const OTHER_PROJECT_ID: Uuid = Uuid::from_u128(3);
    const THREAD_ID: Uuid = Uuid::from_u128(4);
    const OTHER_THREAD_ID: Uuid = Uuid::from_u128(5);
    const RUN_ID: Uuid = Uuid::from_u128(6);
    const REPOSITORY_ID: Uuid = Uuid::from_u128(7);
    const APPROVAL_ID: Uuid = Uuid::from_u128(8);
    const ACTOR_ID: Uuid = Uuid::from_u128(9);
    const OTHER_ACTOR_ID: Uuid = Uuid::from_u128(10);

    fn project_scope() -> ProjectScope {
        ProjectScope::run(PROJECT_ID, THREAD_ID, RUN_ID, REPOSITORY_ID)
    }

    fn project_resource(action: Action, role: MembershipRole) -> Resource {
        match action {
            Action::OrganizationProjectCreate | Action::RoleManage => Resource::Organization {
                organization_id: ORGANIZATION_ID,
            },
            Action::ApprovalRespond => Resource::Approval {
                scope: ProjectScope::approval(
                    PROJECT_ID,
                    THREAD_ID,
                    RUN_ID,
                    REPOSITORY_ID,
                    APPROVAL_ID,
                ),
                requirements: ApprovalRequirements {
                    requesting_actor_id: OTHER_ACTOR_ID,
                    eligibility: ApprovalEligibility::Role(role),
                    human_required: false,
                    separation_of_duty: false,
                },
            },
            _ => Resource::Project(project_scope()),
        }
    }

    fn membership_for(action: Action, role: MembershipRole, enabled: bool) -> Membership {
        match action {
            Action::OrganizationProjectCreate | Action::RoleManage => {
                Membership::organization(ORGANIZATION_ID, RoleGrant::built_in(role), enabled)
            }
            _ => Membership::project(PROJECT_ID, RoleGrant::built_in(role), enabled),
        }
    }

    fn subject(
        kind: ActorKind,
        action: Action,
        role: MembershipRole,
        actor_active: bool,
        membership_enabled: bool,
        delegated_scope: Option<DelegatedScope>,
    ) -> Subject {
        Subject::new(
            ACTOR_ID,
            kind,
            actor_active,
            Some(membership_for(action, role, membership_enabled)),
            delegated_scope,
        )
    }

    fn worker_delegation(action: Action, resource: Resource) -> Option<DelegatedScope> {
        let scope = match resource {
            Resource::Project(scope) | Resource::Approval { scope, .. } => scope,
            Resource::Organization { .. } => return None,
        };
        Some(DelegatedScope::new(scope, [action]))
    }

    /// The roles' permissions before roles became data, kept as the
    /// reference the built-in roles must still match. Actions added since
    /// then sit with the action they were split from.
    #[test]
    fn exhaustive_ordinary_role_action_matrix() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = (role == MembershipRole::Worker)
                    .then(|| worker_delegation(action, resource))
                    .flatten();
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, true, true, delegation),
                    action,
                    resource,
                );
                let expected = match role {
                    MembershipRole::Owner | MembershipRole::Admin => true,
                    MembershipRole::Member => matches!(
                        action,
                        Action::ProjectView
                            | Action::RealtimeSubscribe
                            | Action::McpRead
                            | Action::McpWrite
                            | Action::ExecutionProfileView
                            | Action::ExecutionProfileManage
                            | Action::NavigationRead
                            | Action::NavigationWrite
                            | Action::WorkerInvoke
                            | Action::RepositoryPublish
                            | Action::PullRequestCreate
                            | Action::PullRequestRefresh
                            | Action::RunInterrupt
                            | Action::ApprovalRespond
                            | Action::ChannelCreate
                            | Action::MessageSend
                            | Action::ThreadCreate
                            | Action::ConversationArchive
                            | Action::ReadStateUpdate
                            | Action::MentionSearch
                            | Action::WorkItemView
                            | Action::WorkItemCreate
                            | Action::WorkItemEdit
                            | Action::RepositoryView
                            // Split from WorkItemEdit.
                            | Action::WorkItemArchive
                            | Action::PageEdit
                            | Action::WhiteboardEdit
                    ),
                    MembershipRole::Viewer => {
                        matches!(
                            action,
                            Action::ProjectView
                                | Action::RealtimeSubscribe
                                | Action::McpRead
                                | Action::NavigationRead
                                | Action::NavigationWrite
                                | Action::ReadStateUpdate
                                | Action::WorkItemView
                                | Action::RepositoryView
                        )
                    }
                    MembershipRole::Worker => matches!(
                        action,
                        Action::ProjectView
                            | Action::NavigationRead
                            | Action::RunInterrupt
                            | Action::ApprovalRespond
                    ),
                };

                assert_eq!(
                    decision.is_allowed(),
                    expected,
                    "{role:?} performing {action:?}: {decision:?}"
                );
            }
        }
    }

    #[test]
    fn only_owners_and_admins_moderate_messages() {
        let action = Action::MessageModerate;
        for role in MembershipRole::ALL {
            let resource = project_resource(action, role);
            let delegation = (role == MembershipRole::Worker)
                .then(|| worker_delegation(action, resource))
                .flatten();
            let decision = authorize(
                &subject(ActorKind::Human, action, role, true, true, delegation),
                action,
                resource,
            );
            assert_eq!(
                decision.is_allowed(),
                matches!(role, MembershipRole::Owner | MembershipRole::Admin),
                "{role:?}: {decision:?}"
            );
        }
    }

    #[test]
    fn disabled_actor_denies_every_action_before_role_or_delegation() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = worker_delegation(action, resource);
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, false, true, delegation),
                    action,
                    resource,
                );
                assert_eq!(
                    decision,
                    Decision::Deny(DenyReason::ActorDisabled),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn disabled_membership_denies_every_action_before_role_or_delegation() {
        for role in MembershipRole::ALL {
            for action in Action::ALL {
                let resource = project_resource(action, role);
                let delegation = worker_delegation(action, resource);
                let decision = authorize(
                    &subject(ActorKind::Human, action, role, true, false, delegation),
                    action,
                    resource,
                );
                assert_eq!(
                    decision,
                    Decision::Deny(DenyReason::MembershipDisabled),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn membership_is_always_required() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = Subject::new(ACTOR_ID, actor_kind, true, None, None);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ProjectView,
                    Resource::Project(ProjectScope::project(PROJECT_ID)),
                ),
                Decision::Deny(DenyReason::MembershipMissing),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn actor_kind_alone_never_creates_a_superuser_bypass() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let viewer = subject(
                actor_kind,
                Action::ProjectManage,
                MembershipRole::Viewer,
                true,
                true,
                None,
            );

            assert_eq!(
                authorize(
                    &viewer,
                    Action::ProjectManage,
                    Resource::Project(ProjectScope::project(PROJECT_ID)),
                ),
                Decision::Deny(DenyReason::RoleDenied),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn human_and_external_agent_with_the_same_role_have_identical_ordinary_authority() {
        for role in [
            MembershipRole::Owner,
            MembershipRole::Admin,
            MembershipRole::Member,
            MembershipRole::Viewer,
        ] {
            for action in Action::ALL {
                if action.permission().info().people_only {
                    continue;
                }
                let resource = project_resource(action, role);
                let human = subject(ActorKind::Human, action, role, true, true, None);
                let external_agent =
                    subject(ActorKind::ExternalAgent, action, role, true, true, None);

                assert_eq!(
                    authorize(&human, action, resource),
                    authorize(&external_agent, action, resource),
                    "{role:?} performing {action:?}"
                );
            }
        }
    }

    #[test]
    fn only_a_human_owner_or_admin_can_manage_external_connections() {
        for role in [MembershipRole::Owner, MembershipRole::Admin] {
            let resource = Resource::Project(ProjectScope::project(PROJECT_ID));
            let human = subject(
                ActorKind::Human,
                Action::ExternalAgentManage,
                role,
                true,
                true,
                None,
            );
            let external = subject(
                ActorKind::ExternalAgent,
                Action::ExternalAgentManage,
                role,
                true,
                true,
                None,
            );
            assert_eq!(
                authorize(&human, Action::ExternalAgentManage, resource),
                Decision::Allow
            );
            assert_eq!(
                authorize(&external, Action::ExternalAgentManage, resource),
                Decision::Deny(DenyReason::HumanRequired)
            );
        }
    }

    #[test]
    fn worker_requires_an_explicit_delegation() {
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            None,
        );

        assert_eq!(
            authorize(
                &subject,
                Action::ProjectView,
                Resource::Project(project_scope()),
            ),
            Decision::Deny(DenyReason::DelegatedScopeMissing)
        );
    }

    #[test]
    fn worker_delegation_must_match_the_action() {
        let scope = project_scope();
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            Some(DelegatedScope::new(scope, [Action::NavigationRead])),
        );

        assert_eq!(
            authorize(&subject, Action::ProjectView, Resource::Project(scope),),
            Decision::Deny(DenyReason::DelegatedScopeMismatch)
        );
    }

    #[test]
    fn worker_delegation_must_match_every_resource_identifier() {
        let delegated = project_scope();
        let subject = subject(
            ActorKind::ExecutionWorker,
            Action::ProjectView,
            MembershipRole::Worker,
            true,
            true,
            Some(DelegatedScope::new(delegated, [Action::ProjectView])),
        );
        let mismatches = [
            ProjectScope {
                project_id: OTHER_PROJECT_ID,
                ..delegated
            },
            ProjectScope {
                thread_id: Some(OTHER_THREAD_ID),
                ..delegated
            },
            ProjectScope {
                run_id: Some(Uuid::from_u128(11)),
                ..delegated
            },
            ProjectScope {
                repository_id: Some(Uuid::from_u128(12)),
                ..delegated
            },
            ProjectScope {
                approval_id: Some(APPROVAL_ID),
                ..delegated
            },
            ProjectScope {
                work_item_id: Some(Uuid::from_u128(13)),
                ..delegated
            },
            ProjectScope::project(PROJECT_ID),
        ];

        for mismatched in mismatches {
            let decision = authorize(&subject, Action::ProjectView, Resource::Project(mismatched));
            let expected = if mismatched.project_id == OTHER_PROJECT_ID {
                Decision::Deny(DenyReason::MembershipScopeMismatch)
            } else {
                Decision::Deny(DenyReason::DelegatedScopeMismatch)
            };
            assert_eq!(decision, expected, "{mismatched:?}");
        }
    }

    #[test]
    fn worker_cannot_delegate_project_management_or_worker_invocation() {
        for action in [
            Action::OrganizationProjectCreate,
            Action::ProjectManage,
            Action::NavigationWrite,
            Action::WorkerInvoke,
        ] {
            let resource = project_resource(action, MembershipRole::Worker);
            let delegation = worker_delegation(action, resource);
            let worker = subject(
                ActorKind::ExecutionWorker,
                action,
                MembershipRole::Worker,
                true,
                true,
                delegation,
            );

            assert_eq!(
                authorize(&worker, action, resource),
                Decision::Deny(DenyReason::RoleDenied),
                "{action:?}"
            );
        }
    }

    #[test]
    fn cross_scope_memberships_do_not_authorize() {
        let project_member = Subject::new(
            ACTOR_ID,
            ActorKind::Human,
            true,
            Some(Membership::project(
                PROJECT_ID,
                RoleGrant::built_in(MembershipRole::Owner),
                true,
            )),
            None,
        );
        assert_eq!(
            authorize(
                &project_member,
                Action::OrganizationProjectCreate,
                Resource::Organization {
                    organization_id: ORGANIZATION_ID,
                },
            ),
            Decision::Deny(DenyReason::MembershipScopeMismatch)
        );

        let organization_member = Subject::new(
            ACTOR_ID,
            ActorKind::Human,
            true,
            Some(Membership::organization(
                ORGANIZATION_ID,
                RoleGrant::built_in(MembershipRole::Owner),
                true,
            )),
            None,
        );
        assert_eq!(
            authorize(
                &organization_member,
                Action::ProjectView,
                Resource::Project(ProjectScope::project(PROJECT_ID)),
            ),
            Decision::Deny(DenyReason::MembershipScopeMismatch)
        );
    }

    #[test]
    fn action_and_resource_must_be_compatible() {
        let owner = subject(
            ActorKind::Human,
            Action::ProjectManage,
            MembershipRole::Owner,
            true,
            true,
            None,
        );

        assert_eq!(
            authorize(
                &owner,
                Action::ProjectManage,
                Resource::Organization {
                    organization_id: ORGANIZATION_ID,
                },
            ),
            Decision::Deny(DenyReason::ActionResourceMismatch)
        );
        assert_eq!(
            authorize(
                &owner,
                Action::ApprovalRespond,
                Resource::Project(ProjectScope::project(PROJECT_ID)),
            ),
            Decision::Deny(DenyReason::ActionResourceMismatch)
        );
    }

    fn approval_resource(
        eligibility: ApprovalEligibility,
        human_required: bool,
        separation_of_duty: bool,
        requesting_actor_id: Uuid,
    ) -> Resource {
        Resource::Approval {
            scope: ProjectScope::approval(
                PROJECT_ID,
                THREAD_ID,
                RUN_ID,
                REPOSITORY_ID,
                APPROVAL_ID,
            ),
            requirements: ApprovalRequirements {
                requesting_actor_id,
                eligibility,
                human_required,
                separation_of_duty,
            },
        }
    }

    fn approval_subject(kind: ActorKind, actor_id: Uuid, role: MembershipRole) -> Subject {
        Subject::new(
            actor_id,
            kind,
            true,
            Some(Membership::project(
                PROJECT_ID,
                RoleGrant::built_in(role),
                true,
            )),
            None,
        )
    }

    #[test]
    fn approval_must_be_addressed_to_the_actor_or_their_exact_role() {
        let member = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Member);

        assert!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
        assert!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Role(MembershipRole::Member),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
        assert_eq!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(OTHER_ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::ApprovalEligibilityMismatch)
        );
        assert_eq!(
            authorize(
                &member,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Role(MembershipRole::Owner),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::ApprovalEligibilityMismatch)
        );
    }

    #[test]
    fn viewer_cannot_answer_even_when_explicitly_addressed() {
        let viewer = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Viewer);

        assert_eq!(
            authorize(
                &viewer,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    false,
                    false,
                    OTHER_ACTOR_ID,
                ),
            ),
            Decision::Deny(DenyReason::RoleDenied)
        );
    }

    #[test]
    fn human_required_approval_rejects_every_non_human_actor_kind() {
        for actor_kind in [
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = approval_subject(actor_kind, ACTOR_ID, MembershipRole::Member);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ApprovalRespond,
                    approval_resource(
                        ApprovalEligibility::Actor(ACTOR_ID),
                        true,
                        false,
                        OTHER_ACTOR_ID,
                    ),
                ),
                Decision::Deny(DenyReason::HumanRequired),
                "{actor_kind:?}"
            );
        }

        let human = approval_subject(ActorKind::Human, ACTOR_ID, MembershipRole::Member);
        assert!(
            authorize(
                &human,
                Action::ApprovalRespond,
                approval_resource(
                    ApprovalEligibility::Actor(ACTOR_ID),
                    true,
                    false,
                    OTHER_ACTOR_ID,
                ),
            )
            .is_allowed()
        );
    }

    #[test]
    fn separation_of_duty_rejects_self_approval_for_every_actor_kind() {
        for actor_kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            let subject = approval_subject(actor_kind, ACTOR_ID, MembershipRole::Member);

            assert_eq!(
                authorize(
                    &subject,
                    Action::ApprovalRespond,
                    approval_resource(ApprovalEligibility::Actor(ACTOR_ID), false, true, ACTOR_ID,),
                ),
                Decision::Deny(DenyReason::SeparationOfDuty),
                "{actor_kind:?}"
            );
        }
    }

    #[test]
    fn explicitly_delegated_worker_approval_still_obeys_approval_conditions() {
        let resource = approval_resource(
            ApprovalEligibility::Actor(ACTOR_ID),
            true,
            false,
            OTHER_ACTOR_ID,
        );
        let scope = match resource {
            Resource::Approval { scope, .. } => scope,
            _ => unreachable!("test resource is an approval"),
        };
        let worker = Subject::new(
            ACTOR_ID,
            ActorKind::ExecutionWorker,
            true,
            Some(Membership::project(
                PROJECT_ID,
                RoleGrant::built_in(MembershipRole::Worker),
                true,
            )),
            Some(DelegatedScope::new(scope, [Action::ApprovalRespond])),
        );

        assert_eq!(
            authorize(&worker, Action::ApprovalRespond, resource),
            Decision::Deny(DenyReason::HumanRequired)
        );
    }

    #[test]
    fn every_permission_is_listed_once_with_a_default_for_each_built_in_role() {
        let mut indexes = PERMISSIONS
            .iter()
            .map(|info| info.permission as usize)
            .collect::<Vec<_>>();
        indexes.sort_unstable();
        assert_eq!(indexes, (0..PERMISSIONS.len()).collect::<Vec<_>>());
        assert_eq!(Permission::SettingsManage as usize, PERMISSIONS.len() - 1);
        for info in PERMISSIONS {
            assert_eq!(Permission::from_key(info.key), Some(info.permission));
            assert_eq!(info.permission.info().key, info.key);
            // Owners always hold everything, so a workspace can't lock
            // itself out.
            assert!(info.built_in[0], "{} must be held by owners", info.key);
            if info.required {
                assert!(info.built_in.iter().all(|held| *held), "{}", info.key);
            }
            assert!(
                super::PERMISSION_MODULES
                    .iter()
                    .any(|(module, _)| *module == info.module),
                "{} names an unknown module",
                info.key
            );
        }
        assert_eq!(
            PermissionSet::built_in(MembershipRole::Owner),
            PermissionSet::all()
        );
        for action in Action::ALL {
            assert!(
                PERMISSIONS
                    .iter()
                    .any(|info| info.permission == action.permission())
            );
        }
    }

    #[test]
    fn the_workspace_directory_is_its_own_workspace_permission() {
        // Every built-in role keeps the workspace's member directory, which
        // anyone in a workspace saw before; a role can still leave it out.
        let directory = Permission::DirectoryView.info();
        assert_eq!(directory.key, "directory.view");
        assert_eq!(directory.scope, PermissionScope::Workspace);
        assert_eq!(directory.built_in, [true; 4]);
        assert!(!directory.required && !directory.people_only);
        // Projects' member lists stay with "See the member list", which
        // only owners and admins hold.
        let members = Permission::MembersView.info();
        assert_eq!(members.scope, PermissionScope::Both);
        assert_eq!(members.built_in, [true, true, false, false]);
    }

    #[test]
    fn custom_roles_only_narrow_their_base() {
        let resource = Resource::Project(ProjectScope::project(PROJECT_ID));
        let holder = |grant: RoleGrant| {
            Subject::new(
                ACTOR_ID,
                ActorKind::Human,
                true,
                Some(Membership::project(PROJECT_ID, grant, true)),
                None,
            )
        };
        let reader =
            holder(RoleGrant::stored("member", Some(&["tasks.view"])).expect("valid in the test"));
        assert!(authorize(&reader, Action::WorkItemView, resource).is_allowed());
        // Opening the project comes with every role.
        assert!(authorize(&reader, Action::ProjectView, resource).is_allowed());
        assert_eq!(
            authorize(&reader, Action::WorkItemEdit, resource),
            Decision::Deny(DenyReason::RoleDenied)
        );
        // Stored keys beyond the base, or unknown ones, grant nothing.
        let viewer = holder(
            RoleGrant::stored(
                "viewer",
                Some(&["tasks.edit", "settings.manage", "nonsense"]),
            )
            .expect("valid in the test"),
        );
        for action in Action::ALL {
            let resource = project_resource(action, MembershipRole::Viewer);
            let membership = membership_for(action, MembershipRole::Viewer, true);
            let custom = Subject::new(
                ACTOR_ID,
                ActorKind::Human,
                true,
                Some(match membership.scope() {
                    super::MembershipScope::Organization(id) => Membership::organization(
                        id,
                        RoleGrant::stored("viewer", Some(&["tasks.edit", "settings.manage"]))
                            .expect("valid in the test"),
                        true,
                    ),
                    super::MembershipScope::Project(id) => Membership::project(
                        id,
                        RoleGrant::stored("viewer", Some(&["tasks.edit", "settings.manage"]))
                            .expect("valid in the test"),
                        true,
                    ),
                }),
                None,
            );
            let built_in = Subject::new(ACTOR_ID, ActorKind::Human, true, Some(membership), None);
            if authorize(&custom, action, resource).is_allowed() {
                assert!(
                    authorize(&built_in, action, resource).is_allowed(),
                    "{action:?}"
                );
            }
        }
        assert_eq!(
            authorize(&viewer, Action::WorkItemEdit, resource),
            Decision::Deny(DenyReason::RoleDenied)
        );
        assert!(RoleGrant::stored("someone", None::<&[&str]>).is_err());
    }

    #[test]
    fn agent_limits_cut_down_a_person_but_not_an_agent_with_its_own_role() {
        let resource = Resource::Project(ProjectScope::project(PROJECT_ID));
        let limit = PermissionSet::EMPTY
            .with(Permission::ProjectsView)
            .with(Permission::TasksView);
        let person = subject(
            ActorKind::Human,
            Action::WorkItemEdit,
            MembershipRole::Member,
            true,
            true,
            None,
        );
        assert!(authorize(&person, Action::WorkItemEdit, resource).is_allowed());
        let through_an_app = person.clone().limited_to(limit);
        assert_eq!(
            authorize(&through_an_app, Action::WorkItemEdit, resource),
            Decision::Deny(DenyReason::AgentLimit)
        );
        assert!(authorize(&through_an_app, Action::WorkItemView, resource).is_allowed());
        // A limit never adds what the role lacks.
        let viewer = subject(
            ActorKind::Human,
            Action::WorkItemEdit,
            MembershipRole::Viewer,
            true,
            true,
            None,
        )
        .limited_to(PermissionSet::all());
        assert_eq!(
            authorize(&viewer, Action::WorkItemEdit, resource),
            Decision::Deny(DenyReason::RoleDenied)
        );
    }

    #[tokio::test]
    async fn a_call_acting_for_a_person_carries_their_limits_into_every_check() {
        let resource = Resource::Project(ProjectScope::project(PROJECT_ID));
        let person = subject(
            ActorKind::Human,
            Action::WorkItemEdit,
            MembershipRole::Member,
            true,
            true,
            None,
        );
        let other_person = Subject::new(
            OTHER_ACTOR_ID,
            ActorKind::Human,
            true,
            Some(Membership::project(
                PROJECT_ID,
                RoleGrant::built_in(MembershipRole::Member),
                true,
            )),
            None,
        );
        let limits = super::AgentLimits {
            actor_id: ACTOR_ID,
            organizations: Default::default(),
            projects: [(
                PROJECT_ID,
                PermissionSet::EMPTY.with(Permission::ProjectsView),
            )]
            .into_iter()
            .collect(),
        };
        let (edit, view, other, elsewhere) = super::with_agent_limits(limits, async {
            (
                authorize(&person, Action::WorkItemEdit, resource),
                authorize(&person, Action::ProjectView, resource),
                authorize(&other_person, Action::WorkItemEdit, resource),
                authorize(
                    &Subject::new(
                        ACTOR_ID,
                        ActorKind::Human,
                        true,
                        Some(Membership::project(
                            OTHER_PROJECT_ID,
                            RoleGrant::built_in(MembershipRole::Member),
                            true,
                        )),
                        None,
                    ),
                    Action::ProjectView,
                    Resource::Project(ProjectScope::project(OTHER_PROJECT_ID)),
                ),
            )
        })
        .await;
        assert_eq!(edit, Decision::Deny(DenyReason::AgentLimit));
        assert!(view.is_allowed());
        // Only the person the call acts for is limited.
        assert!(other.is_allowed());
        // A project the call didn't load allows nothing.
        assert_eq!(elsewhere, Decision::Deny(DenyReason::AgentLimit));
        assert!(authorize(&person, Action::WorkItemEdit, resource).is_allowed());
    }

    #[test]
    fn people_only_permissions_refuse_agents_whatever_their_role() {
        for info in PERMISSIONS.iter().filter(|info| info.people_only) {
            let action = Action::ALL
                .into_iter()
                .find(|action| action.permission() == info.permission)
                .expect("valid in the test");
            let resource = project_resource(action, MembershipRole::Owner);
            let agent = subject(
                ActorKind::ExternalAgent,
                action,
                MembershipRole::Owner,
                true,
                true,
                None,
            );
            assert_eq!(
                authorize(&agent, action, resource),
                Decision::Deny(DenyReason::HumanRequired),
                "{}",
                info.key
            );
        }
    }
}
