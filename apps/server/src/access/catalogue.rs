//! The permission catalogue (K1 §3.3, K1.9): every permission a role can
//! hold, its module, where it applies, its built-in defaults, and its shape.
//!
//! Every entry is either a **level on a kind of thing** (`see` < `comment` <
//! `suggest` < `edit_own` < `edit` < `manage`; holding a level implies every
//! lower level on the same kind) or a **named capability** (a verb that is not
//! a level, such as `projects.create`). Stored keys keep their meaning
//! forever. `policy` re-exports all of this for the code that still reads it
//! from there. The SQL side is `permission_catalogue`, kept equal to this list
//! by `roles_and_access::catalogue_matches_sql_and_role_allows_holds_to_the_role`.

use crate::policy::MembershipRole;

/// The levels on a kind of thing, lowest first.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Level {
    See,
    Comment,
    Suggest,
    EditOwn,
    Edit,
    Manage,
}

impl Level {
    pub const fn key(self) -> &'static str {
        match self {
            Self::See => "see",
            Self::Comment => "comment",
            Self::Suggest => "suggest",
            Self::EditOwn => "edit_own",
            Self::Edit => "edit",
            Self::Manage => "manage",
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
    CollectionsEdit,
    CollectionsBuild,
    SheetsEdit,
    RunsLaunch,
    RunsApprove,
    RunsInterrupt,
    RunsAccounts,
    RunsReport,
    RequestsRaise,
    RequestsAnswer,
    CodeView,
    CodePublish,
    CodeManage,
    AgentsManage,
    AgentsManageAny,
    HostsManage,
    MembersView,
    DirectoryView,
    MembersInvite,
    MembersManage,
    RolesManage,
    SettingsManage,
    // K1.9: the levels and capabilities of K1 §3.3. Appended, so stored
    // bits and orderings of the keys above never move.
    SpacesBrowse,
    TasksComment,
    TasksSuggest,
    TasksEditOwn,
    ViewsShare,
    DashboardsView,
    DashboardsEdit,
    ConversationsSee,
    ConversationsMembers,
    DirectStart,
    RepliesSteer,
    PagesSee,
    PagesComment,
    PagesSuggest,
    PagesEditOwn,
    WhiteboardsComment,
    WhiteboardsSuggest,
    WhiteboardsEditOwn,
    CollectionsComment,
    CollectionsSuggest,
    DataConnect,
    DataShare,
    SheetsComment,
    SheetsSuggest,
    SheetsEditOwn,
    RunsEnvironments,
    RequestsAnswerSensitive,
    AgentsCreate,
    WebhooksManage,
    AutomationsManage,
    // M1.6: building built modules and switching them.
    ModulesBuild,
    ModulesManage,
    // H1.24: deleting the whole workspace.
    WorkspaceDelete,
    // K1.19: reading the workspace's audit log.
    SettingsAudit,
    // K1.27: groups, and chat's keys of K1 §3.3.
    MembersGroups,
    ChannelsBrowse,
    ChannelsCreatePrivate,
    ChannelsMentionEveryone,
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
    /// The kind of thing and the level on it, for a level; `None` for a
    /// capability.
    pub level: Option<(&'static str, Level)>,
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
        level: None,
        required: false,
    }
}

/// A level on a kind of thing.
const fn on(kind: &'static str, level: Level, info: PermissionInfo) -> PermissionInfo {
    PermissionInfo {
        level: Some((kind, level)),
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
const OWNERS: [bool; 4] = [true, false, false, false];

/// Every permission, in the order the Roles page shows them.
pub const PERMISSIONS: [PermissionInfo; 73] = [
    on(
        "space",
        Level::See,
        required(permission(
            Permission::ProjectsView,
            "projects.view",
            "projects",
            "Open projects they're in",
            "See a project, its channels, tasks and pages, and get its updates.",
            PermissionScope::Project,
            ALL_ROLES,
        )),
    ),
    permission(
        Permission::ProjectsCreate,
        "projects.create",
        "projects",
        "Create projects",
        "Start new projects in the workspace.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    on(
        "space",
        Level::Manage,
        permission(
            Permission::ProjectsManage,
            "projects.manage",
            "projects",
            "Change and archive projects",
            "Rename a project, change its details, archive it or bring it back.",
            PermissionScope::Project,
            MANAGERS,
        ),
    ),
    permission(
        Permission::SpacesBrowse,
        "spaces.browse",
        "projects",
        "Browse open projects",
        "Find and open projects that are open to the whole workspace without being in them. Guests don't.",
        PermissionScope::Workspace,
        ALL_ROLES,
    ),
    on(
        "task",
        Level::See,
        permission(
            Permission::TasksView,
            "tasks.view",
            "tasks",
            "See tasks",
            "Read tasks, the board, milestones and views.",
            PermissionScope::Project,
            ALL_ROLES,
        ),
    ),
    on(
        "task",
        Level::Comment,
        permission(
            Permission::TasksComment,
            "tasks.comment",
            "tasks",
            "Comment on tasks",
            "Write comments on tasks.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "task",
        Level::Suggest,
        permission(
            Permission::TasksSuggest,
            "tasks.suggest",
            "tasks",
            "Suggest task changes",
            "Propose changes to tasks for someone who edits them to accept.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "task",
        Level::EditOwn,
        permission(
            Permission::TasksEditOwn,
            "tasks.edit_own",
            "tasks",
            "Edit their own tasks",
            "Change tasks they created.",
            PermissionScope::Project,
            WORKERS,
        ),
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
    on(
        "task",
        Level::Edit,
        permission(
            Permission::TasksEdit,
            "tasks.edit",
            "tasks",
            "Edit tasks",
            "Change tasks: status, fields, comments, labels and dependencies.",
            PermissionScope::Project,
            WORKERS,
        ),
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
        Permission::ViewsShare,
        "views.share",
        "tasks",
        "Share views",
        "Save views of tasks and records for everyone in the project.",
        PermissionScope::Project,
        WORKERS,
    ),
    on(
        "dashboard",
        Level::See,
        permission(
            Permission::DashboardsView,
            "dashboards.view",
            "dashboards",
            "See dashboards",
            "Read the project's dashboards.",
            PermissionScope::Project,
            ALL_ROLES,
        ),
    ),
    on(
        "dashboard",
        Level::Edit,
        permission(
            Permission::DashboardsEdit,
            "dashboards.edit",
            "dashboards",
            "Edit dashboards",
            "Build and change the project's dashboards.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "conversation",
        Level::Comment,
        permission(
            Permission::ChannelsPost,
            "channels.post",
            "channels",
            "Post messages",
            "Write in channels and threads, and mention people and agents.",
            PermissionScope::Project,
            WORKERS,
        ),
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
        Permission::ChannelsCreatePrivate,
        "channels.create_private",
        "channels",
        "Create private channels",
        "Start channels only their members can find and read.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::ChannelsBrowse,
        "channels.browse",
        "channels",
        "Browse channels",
        "List the open channels of a project they haven't joined.",
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
        Permission::ChannelsMentionEveryone,
        "channels.mention_everyone",
        "channels",
        "Notify everyone",
        "Reach everyone in a large conversation with @everyone or @here.",
        PermissionScope::Project,
        MANAGERS,
    ),
    on(
        "conversation",
        Level::See,
        required(permission(
            Permission::ConversationsSee,
            "conversations.see",
            "channels",
            "Read conversations they can reach",
            "Read the channels and threads of projects they are in.",
            PermissionScope::Project,
            ALL_ROLES,
        )),
    ),
    permission(
        Permission::ConversationsMembers,
        "conversations.members",
        "channels",
        "Change conversation members",
        "Add and remove people and agents in groups and private channels.",
        PermissionScope::Project,
        WORKERS,
    ),
    permission(
        Permission::DirectStart,
        "direct.start",
        "messages",
        "Start direct messages",
        "Start a direct message or group with someone they share the workspace with.",
        PermissionScope::Workspace,
        WORKERS,
    ),
    permission(
        Permission::RepliesSteer,
        "replies.steer",
        "channels",
        "Steer replies",
        "Stop or retry the reply an agent is writing in a conversation.",
        PermissionScope::Project,
        WORKERS,
    ),
    on(
        "page",
        Level::Edit,
        permission(
            Permission::PagesEdit,
            "pages.edit",
            "pages",
            "Edit pages and decisions",
            "Write pages and skills, and record decisions.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "page",
        Level::See,
        required(permission(
            Permission::PagesSee,
            "pages.see",
            "pages",
            "Read pages",
            "Read pages, skills and decisions.",
            PermissionScope::Project,
            ALL_ROLES,
        )),
    ),
    on(
        "page",
        Level::Comment,
        permission(
            Permission::PagesComment,
            "pages.comment",
            "pages",
            "Comment on pages",
            "Write comments on pages.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "page",
        Level::Suggest,
        permission(
            Permission::PagesSuggest,
            "pages.suggest",
            "pages",
            "Suggest page changes",
            "Propose changes to pages for someone who edits them to accept.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "page",
        Level::EditOwn,
        permission(
            Permission::PagesEditOwn,
            "pages.edit_own",
            "pages",
            "Edit their own pages",
            "Change pages they created.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "whiteboard",
        Level::Edit,
        permission(
            Permission::WhiteboardsEdit,
            "whiteboards.edit",
            "whiteboards",
            "Draw on whiteboards",
            "Change a project's whiteboards. Anyone in the project can still look at them.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "whiteboard",
        Level::Comment,
        permission(
            Permission::WhiteboardsComment,
            "whiteboards.comment",
            "whiteboards",
            "Comment on whiteboards",
            "Write comments on whiteboards.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "whiteboard",
        Level::Suggest,
        permission(
            Permission::WhiteboardsSuggest,
            "whiteboards.suggest",
            "whiteboards",
            "Suggest whiteboard changes",
            "Propose changes to whiteboards for someone who edits them to accept.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "whiteboard",
        Level::EditOwn,
        permission(
            Permission::WhiteboardsEditOwn,
            "whiteboards.edit_own",
            "whiteboards",
            "Draw on their own whiteboards",
            "Change whiteboards they created.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "record",
        Level::Edit,
        permission(
            Permission::CollectionsEdit,
            "collections.edit",
            "collections",
            "Edit records",
            "Add, change, archive and restore records in a project's collections. Anyone in the project can still read them.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "collection",
        Level::Manage,
        permission(
            Permission::CollectionsBuild,
            "collections.build",
            "collections",
            "Build collections",
            "Create collections and change their fields and views.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "record",
        Level::Comment,
        permission(
            Permission::CollectionsComment,
            "collections.comment",
            "collections",
            "Comment on records",
            "Write comments on records.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "record",
        Level::Suggest,
        permission(
            Permission::CollectionsSuggest,
            "collections.suggest",
            "collections",
            "Suggest record changes",
            "Propose changes to records for someone who edits them to accept.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    permission(
        Permission::DataConnect,
        "data.connect",
        "collections",
        "Connect outside data",
        "Connect a collection to data kept somewhere else.",
        PermissionScope::Project,
        MANAGERS,
    ),
    permission(
        Permission::DataShare,
        "data.share",
        "collections",
        "Share data outside",
        "Share a collection with people or services outside the workspace.",
        PermissionScope::Project,
        MANAGERS,
    ),
    on(
        "sheet",
        Level::Edit,
        permission(
            Permission::SheetsEdit,
            "sheets.edit",
            "sheets",
            "Edit sheets",
            "Create sheets and change their cells and tabs. Anyone in the project can still read them.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "sheet",
        Level::Comment,
        permission(
            Permission::SheetsComment,
            "sheets.comment",
            "sheets",
            "Comment on sheets",
            "Write comments on sheets and their cells.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "sheet",
        Level::Suggest,
        permission(
            Permission::SheetsSuggest,
            "sheets.suggest",
            "sheets",
            "Suggest sheet changes",
            "Propose changes to sheets for someone who edits them to accept.",
            PermissionScope::Project,
            WORKERS,
        ),
    ),
    on(
        "sheet",
        Level::EditOwn,
        permission(
            Permission::SheetsEditOwn,
            "sheets.edit_own",
            "sheets",
            "Edit their own sheets",
            "Change sheets they created.",
            PermissionScope::Project,
            WORKERS,
        ),
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
        Permission::RunsEnvironments,
        "runs.environments",
        "runs",
        "Manage environments",
        "Set up and change the environments runs work in.",
        PermissionScope::Project,
        MANAGERS,
    ),
    // A1: every account reports its own work the same way, person or agent.
    permission(
        Permission::RunsReport,
        "runs.report",
        "runs",
        "Report their own runs",
        "Open runs for their own work and report its steps, usage and results.",
        PermissionScope::Project,
        ALL_ROLES,
    ),
    permission(
        Permission::RequestsRaise,
        "requests.raise",
        "runs",
        "Raise requests",
        "Ask someone for an approval or an answer from a run.",
        PermissionScope::Project,
        ALL_ROLES,
    ),
    permission(
        Permission::RequestsAnswer,
        "requests.answer",
        "runs",
        "Answer requests",
        "Answer the questions and proposals people and agents raise.",
        PermissionScope::Project,
        ALL_ROLES,
    ),
    permission(
        Permission::RequestsAnswerSensitive,
        "requests.answer_sensitive",
        "runs",
        "Answer sensitive requests",
        "Answer approvals and questions marked sensitive.",
        PermissionScope::Project,
        WORKERS,
    ),
    on(
        "code",
        Level::See,
        permission(
            Permission::CodeView,
            "code.view",
            "code",
            "See code",
            "See repositories and pull requests.",
            PermissionScope::Project,
            ALL_ROLES,
        ),
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
    on(
        "code",
        Level::Manage,
        permission(
            Permission::CodeManage,
            "code.manage",
            "code",
            "Manage repositories",
            "Connect GitHub, add or archive repositories, and merge pull requests.",
            PermissionScope::Project,
            MANAGERS,
        ),
    ),
    on(
        "agent",
        Level::Manage,
        permission(
            Permission::AgentsManage,
            "agents.manage",
            "agents",
            "Manage agents",
            "Connect agents to a project and change or remove them.",
            PermissionScope::Project,
            MANAGERS,
        ),
    ),
    permission(
        Permission::AgentsCreate,
        "agents.create",
        "agents",
        "Add agents",
        "Add new agents to the workspace.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    // K1.4: the owner starting role only, so a workspace never loses control
    // of an agent whose named managers left.
    permission(
        Permission::AgentsManageAny,
        "agents.manage_any",
        "agents",
        "Manage every agent the workspace owns",
        "Manage any agent or app the workspace owns, without being named as its manager.",
        PermissionScope::Workspace,
        OWNERS,
    ),
    permission(
        Permission::HostsManage,
        "hosts.manage",
        "agents",
        "Manage hosts",
        "Link machines and services that run agents for the workspace, choose which agents they run, and unlink them.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
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
    permission(
        Permission::MembersGroups,
        "members.groups",
        "members",
        "Manage groups",
        "Make, change and archive groups, and choose who is in them.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    permission(
        Permission::RolesManage,
        "roles.manage",
        "members",
        "Define roles",
        "Create, change and remove the workspace's roles.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    permission(
        Permission::SettingsManage,
        "settings.manage",
        "settings",
        "Manage settings",
        "Change workspace and project settings, keys and layouts.",
        PermissionScope::Both,
        MANAGERS,
    ),
    permission(
        Permission::SettingsAudit,
        "settings.audit",
        "settings",
        "Read the audit log",
        "See the workspace's audit log: who changed what, and when.",
        PermissionScope::Workspace,
        MANAGERS,
    ),
    permission(
        Permission::WebhooksManage,
        "webhooks.manage",
        "settings",
        "Manage webhooks",
        "Set up, change and remove outgoing webhooks.",
        PermissionScope::Both,
        MANAGERS,
    ),
    permission(
        Permission::AutomationsManage,
        "automations.manage",
        "settings",
        "Manage automations",
        "See and change every automation rule, not only their own.",
        PermissionScope::Project,
        MANAGERS,
    ),
    // M1.6: built modules.
    permission(
        Permission::ModulesBuild,
        "modules.build",
        "settings",
        "Build modules",
        "Make and change built modules: bundles of collections, forms, automations and navigation.",
        PermissionScope::Both,
        MANAGERS,
    ),
    permission(
        Permission::ModulesManage,
        "modules.manage",
        "settings",
        "Manage modules",
        "Switch built modules on and off, archive and restore them.",
        PermissionScope::Both,
        MANAGERS,
    ),
    // H1.24: deleting the whole workspace, after a 14-day grace period
    // anyone holding it can cancel.
    permission(
        Permission::WorkspaceDelete,
        "workspace.delete",
        "settings",
        "Delete the workspace",
        "Schedule the whole workspace for deletion, or cancel that within 14 days.",
        PermissionScope::Workspace,
        OWNERS,
    ),
];

/// The modules permissions are grouped by, in order, with their names.
pub const PERMISSION_MODULES: [(&str, &str); 14] = [
    ("projects", "Projects"),
    ("tasks", "Tasks"),
    ("channels", "Channels"),
    ("messages", "Direct messages"),
    ("pages", "Pages and decisions"),
    ("whiteboards", "Whiteboards"),
    ("collections", "Collections"),
    ("dashboards", "Dashboards"),
    ("sheets", "Sheets"),
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

    /// Whether it only lets its holder see things: a "see" level, or a
    /// capability that only reads.
    pub fn only_sees(self) -> bool {
        matches!(self.info().level, Some((_, Level::See)))
            || matches!(
                self,
                Self::MembersView | Self::DirectoryView | Self::SpacesBrowse
            )
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PERMISSIONS
            .iter()
            .find(|info| info.key == key)
            .map(|info| info.permission)
    }

    const fn bit(self) -> u128 {
        1 << (self as u8)
    }
}

/// A set of permissions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PermissionSet(u128);

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

    /// The permissions that act only on the workspace as a whole (members,
    /// roles, new spaces, deleting it): never reachable from inside one
    /// space.
    pub fn workspace_only() -> Self {
        PERMISSIONS
            .iter()
            .filter(|info| matches!(info.scope, PermissionScope::Workspace))
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

    pub const fn without_all(self, other: Self) -> Self {
        Self(self.0 & !other.0)
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

    /// The set closed under level implication (K1.9): holding a level on a
    /// kind of thing holds every lower level on the same kind.
    #[must_use]
    pub fn implied(self) -> Self {
        let held = self;
        PERMISSIONS
            .iter()
            .filter_map(|info| info.level.map(|level| (info.permission, level)))
            .filter(|(permission, _)| held.contains(*permission))
            .fold(self, |set, (_, (kind, level))| {
                PERMISSIONS
                    .iter()
                    .filter(|lower| {
                        lower
                            .level
                            .is_some_and(|(other, below)| other == kind && below < level)
                    })
                    .fold(set, |set, lower| set.with(lower.permission))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, PERMISSIONS, Permission, PermissionSet};
    use crate::policy::MembershipRole;

    /// The keys and built-in defaults that existed before K1.9, which stored
    /// custom roles and every built-in role still mean the same by.
    const BEFORE_K1_9: [(&str, [bool; 4]); 35] = [
        ("projects.view", [true, true, true, true]),
        ("projects.create", [true, true, false, false]),
        ("projects.manage", [true, true, false, false]),
        ("tasks.view", [true, true, true, true]),
        ("tasks.create", [true, true, true, false]),
        ("tasks.edit", [true, true, true, false]),
        ("tasks.delete", [true, true, true, false]),
        ("tasks.workflow", [true, true, false, false]),
        ("channels.post", [true, true, true, false]),
        ("channels.create", [true, true, true, false]),
        ("channels.moderate", [true, true, false, false]),
        ("pages.edit", [true, true, true, false]),
        ("whiteboards.edit", [true, true, true, false]),
        ("collections.edit", [true, true, true, false]),
        ("collections.build", [true, true, true, false]),
        ("sheets.edit", [true, true, true, false]),
        ("runs.launch", [true, true, true, false]),
        ("runs.approve", [true, true, true, false]),
        ("runs.interrupt", [true, true, true, false]),
        ("runs.accounts", [true, true, true, false]),
        ("runs.report", [true, true, true, true]),
        ("requests.raise", [true, true, true, true]),
        ("requests.answer", [true, true, true, true]),
        ("code.view", [true, true, true, true]),
        ("code.publish", [true, true, true, false]),
        ("code.manage", [true, true, false, false]),
        ("agents.manage", [true, true, false, false]),
        ("agents.manage_any", [true, false, false, false]),
        ("hosts.manage", [true, true, false, false]),
        ("members.view", [true, true, false, false]),
        ("directory.view", [true, true, true, true]),
        ("members.invite", [true, true, false, false]),
        ("members.manage", [true, true, false, false]),
        ("roles.manage", [true, true, false, false]),
        ("settings.manage", [true, true, false, false]),
    ];

    #[test]
    fn every_existing_key_keeps_its_built_in_defaults() {
        for (key, built_in) in BEFORE_K1_9 {
            let permission = Permission::from_key(key).unwrap_or_else(|| panic!("{key} is gone"));
            assert_eq!(permission.info().built_in, built_in, "{key}");
        }
    }

    #[test]
    fn every_level_implies_the_lower_ones() {
        for info in PERMISSIONS {
            let Some((kind, level)) = info.level else {
                continue;
            };
            let implied = PermissionSet::EMPTY.with(info.permission).implied();
            for lower in PERMISSIONS {
                let Some((other, below)) = lower.level else {
                    continue;
                };
                assert_eq!(
                    implied.contains(lower.permission),
                    other == kind && below <= level,
                    "{} and {}",
                    info.key,
                    lower.key
                );
            }
        }
        // A capability implies nothing.
        assert_eq!(
            PermissionSet::EMPTY
                .with(Permission::ProjectsCreate)
                .implied(),
            PermissionSet::EMPTY.with(Permission::ProjectsCreate)
        );
        // Built-in roles are already closed, so implication changes nothing
        // for them.
        for role in MembershipRole::ALL {
            let held = PermissionSet::built_in(role);
            assert_eq!(held.implied(), held, "{}", role.key());
        }
    }

    #[test]
    fn each_kind_has_one_permission_per_level_and_keys_say_their_level() {
        let mut seen = std::collections::BTreeSet::new();
        for info in PERMISSIONS {
            if let Some((kind, level)) = info.level {
                assert!(seen.insert((kind, level)), "{kind} {level:?} twice");
                // Keys from before K1.9 keep their names (`channels.post`).
                let new_key = !BEFORE_K1_9.iter().any(|(key, _)| *key == info.key);
                if new_key && matches!(level, Level::Comment | Level::Suggest | Level::EditOwn) {
                    assert!(
                        info.key.ends_with(level.key()),
                        "{} should end with {}",
                        info.key,
                        level.key()
                    );
                }
            }
        }
    }
}
