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
    /// Open runs for yourself and report on them (A1): any account, for its
    /// own work.
    RunReport,
    /// Ask someone for an approval or an answer (A1).
    RequestRaise,
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
    /// Comment on a task (K1.16: the web and agents both need
    /// "comment on tasks").
    WorkItemComment,
    /// Archive a task, or bring one back.
    WorkItemArchive,
    /// Change a project's task workflow: its statuses and their order.
    WorkflowManage,
    /// Write pages, skills and decisions.
    PageEdit,
    /// Draw on a project's whiteboards.
    WhiteboardEdit,
    /// Add, change, archive and restore a collection's records (AWD-90).
    CollectionRecordEdit,
    /// Create collections and change their fields and views (AWD-90).
    CollectionBuild,
    /// Change a project's sheets: their cells, tabs and names (AWD-54).
    SheetEdit,
    RepositoryView,
    RepositoryManage,
    RepositoryPublish,
    GithubIntegrationManage,
    PullRequestCreate,
    PullRequestRefresh,
    /// Merge a pull request on GitHub (AWD-59): for those who manage the
    /// project's code.
    PullRequestMerge,
    /// Change settings and keys for a workspace or project (owners and
    /// admins; people only).
    SettingsManage,
    /// Define the workspace's roles (owners and admins; people only).
    RoleManage,
    /// Link hosts for the workspace and manage them (A1.11; people only).
    HostManage,
    /// Add or remove others in a group (leaving needs nothing).
    ConversationMembersChange,
    /// Continue, stop or retry the replies accounts were asked for in a
    /// conversation (A2.8), whatever kind of account asks.
    ReplySteer,
}

impl Action {
    pub const ALL: [Self; 49] = [
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
        Self::RunReport,
        Self::RequestRaise,
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
        Self::WorkItemComment,
        Self::WorkItemArchive,
        Self::WorkflowManage,
        Self::PageEdit,
        Self::WhiteboardEdit,
        Self::CollectionRecordEdit,
        Self::CollectionBuild,
        Self::SheetEdit,
        Self::RepositoryView,
        Self::RepositoryManage,
        Self::RepositoryPublish,
        Self::GithubIntegrationManage,
        Self::PullRequestCreate,
        Self::PullRequestRefresh,
        Self::PullRequestMerge,
        Self::SettingsManage,
        Self::RoleManage,
        Self::HostManage,
        Self::ConversationMembersChange,
        Self::ReplySteer,
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
            Self::RunReport => Permission::RunsReport,
            Self::RequestRaise => Permission::RequestsRaise,
            Self::ChannelCreate | Self::ConversationArchive => Permission::ChannelsCreate,
            Self::MessageSend | Self::ThreadCreate | Self::MentionSearch => {
                Permission::ChannelsPost
            }
            Self::MessageModerate => Permission::ChannelsModerate,
            Self::ConversationMembersChange => Permission::ConversationsMembers,
            Self::ReplySteer => Permission::RepliesSteer,
            Self::WorkItemView => Permission::TasksView,
            Self::WorkItemCreate => Permission::TasksCreate,
            Self::WorkItemArchive => Permission::TasksDelete,
            Self::WorkItemComment => Permission::TasksComment,
            Self::WorkflowManage => Permission::TasksWorkflow,
            Self::PageEdit => Permission::PagesEdit,
            Self::WhiteboardEdit => Permission::WhiteboardsEdit,
            Self::CollectionRecordEdit => Permission::CollectionsEdit,
            Self::CollectionBuild => Permission::CollectionsBuild,
            Self::SheetEdit => Permission::SheetsEdit,
            Self::RepositoryView => Permission::CodeView,
            Self::RepositoryManage | Self::GithubIntegrationManage | Self::PullRequestMerge => {
                Permission::CodeManage
            }
            Self::RepositoryPublish | Self::PullRequestCreate | Self::PullRequestRefresh => {
                Permission::CodePublish
            }
            Self::RoleManage => Permission::RolesManage,
            Self::HostManage => Permission::HostsManage,
        }
    }
}

