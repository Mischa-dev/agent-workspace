//! Compile-time inventory of every externally reachable state-changing route.
//!
//! The policy engine remains the enforcement boundary. This inventory makes
//! the route-to-action classification reviewable and has a source-backed test
//! so adding a new POST, PUT, PATCH, or DELETE route without a classification
//! fails CI.

use crate::policy::Action;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MutationMethod {
    Post,
    Put,
    Patch,
    Delete,
    /// A transport whose allowed methods are owned by a pinned protocol SDK.
    Protocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationAction {
    Policy(Action),
    SessionAuthentication,
    InvitationCapability,
    OAuthProtocol,
    McpProtocol,
    GithubWebhook,
    /// Browser error reports: counted and logged, never stored.
    ClientTelemetry,
    /// A button on a person's own push notification, carrying a short-lived
    /// token from that push. The command it runs is authorized as that
    /// person, exactly as in the app.
    NotificationActionToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MutatingEntryPoint {
    pub module: &'static str,
    pub method: MutationMethod,
    pub path: &'static str,
    pub action: MutationAction,
}

macro_rules! policy_route {
    ($module:literal, $method:ident, $path:literal, $action:ident) => {
        MutatingEntryPoint {
            module: $module,
            method: MutationMethod::$method,
            path: $path,
            action: MutationAction::Policy(Action::$action),
        }
    };
}

macro_rules! boundary_route {
    ($module:literal, $method:ident, $path:literal, $action:ident) => {
        MutatingEntryPoint {
            module: $module,
            method: MutationMethod::$method,
            path: $path,
            action: MutationAction::$action,
        }
    };
}

pub const MUTATING_ENTRY_POINTS: &[MutatingEntryPoint] = &[
    policy_route!(
        "settings",
        Put,
        "/api/v1/settings/values/{key}",
        SettingsManage
    ),
    policy_route!(
        "settings",
        Post,
        "/api/v1/settings/values/{key}/reset",
        SettingsManage
    ),
    policy_route!(
        "environments",
        Post,
        "/api/v1/projects/{project_id}/environments",
        SettingsManage
    ),
    policy_route!(
        "environments",
        Put,
        "/api/v1/projects/{project_id}/environments/{environment_id}",
        SettingsManage
    ),
    policy_route!(
        "environments",
        Post,
        "/api/v1/projects/{project_id}/environments/{environment_id}/archive",
        SettingsManage
    ),
    policy_route!(
        "environments",
        Post,
        "/api/v1/projects/{project_id}/environments/{environment_id}/default",
        SettingsManage
    ),
    policy_route!(
        "environments",
        Post,
        "/api/v1/projects/{project_id}/environments/{environment_id}/check",
        WorkerInvoke
    ),
    policy_route!(
        "harnesses",
        Post,
        "/api/v1/organizations/{organization_id}/harnesses",
        SettingsManage
    ),
    policy_route!(
        "harnesses",
        Patch,
        "/api/v1/organizations/{organization_id}/harnesses/{harness_id}",
        SettingsManage
    ),
    policy_route!(
        "harnesses",
        Delete,
        "/api/v1/organizations/{organization_id}/harnesses/{harness_id}",
        SettingsManage
    ),
    policy_route!(
        "agent_profiles",
        Post,
        "/api/v1/projects/{project_id}/agent-profiles",
        SettingsManage
    ),
    policy_route!(
        "agent_profiles",
        Patch,
        "/api/v1/projects/{project_id}/agent-profiles/{profile_id}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Put,
        "/api/v1/organizations/{organization_id}/secrets/{name}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Delete,
        "/api/v1/organizations/{organization_id}/secrets/{name}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Put,
        "/api/v1/organizations/{organization_id}/personal-secrets/{name}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Delete,
        "/api/v1/organizations/{organization_id}/personal-secrets/{name}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Put,
        "/api/v1/projects/{project_id}/secrets/{name}",
        SettingsManage
    ),
    policy_route!(
        "secrets",
        Delete,
        "/api/v1/projects/{project_id}/secrets/{name}",
        SettingsManage
    ),
    boundary_route!("metrics", Post, "/api/v1/client-errors", ClientTelemetry),
    boundary_route!(
        "auth/workos",
        Post,
        "/api/v1/auth/workos/start",
        SessionAuthentication
    ),
    boundary_route!(
        "auth/workos",
        Patch,
        "/api/v1/account/profile",
        SessionAuthentication
    ),
    policy_route!(
        "memberships",
        Post,
        "/api/v1/projects/{project_id}/members",
        MembershipManage
    ),
    policy_route!(
        "workspace",
        Patch,
        "/api/v1/organizations/{organization_id}",
        SettingsManage
    ),
    policy_route!(
        "workspace",
        Patch,
        "/api/v1/organizations/{organization_id}/members/{actor_id}",
        MembershipManage
    ),
    policy_route!(
        "workspace",
        Post,
        "/api/v1/organizations/{organization_id}/transfer-ownership",
        MembershipManage
    ),
    boundary_route!(
        "workspace",
        Post,
        "/api/v1/organizations/{organization_id}/leave",
        SessionAuthentication
    ),
    policy_route!(
        "workspace",
        Post,
        "/api/v1/organizations/{organization_id}/invitations",
        InvitationManage
    ),
    policy_route!(
        "workspace",
        Post,
        "/api/v1/workspace-invitations/{invitation_id}/revoke",
        InvitationManage
    ),
    boundary_route!(
        "workspace",
        Post,
        "/api/v1/workspace-invitations/resolve",
        InvitationCapability
    ),
    boundary_route!(
        "workspace",
        Post,
        "/api/v1/workspace-invitations/accept-new",
        InvitationCapability
    ),
    boundary_route!(
        "workspace",
        Post,
        "/api/v1/workspace-invitations/accept-existing",
        InvitationCapability
    ),
    boundary_route!(
        "workspace",
        Put,
        "/api/v1/account/default-workspace",
        SessionAuthentication
    ),
    boundary_route!("auth", Post, "/api/v1/auth/login", SessionAuthentication),
    boundary_route!("auth", Post, "/api/v1/auth/logout", SessionAuthentication),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/auth/login/two-factor",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/password",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/sessions/{session_id}/revoke",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/sessions/revoke-others",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/two-factor/setup",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/two-factor/confirm",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/two-factor/recovery-codes",
        SessionAuthentication
    ),
    boundary_route!(
        "account_security",
        Post,
        "/api/v1/account/two-factor/disable",
        SessionAuthentication
    ),
    policy_route!(
        "account_security",
        Post,
        "/api/v1/account/api-tokens",
        CredentialManage
    ),
    policy_route!(
        "account_security",
        Post,
        "/api/v1/account/api-tokens/{credential_id}/revoke",
        CredentialManage
    ),
    policy_route!("webhooks", Post, "/api/v1/webhooks", CredentialManage),
    policy_route!(
        "webhooks",
        Patch,
        "/api/v1/webhooks/{webhook_id}",
        CredentialManage
    ),
    policy_route!(
        "webhooks",
        Delete,
        "/api/v1/webhooks/{webhook_id}",
        CredentialManage
    ),
    policy_route!(
        "webhooks",
        Post,
        "/api/v1/webhooks/{webhook_id}/rotate-secret",
        CredentialManage
    ),
    policy_route!(
        "webhooks",
        Post,
        "/api/v1/webhooks/{webhook_id}/test",
        CredentialManage
    ),
    policy_route!(
        "webhooks",
        Post,
        "/api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver",
        CredentialManage
    ),
    boundary_route!(
        "account_security",
        Put,
        "/api/v1/account/appearance",
        SessionAuthentication
    ),
    policy_route!("api", Post, "/api/v1/threads/{id}/agent-runs", WorkerInvoke),
    policy_route!(
        "api",
        Post,
        "/api/v1/agent-runs/{id}/interrupt",
        RunInterrupt
    ),
    policy_route!("api", Post, "/api/v1/agent-runs/{id}/retry", WorkerInvoke),
    policy_route!(
        "api",
        Post,
        "/api/v1/agent-runs/{id}/acknowledge-failure",
        ReadStateUpdate
    ),
    policy_route!(
        "api",
        Post,
        "/api/v1/agent-approvals/{id}/respond",
        ApprovalRespond
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
        CredentialManage
    ),
    policy_route!(
        "execution",
        Post,
        "/api/v1/projects/{id}/execution-profiles",
        ExecutionProfileManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts",
        ExecutionProfileManage
    ),
    policy_route!(
        "account",
        Patch,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
        ExecutionProfileManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/use",
        ExecutionProfileManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/browser",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/device-code",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/api-key",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/cancel",
        CredentialManage
    ),
    policy_route!(
        "account",
        Post,
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}/logout",
        CredentialManage
    ),
    policy_route!(
        "execution",
        Post,
        "/api/v1/projects/{id}/harnesses/{kind}/catalog/refresh",
        ExecutionProfileManage
    ),
    policy_route!(
        "execution",
        Put,
        "/api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
        ProjectManage
    ),
    policy_route!(
        "execution",
        Delete,
        "/api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
        ProjectManage
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/projects/{id}/channels",
        ChannelCreate
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/channels/{id}/messages",
        MessageSend
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/threads/{id}/messages",
        MessageSend
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/channels/{id}/threads",
        ThreadCreate
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/channels/{id}/archive",
        ConversationArchive
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/threads/{id}/archive",
        ConversationArchive
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/channels/{id}/restore",
        ConversationArchive
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/threads/{id}/restore",
        ConversationArchive
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/conversations/{id}/mark-seen",
        ReadStateUpdate
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/conversations/{id}/mark-unread",
        ReadStateUpdate
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/conversations/{id}/messages",
        MessageSend
    ),
    policy_route!(
        "conversations",
        Post,
        "/api/v1/conversations/{id}/threads",
        ThreadCreate
    ),
    policy_route!("conversations", Patch, "/api/v1/threads/{id}", ThreadCreate),
    policy_route!("memory", Post, "/api/v1/projects/{id}/pages", WorkItemEdit),
    policy_route!("memory", Patch, "/api/v1/pages/{id}", WorkItemEdit),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Patch,
        "/api/v1/whiteboards/{id}",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/duplicate",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/scene",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/operations",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/files",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/revisions/{revision}/restore",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Post,
        "/api/v1/whiteboards/{id}/links",
        WorkItemEdit
    ),
    policy_route!(
        "whiteboards/http",
        Delete,
        "/api/v1/whiteboards/{id}/links/{kind}/{target_id}",
        WorkItemEdit
    ),
    policy_route!(
        "memory",
        Post,
        "/api/v1/projects/{id}/decisions",
        MessageSend
    ),
    policy_route!(
        "memory",
        Post,
        "/api/v1/conversations/{id}/pins",
        MessageSend
    ),
    policy_route!(
        "memory",
        Delete,
        "/api/v1/conversations/{id}/pins/{kind}/{target_id}",
        MessageSend
    ),
    boundary_route!(
        "connected_apps",
        Patch,
        "/api/v1/account/connected-apps/{id}",
        SessionAuthentication
    ),
    boundary_route!(
        "connected_apps",
        Post,
        "/api/v1/account/connected-apps/{id}/revoke",
        SessionAuthentication
    ),
    policy_route!("direct", Post, "/api/v1/direct/dms", ChannelCreate),
    policy_route!("direct", Post, "/api/v1/direct/groups", ChannelCreate),
    policy_route!(
        "direct",
        Post,
        "/api/v1/conversations/{id}/members",
        ChannelCreate
    ),
    policy_route!(
        "direct",
        Post,
        "/api/v1/conversations/{id}/members/{actor_id}/remove",
        ChannelCreate
    ),
    policy_route!(
        "replies",
        Post,
        "/api/v1/conversations/{id}/continue",
        MessageSend
    ),
    policy_route!(
        "replies",
        Post,
        "/api/v1/reply-requests/{id}/retry",
        MessageSend
    ),
    policy_route!(
        "replies",
        Post,
        "/api/v1/reply-requests/{id}/stop",
        MessageSend
    ),
    // Editing and removing your own message needs MessageSend. Removing
    // someone else's also needs MessageModerate, checked in the handler.
    policy_route!(
        "message_edits",
        Patch,
        "/api/v1/conversations/{id}/messages/{message_id}",
        MessageSend
    ),
    policy_route!(
        "message_edits",
        Delete,
        "/api/v1/conversations/{id}/messages/{message_id}",
        MessageSend
    ),
    // Anyone who can post in a conversation can react there.
    policy_route!(
        "reactions",
        Put,
        "/api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
        MessageSend
    ),
    policy_route!(
        "reactions",
        Delete,
        "/api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
        MessageSend
    ),
    // A person changes only their own notifications; every row is also
    // checked against what they can still see.
    boundary_route!(
        "notifications",
        Post,
        "/api/v1/inbox/update",
        SessionAuthentication
    ),
    // Notification settings, devices and usage are the caller's own: their
    // devices, their provider accounts (owner only), their preferences.
    boundary_route!(
        "notification_settings",
        Put,
        "/api/v1/notification-settings/channels",
        SessionAuthentication
    ),
    boundary_route!(
        "notification_settings",
        Put,
        "/api/v1/notification-settings/mutes",
        SessionAuthentication
    ),
    boundary_route!(
        "push",
        Post,
        "/api/v1/push/subscriptions",
        SessionAuthentication
    ),
    boundary_route!(
        "push",
        Delete,
        "/api/v1/push/subscriptions/{id}",
        SessionAuthentication
    ),
    boundary_route!(
        "push",
        Post,
        "/api/v1/push/subscriptions/{id}/test",
        SessionAuthentication
    ),
    boundary_route!(
        "push",
        Post,
        "/api/v1/notification-actions",
        NotificationActionToken
    ),
    boundary_route!(
        "usage",
        Post,
        "/api/v1/usage/accounts/{account_id}/refresh",
        SessionAuthentication
    ),
    boundary_route!(
        "usage",
        Post,
        "/api/v1/usage/accounts/{account_id}/schedules",
        SessionAuthentication
    ),
    boundary_route!(
        "usage",
        Delete,
        "/api/v1/usage/accounts/{account_id}/schedules/{schedule_id}",
        SessionAuthentication
    ),
    boundary_route!(
        "usage",
        Put,
        "/api/v1/usage/accounts/{account_id}/rules/{rule}",
        SessionAuthentication
    ),
    boundary_route!(
        "usage",
        Post,
        "/api/v1/usage/accounts/{account_id}/reset-credits/spend",
        SessionAuthentication
    ),
    // A batch is a set of launches, each authorized in its own project.
    policy_route!(
        "usage",
        Post,
        "/api/v1/usage/accounts/{account_id}/batch",
        WorkerInvoke
    ),
    // The first-run path's progress is the caller's own.
    boundary_route!(
        "onboarding",
        Put,
        "/api/v1/organizations/{organization_id}/onboarding",
        SessionAuthentication
    ),
    policy_route!(
        "repositories",
        Post,
        "/api/v1/projects/{project_id}/repositories",
        RepositoryManage
    ),
    policy_route!(
        "repositories",
        Post,
        "/api/v1/projects/{project_id}/repositories/{repository_id}/prepare",
        RepositoryManage
    ),
    policy_route!(
        "repositories",
        Post,
        "/api/v1/projects/{project_id}/repositories/{repository_id}/archive",
        RepositoryManage
    ),
    policy_route!(
        "github",
        Post,
        "/api/v1/projects/{project_id}/integrations/github/sync",
        GithubIntegrationManage
    ),
    policy_route!(
        "github/connect",
        Post,
        "/api/v1/projects/{project_id}/integrations/github/connect",
        GithubIntegrationManage
    ),
    policy_route!(
        "github/connect",
        Post,
        "/api/v1/projects/{project_id}/integrations/github/repositories",
        RepositoryManage
    ),
    policy_route!(
        "github",
        Put,
        "/api/v1/projects/{project_id}/integrations/github/automation",
        GithubIntegrationManage
    ),
    policy_route!(
        "github",
        Post,
        "/api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
        RepositoryPublish
    ),
    policy_route!(
        "github",
        Post,
        "/api/v1/projects/{project_id}/agent-runs/{run_id}/pull-request",
        PullRequestCreate
    ),
    policy_route!(
        "github",
        Post,
        "/api/v1/projects/{project_id}/pull-requests/{pull_request_id}/refresh",
        PullRequestRefresh
    ),
    boundary_route!(
        "github",
        Post,
        "/api/v1/integrations/github/webhook",
        GithubWebhook
    ),
    policy_route!(
        "invitations",
        Post,
        "/api/v1/projects/{project_id}/invitations",
        InvitationManage
    ),
    policy_route!(
        "invitations",
        Post,
        "/api/v1/project-invitations/{invitation_id}/revoke",
        InvitationManage
    ),
    policy_route!(
        "invitations",
        Post,
        "/api/v1/project-invitations/{invitation_id}/replace",
        InvitationManage
    ),
    boundary_route!(
        "invitations",
        Post,
        "/api/v1/project-invitations/resolve",
        InvitationCapability
    ),
    boundary_route!(
        "invitations",
        Post,
        "/api/v1/project-invitations/accept-new",
        InvitationCapability
    ),
    boundary_route!(
        "invitations",
        Post,
        "/api/v1/project-invitations/accept-existing",
        InvitationCapability
    ),
    policy_route!(
        "memberships",
        Patch,
        "/api/v1/projects/{project_id}/members/{actor_id}",
        MembershipManage
    ),
    policy_route!(
        "external_agents",
        Post,
        "/api/v1/projects/{project_id}/external-agents",
        ExternalAgentManage
    ),
    policy_route!(
        "external_agents",
        Patch,
        "/api/v1/projects/{project_id}/external-agents/{connection_id}",
        ExternalAgentManage
    ),
    policy_route!(
        "external_agents",
        Post,
        "/api/v1/projects/{project_id}/external-agents/{connection_id}/credentials/rotate",
        ExternalAgentManage
    ),
    policy_route!(
        "external_agents",
        Post,
        "/api/v1/projects/{project_id}/external-agents/{connection_id}/remove",
        ExternalAgentManage
    ),
    policy_route!(
        "external_agents",
        Post,
        "/api/v1/projects/{project_id}/external-agents/{connection_id}/webhook",
        ExternalAgentManage
    ),
    boundary_route!("mcp/oauth", Post, "/oauth/register", OAuthProtocol),
    boundary_route!("mcp/oauth", Post, "/oauth/authorize", OAuthProtocol),
    boundary_route!("mcp/oauth", Post, "/connect/confirm", OAuthProtocol),
    boundary_route!("mcp/oauth", Post, "/oauth/token", OAuthProtocol),
    boundary_route!("mcp/oauth", Post, "/oauth/revoke", OAuthProtocol),
    boundary_route!("mcp", Protocol, "/api/v1/mcp", McpProtocol),
    boundary_route!("public_api", Post, "/api/public/v1/{tool}", McpProtocol),
    policy_route!(
        "projects",
        Post,
        "/api/v1/projects",
        OrganizationProjectCreate
    ),
    policy_route!(
        "projects",
        Post,
        "/api/v1/projects/{id}/archive",
        ProjectManage
    ),
    policy_route!(
        "projects",
        Put,
        "/api/v1/projects/{id}/navigation",
        NavigationWrite
    ),
    policy_route!(
        "projects",
        Post,
        "/api/v1/projects/{id}/switch",
        NavigationWrite
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/move",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
        WorkerInvoke
    ),
    policy_route!(
        "launch",
        Post,
        "/api/v1/projects/{project_id}/launches",
        WorkerInvoke
    ),
    policy_route!(
        "launch",
        Post,
        "/api/v1/projects/{project_id}/launches/{launch_id}/cancel",
        WorkerInvoke
    ),
    policy_route!(
        "automations",
        Post,
        "/api/v1/projects/{project_id}/automations",
        WorkItemEdit
    ),
    policy_route!(
        "automations",
        Patch,
        "/api/v1/projects/{project_id}/automations/{rule_id}",
        WorkItemEdit
    ),
    policy_route!(
        "automations",
        Delete,
        "/api/v1/projects/{project_id}/automations/{rule_id}",
        WorkItemEdit
    ),
    policy_route!(
        "automations",
        Post,
        "/api/v1/projects/{project_id}/automations/{rule_id}/run",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Delete,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
        MessageSend
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Delete,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Delete,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/bulk",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/dependencies",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/relations",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/watch",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/release-claim",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/labels",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/labels/{label_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/milestones",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/milestones/{milestone_id}",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/organizations/{organization_id}/ai/test",
        SettingsManage
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/test",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/analyze",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/accept",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/dismiss",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/plans",
        WorkItemCreate
    ),
    policy_route!(
        "ai/http",
        Put,
        "/api/v1/projects/{project_id}/ai/plans/{import_id}",
        WorkItemCreate
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/ai/plans/{import_id}/commit",
        WorkItemCreate
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/triage",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/criteria",
        WorkItemEdit
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/summary",
        WorkItemView
    ),
    policy_route!(
        "ai/http",
        Post,
        "/api/v1/conversations/{id}/ai/summary",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/views",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/views/{view_id}",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Delete,
        "/api/v1/projects/{project_id}/views/{view_id}",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/views/{view_id}/pin",
        WorkItemView
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/task-templates",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/task-templates/{template_id}",
        WorkItemEdit
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/task-templates/{template_id}/apply",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/imports",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/imports/{import_id}",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/imports/{import_id}/commit",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/save-as-template",
        SettingsManage
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/organizations/{organization_id}/project-templates/{template_id}",
        SettingsManage
    ),
    policy_route!(
        "work_items",
        Post,
        "/api/v1/projects/{project_id}/recurring-tasks",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Patch,
        "/api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Delete,
        "/api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
        WorkItemCreate
    ),
    policy_route!(
        "work_items",
        Put,
        "/api/v1/projects/{project_id}/work-item-workflow",
        ProjectManage
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{MUTATING_ENTRY_POINTS, MutationMethod};

    const ROUTER_SOURCES: &[(&str, &str)] = &[
        ("account", include_str!("account.rs")),
        ("account_security", include_str!("account_security.rs")),
        ("api", include_str!("api.rs")),
        ("auth", include_str!("auth/mod.rs")),
        ("auth/workos", include_str!("auth/workos.rs")),
        ("conversations", include_str!("conversations.rs")),
        ("direct", include_str!("direct.rs")),
        ("replies", include_str!("replies.rs")),
        ("message_edits", include_str!("message_edits.rs")),
        ("reactions", include_str!("reactions.rs")),
        ("notifications", include_str!("notifications.rs")),
        (
            "notification_settings",
            include_str!("notification_settings.rs"),
        ),
        ("push", include_str!("push.rs")),
        ("usage", include_str!("usage.rs")),
        ("github", include_str!("github.rs")),
        ("github/connect", include_str!("github/connect.rs")),
        ("invitations", include_str!("invitations.rs")),
        ("memberships", include_str!("memberships.rs")),
        ("workspace", include_str!("workspace.rs")),
        ("onboarding", include_str!("onboarding.rs")),
        ("audit_log", include_str!("audit_log.rs")),
        ("mcp/oauth", include_str!("mcp/oauth.rs")),
        ("public_api", include_str!("public_api.rs")),
        ("projects", include_str!("projects.rs")),
        ("repositories", include_str!("repositories.rs")),
        ("work_items", include_str!("work_items.rs")),
        ("ai/http", include_str!("ai/http.rs")),
        ("execution", include_str!("execution.rs")),
        ("external_agents", include_str!("external_agents.rs")),
        ("memory", include_str!("memory.rs")),
        ("whiteboards/http", include_str!("whiteboards/http.rs")),
        ("connected_apps", include_str!("connected_apps.rs")),
        ("webhooks", include_str!("webhooks.rs")),
        ("settings", include_str!("settings.rs")),
        ("environments", include_str!("environments.rs")),
        ("harnesses", include_str!("harnesses.rs")),
        ("agent_profiles", include_str!("agent_profiles.rs")),
        ("run_views", include_str!("run_views.rs")),
        ("launch", include_str!("launch.rs")),
        ("automations", include_str!("automations.rs")),
        ("agent_sessions", include_str!("agent_sessions.rs")),
        ("secrets", include_str!("secrets.rs")),
        ("metrics", include_str!("metrics.rs")),
    ];

    #[test]
    fn every_http_mutation_route_has_exactly_one_typed_action() {
        let actual = ROUTER_SOURCES
            .iter()
            .flat_map(|(module, source)| mutation_routes(module, source))
            .collect::<BTreeSet<_>>();
        let expected = MUTATING_ENTRY_POINTS
            .iter()
            .filter(|entry| entry.method != MutationMethod::Protocol)
            .map(|entry| (entry.module, entry.method, entry.path))
            .collect::<BTreeSet<_>>();

        assert_eq!(actual, expected);
        assert_eq!(expected.len(), MUTATING_ENTRY_POINTS.len() - 1);
    }

    fn mutation_routes<'a>(
        module: &'static str,
        source: &'a str,
    ) -> Vec<(&'static str, MutationMethod, &'a str)> {
        route_blocks(source)
            .into_iter()
            .flat_map(|block| {
                let path = first_string_literal(block).expect("route path literal");
                [
                    (MutationMethod::Post, "post("),
                    (MutationMethod::Put, "put("),
                    (MutationMethod::Patch, "patch("),
                    (MutationMethod::Delete, "delete("),
                ]
                .into_iter()
                .filter(move |(_, token)| block.contains(token))
                .map(move |(method, _)| (module, method, path))
            })
            .collect()
    }

    fn route_blocks(source: &str) -> Vec<&str> {
        let mut blocks = Vec::new();
        let mut remaining = source;
        while let Some(relative_start) = remaining.find(".route(") {
            let start = source.len() - remaining.len() + relative_start;
            let open = start + ".route".len();
            let mut depth = 0_u32;
            let mut in_string = false;
            let mut escaped = false;
            let mut end = None;
            for (offset, character) in source[open..].char_indices() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == '"' {
                        in_string = false;
                    }
                    continue;
                }
                match character {
                    '"' => in_string = true,
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + offset + character.len_utf8());
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.expect("balanced route call");
            blocks.push(&source[start..end]);
            remaining = &source[end..];
        }
        blocks
    }

    fn first_string_literal(block: &str) -> Option<&str> {
        let start = block.find('"')? + 1;
        let end = block[start..].find('"')? + start;
        Some(&block[start..end])
    }
}