pub use crate::access::catalogue::{
    Level, PERMISSIONS, Permission, PermissionInfo, PermissionScope, PermissionSet,
};

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
                    .implied()
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

    /// The grant with what the account's groups are given in the same
    /// space added (K1.27). A worker's membership takes nothing from them:
    /// it acts only through its run.
    pub fn with_groups(self, groups: PermissionSet) -> Self {
        if self.role == MembershipRole::Worker {
            return self;
        }
        Self {
            role: self.role,
            permissions: self.permissions.union(groups),
        }
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

/// Whether an app acting for `actor_id` is kept from `permission` in a DM
/// or group of `workspace_id` by its agent limit: a kept DM follows its
/// workspace's limit; a personal one (the direct scope) is open to what
/// the limit allows in any workspace. False outside such a call.
pub fn agent_limit_hides_direct(
    actor_id: Uuid,
    workspace_id: Uuid,
    permission: Permission,
) -> bool {
    AGENT_LIMITS
        .try_with(|limits| {
            (limits.actor_id == actor_id).then(|| match limits.organizations.get(&workspace_id) {
                Some(limit) => !limit.contains(permission),
                // A personal DM, in no workspace, follows what the limit
                // allows in any; a kept one in a workspace the limit doesn't
                // name holds nothing.
                None if workspace_id == crate::spaces::DIRECT => !limits
                    .organizations
                    .values()
                    .any(|limit| limit.contains(permission)),
                None => true,
            })
        })
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// While an app acts for `actor_id`: the projects its agent limit allows
/// `permission` in (every other project allows nothing). None outside such
/// a call, meaning no limit applies. For queries that can't filter rows in
/// Rust, such as counts.
/// While an app or token acts for `actor_id`, the workspaces whose DMs and
/// groups its limit leaves `permission` in, with the direct scope (personal
/// ones) when it allows it in any, as `agent_limit_hides_direct` decides one
/// at a time; `None` outside such a call. For list SQL over DMs.
pub fn agent_limit_direct_workspaces(actor_id: Uuid, permission: Permission) -> Option<Vec<Uuid>> {
    AGENT_LIMITS
        .try_with(|limits| {
            (limits.actor_id == actor_id).then(|| {
                let mut allowed = limits
                    .organizations
                    .iter()
                    .filter(|(_, allowed)| allowed.contains(permission))
                    .map(|(workspace, _)| *workspace)
                    .collect::<Vec<_>>();
                if !allowed.is_empty() {
                    allowed.push(crate::spaces::DIRECT);
                }
                allowed
            })
        })
        .ok()
        .flatten()
}

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

/// Why `work_item_commands::authorize_mutation` refused a change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    MembershipDisabled,
    RoleDenied,
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

#[cfg(test)]
mod tests {
    use super::{
        Action, MembershipRole, PERMISSIONS, Permission, PermissionScope, PermissionSet, RoleGrant,
    };
    use crate::access::membership_allows;
    use uuid::Uuid;

    const PROJECT_ID: Uuid = Uuid::from_u128(2);
    const OTHER_PROJECT_ID: Uuid = Uuid::from_u128(3);
    const ACTOR_ID: Uuid = Uuid::from_u128(9);
    const OTHER_ACTOR_ID: Uuid = Uuid::from_u128(10);

    #[test]
    fn every_permission_is_listed_once_with_a_default_for_each_built_in_role() {
        let mut indexes = PERMISSIONS
            .iter()
            .map(|info| info.permission as usize)
            .collect::<Vec<_>>();
        indexes.sort_unstable();
        assert_eq!(indexes, (0..PERMISSIONS.len()).collect::<Vec<_>>());
        assert_eq!(Permission::WorkspaceExport as usize, PERMISSIONS.len() - 1);
        for info in PERMISSIONS {
            assert_eq!(Permission::from_key(info.key), Some(info.permission));
            assert_eq!(info.permission.info().key, info.key);
            // Owners always hold everything, so a workspace can't lock
            // itself out.
            assert!(info.built_in[0], "{} must be held by owners", info.key);
            if info.required {
                assert!(info.built_in.iter().all(|held| *held), "{}", info.key);
            }
            // Exactly one module owns it (M1.3o): its manifest lists it.
            let owners = crate::modules::registry::Registry::global()
                .descriptions()
                .iter()
                .filter(|manifest| manifest.permissions.iter().any(|key| key == info.key))
                .count();
            assert_eq!(owners, 1, "{} is owned by {owners} modules", info.key);
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
        assert!(!directory.required);
        // Projects' member lists stay with "See the member list", which
        // only owners and admins hold.
        let members = Permission::MembersView.info();
        assert_eq!(members.scope, PermissionScope::Both);
        assert_eq!(members.built_in, [true, true, false, false]);
    }

    #[test]
    fn custom_roles_only_narrow_their_base() {
        let reader = RoleGrant::stored("member", Some(&["tasks.view"])).expect("valid in the test");
        let allows =
            |grant, permission| membership_allows(ACTOR_ID, PROJECT_ID, grant, true, permission);
        assert!(allows(reader, Permission::TasksView));
        // Opening the project comes with every role.
        assert!(allows(reader, Action::ProjectView.permission()));
        assert!(!allows(reader, Permission::TasksEdit));
        // Stored keys beyond the base, or unknown ones, grant nothing.
        let custom = RoleGrant::stored(
            "viewer",
            Some(&["tasks.edit", "settings.manage", "nonsense"]),
        )
        .expect("valid in the test");
        let viewer = RoleGrant::built_in(MembershipRole::Viewer);
        for info in PERMISSIONS {
            if allows(custom, info.permission) {
                assert!(allows(viewer, info.permission), "{}", info.key);
            }
        }
        assert!(!allows(custom, Permission::TasksEdit));
        assert!(RoleGrant::stored("someone", None::<&[&str]>).is_err());
    }

    #[test]
    fn workers_and_disabled_memberships_hold_nothing_of_their_own() {
        let owner = RoleGrant::built_in(MembershipRole::Owner);
        let worker = RoleGrant::built_in(MembershipRole::Worker);
        for info in PERMISSIONS {
            assert!(!membership_allows(
                ACTOR_ID,
                PROJECT_ID,
                owner,
                false,
                info.permission
            ));
            assert!(!membership_allows(
                ACTOR_ID,
                PROJECT_ID,
                worker,
                true,
                info.permission
            ));
        }
    }

    #[tokio::test]
    async fn a_call_acting_for_a_person_carries_their_limits_into_every_check() {
        let member = RoleGrant::built_in(MembershipRole::Member);
        let viewer = RoleGrant::built_in(MembershipRole::Viewer);
        let limits = super::AgentLimits {
            actor_id: ACTOR_ID,
            organizations: Default::default(),
            projects: [(
                PROJECT_ID,
                PermissionSet::EMPTY
                    .with(Permission::ProjectsView)
                    .with(Permission::TasksView),
            )]
            .into_iter()
            .collect(),
        };
        let allows = |account, space, grant, permission| {
            membership_allows(account, space, grant, true, permission)
        };
        let (edit, view, limited_viewer, other, elsewhere) =
            super::with_agent_limits(limits, async {
                (
                    allows(ACTOR_ID, PROJECT_ID, member, Permission::TasksEdit),
                    allows(ACTOR_ID, PROJECT_ID, member, Permission::TasksView),
                    allows(ACTOR_ID, PROJECT_ID, viewer, Permission::TasksEdit),
                    allows(OTHER_ACTOR_ID, PROJECT_ID, member, Permission::TasksEdit),
                    allows(ACTOR_ID, OTHER_PROJECT_ID, member, Permission::ProjectsView),
                )
            })
            .await;
        assert!(!edit);
        assert!(view);
        // A limit never adds what the role lacks.
        assert!(!limited_viewer);
        // Only the person the call acts for is limited.
        assert!(other);
        // A space the call didn't load allows nothing.
        assert!(!elsewhere);
        assert!(allows(ACTOR_ID, PROJECT_ID, member, Permission::TasksEdit));
    }
}
