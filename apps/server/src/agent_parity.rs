//! AWD-91: agent parity. Mischa's bar is that an agent can do everything a
//! person can in a project, through MCP. This guard maps every web write
//! (each state-changing route in `policy_inventory`) to the MCP tool that
//! does the same thing, or says why it has none:
//!
//! - `Tool`: the tool or tools that do it.
//! - `Person`: deliberately a person's own act, such as their password, their
//!   devices, a secret, or a provider sign-in.
//! - `Plumbing`: not an action in a project at all, such as sign-in, OAuth,
//!   an invitation link or a webhook from GitHub.
//! - `Op`: the generated operation route, where every operation is also the
//!   MCP tool of the same name.
//!
//! AWD-111 closed the last gaps: there's no "not yet" any more.
//!
//! A new web write fails the test until it's listed here, and the way to
//! list it is with its tool. The scenario that walks an agent through a
//! whole project is `tests/integration/awd91_agent_parity.rs`.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::policy_inventory::{MUTATING_ENTRY_POINTS, MutationMethod};

    use MutationMethod::{Delete, Patch, Post, Put};
    use Parity::{Op, Person, Plumbing, Tool};

    enum Parity {
        /// The MCP tools that do it, separated by ", ".
        Tool(&'static str),
        /// Why only a person does it.
        Person(&'static str),
        /// Why it isn't an action in a project.
        Plumbing(&'static str),
        /// The generated operation route (K2): each operation it runs is
        /// also the MCP tool of the same name, by construction.
        Op,
    }

    /// Every web write, in `MUTATING_ENTRY_POINTS` order.
    const WEB_WRITES: &[(MutationMethod, &str, Parity)] = &[
        (Put, "/api/v1/settings/values/{key}", Tool("settings_set")),
        (
            Post,
            "/api/v1/settings/values/{key}/reset",
            Tool("settings_set"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/layout",
            Tool("project_layout_set"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/layout/rules",
            Tool("project_layout_set"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/layout",
            Tool("project_layout_set"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/layout/mine",
            Person("each person's own sidebar"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/modules",
            Tool("workspace_update"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/layout/presets",
            Tool("layout_preset_write"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
            Tool("layout_preset_write"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
            Tool("layout_preset_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/environments",
            Tool("environment_write"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/environments/{environment_id}",
            Tool("environment_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/environments/{environment_id}/archive",
            Tool("environment_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/environments/{environment_id}/default",
            Tool("environment_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/environments/{environment_id}/check",
            Tool("environment_write"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/harnesses",
            Person("registers a program the host runs"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}/harnesses/{harness_id}",
            Person("registers a program the host runs"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/harnesses/{harness_id}",
            Person("registers a program the host runs"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/agent-profiles",
            Tool("agent_profile_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/agent-profiles/{profile_id}",
            Tool("agent_profile_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/agent-profiles/{profile_id}",
            Tool("agent_profile_write"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/mail",
            Person("secrets never pass through an agent"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/mail",
            Person("secrets never pass through an agent"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/mail/test",
            Person("secrets never pass through an agent"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/personal-secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/personal-secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/secrets/{name}",
            Person("secrets never pass through an agent"),
        ),
        (
            Post,
            "/api/v1/client-errors",
            Plumbing("browser error reports"),
        ),
        (Post, "/api/v1/auth/workos/start", Plumbing("sign-in")),
        (
            Patch,
            "/api/v1/account/profile",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/members",
            Tool("project_member_set"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}",
            Tool("workspace_update"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}/members/{actor_id}",
            Tool("role_assign"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/transfer-ownership",
            Person("ownership passes between people"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/roles",
            Tool("role_write"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}/roles/{role_id}",
            Tool("role_write"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/roles/{role_id}",
            Tool("role_write"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/members/{actor_id}/role",
            Tool("role_assign"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/members/{actor_id}/agent-limit",
            Tool("role_assign"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/workspace-templates",
            Tool("workspace_template_save"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/workspace-templates/apply",
            Tool("workspace_template_apply"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/workspace-templates/{template_id}",
            Tool("workspace_template_save"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/leave",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/invitations",
            Tool("invitation_write"),
        ),
        (
            Post,
            "/api/v1/workspace-invitations/{invitation_id}/revoke",
            Tool("invitation_write"),
        ),
        (
            Post,
            "/api/v1/workspace-invitations/resolve",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Post,
            "/api/v1/workspace-invitations/accept-new",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Post,
            "/api/v1/workspace-invitations/accept-existing",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Put,
            "/api/v1/account/default-workspace",
            Person("a person's own account"),
        ),
        (Post, "/api/v1/auth/login", Plumbing("sign-in")),
        (Post, "/api/v1/auth/logout", Plumbing("sign-in")),
        (Post, "/api/v1/auth/login/two-factor", Plumbing("sign-in")),
        (
            Post,
            "/api/v1/account/password",
            Person("a person's own account"),
        ),
        (
            Put,
            "/api/v1/account/email",
            Person("a person's own account"),
        ),
        (
            Delete,
            "/api/v1/account/email",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/email/resend",
            Person("a person's own account"),
        ),
        (Post, "/api/v1/auth/verify-email", Plumbing("sign-in")),
        (Post, "/api/v1/auth/password-reset", Plumbing("sign-in")),
        (
            Post,
            "/api/v1/auth/password-reset/inspect",
            Plumbing("sign-in"),
        ),
        (
            Post,
            "/api/v1/auth/password-reset/complete",
            Plumbing("sign-in"),
        ),
        (
            Post,
            "/api/v1/account/sessions/{session_id}/revoke",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/sessions/revoke-others",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/two-factor/setup",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/two-factor/confirm",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/two-factor/recovery-codes",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/two-factor/disable",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/account/api-tokens",
            Person("credentials are made and revoked by people"),
        ),
        (
            Post,
            "/api/v1/account/api-tokens/{credential_id}/revoke",
            Person("credentials are made and revoked by people"),
        ),
        (Post, "/api/v1/webhooks", Tool("webhook_write")),
        (
            Patch,
            "/api/v1/webhooks/{webhook_id}",
            Tool("webhook_write"),
        ),
        (
            Delete,
            "/api/v1/webhooks/{webhook_id}",
            Tool("webhook_write"),
        ),
        (
            Post,
            "/api/v1/webhooks/{webhook_id}/rotate-secret",
            Tool("webhook_write"),
        ),
        (
            Post,
            "/api/v1/webhooks/{webhook_id}/test",
            Tool("webhook_write"),
        ),
        (
            Post,
            "/api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver",
            Tool("webhook_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/bug-report-links",
            Tool("bug_report_link_write"),
        ),
        (
            Patch,
            "/api/v1/bug-report-links/{link_id}",
            Tool("bug_report_link_write"),
        ),
        (
            Delete,
            "/api/v1/bug-report-links/{link_id}",
            Tool("bug_report_link_write"),
        ),
        (
            Post,
            "/api/v1/bug-report-links/{link_id}/replace",
            Tool("bug_report_link_write"),
        ),
        (
            Post,
            "/api/v1/bug-reports/{token}",
            Plumbing("a public bug report form"),
        ),
        (
            Put,
            "/api/v1/account/appearance",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/threads/{id}/agent-runs",
            Tool("run_start, run_request"),
        ),
        (Post, "/api/v1/agent-runs/{id}/interrupt", Tool("run_stop")),
        (
            Post,
            "/api/v1/agent-runs/{id}/retry",
            Tool("run_retry, run_request"),
        ),
        (
            Post,
            "/api/v1/agent-runs/{id}/acknowledge-failure",
            Tool("run_dismiss_failure"),
        ),
        (
            Post,
            "/api/v1/agent-approvals/{id}/respond",
            Tool("approval_respond"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/execution-profiles",
            // Puts the caller's own harness account to use here, the first
            // step of signing it in: a person's, like the sign-in itself.
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts",
            Person("a person's own provider account"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
            Person("a person's own provider account"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
            Person("a person's own provider account"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/use",
            Person("a person's own provider account"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/browser",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/device-code",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/api-key",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/cancel",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/logout",
            Person("signing in to a provider is a person's"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/harnesses/{kind}/catalog/refresh",
            Tool("model_catalog_write"),
        ),
        (
            Put,
            "/api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
            Tool("model_catalog_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
            Tool("model_catalog_write"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/model-catalog/{kind}/refresh",
            Tool("model_catalog_write"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
            Tool("model_catalog_write"),
        ),
        (
            Delete,
            "/api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
            Tool("model_catalog_write"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/channels",
            Tool("channel_create"),
        ),
        (Post, "/api/v1/channels/{id}/messages", Tool("message_send")),
        (Post, "/api/v1/threads/{id}/messages", Tool("message_send")),
        (Post, "/api/v1/channels/{id}/threads", Tool("thread_start")),
        (
            Post,
            "/api/v1/channels/{id}/archive",
            Tool("conversation_update"),
        ),
        (
            Post,
            "/api/v1/threads/{id}/archive",
            Tool("conversation_update"),
        ),
        (
            Post,
            "/api/v1/channels/{id}/restore",
            Tool("conversation_update"),
        ),
        (
            Post,
            "/api/v1/threads/{id}/restore",
            Tool("conversation_update"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/mark-seen",
            Tool("mark_read"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/mark-unread",
            Tool("mark_read"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/messages",
            Tool("message_send"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/threads",
            Tool("thread_start"),
        ),
        (Patch, "/api/v1/threads/{id}", Tool("thread_update")),
        (Post, "/api/v1/projects/{id}/pages", Tool("page_write")),
        (
            Post,
            "/api/v1/projects/{id}/pages/import",
            Person("a file from the person's own computer; agents write the page with page_write"),
        ),
        (Patch, "/api/v1/pages/{id}", Tool("page_write")),
        (
            Post,
            "/api/v1/pages/{id}/document/sync",
            Plumbing("the live editor's sync handshake"),
        ),
        (
            Post,
            "/api/v1/pages/{id}/document/updates",
            Tool("page_edit"),
        ),
        (Post, "/api/v1/pages/{id}/comments", Tool("page_comment")),
        (
            Patch,
            "/api/v1/pages/{id}/comments/{comment_id}",
            Tool("page_comment_update"),
        ),
        (
            Delete,
            "/api/v1/pages/{id}/comments/{comment_id}",
            Tool("page_comment_update"),
        ),
        (
            Post,
            "/api/v1/pages/{id}/suggestions/{suggestion_id}/decision",
            Tool("page_suggestion_decide"),
        ),
        (Post, "/api/v1/whiteboards", Tool("whiteboard_create")),
        (Patch, "/api/v1/whiteboards/{id}", Tool("whiteboard_update")),
        (
            Delete,
            "/api/v1/whiteboards/{id}",
            Tool("whiteboard_update"),
        ),
        (
            Post,
            "/api/v1/whiteboard-trash/empty",
            Tool("whiteboard_trash_empty"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/duplicate",
            Tool("whiteboard_duplicate"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/scene",
            Tool("whiteboard_edit"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/operations",
            Tool("whiteboard_edit"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/files",
            Tool("whiteboard_edit"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/revisions/{revision}/restore",
            Tool("whiteboard_update"),
        ),
        (
            Post,
            "/api/v1/whiteboards/{id}/links",
            Tool("whiteboard_link"),
        ),
        (
            Delete,
            "/api/v1/whiteboards/{id}/links/{kind}/{target_id}",
            Tool("whiteboard_link"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/decisions",
            Tool("decision_record"),
        ),
        (Post, "/api/v1/conversations/{id}/pins", Tool("pin_set")),
        (
            Delete,
            "/api/v1/conversations/{id}/pins/{kind}/{target_id}",
            Tool("pin_set"),
        ),
        (
            Patch,
            "/api/v1/account/connected-apps/{id}",
            Person("credentials are made and revoked by people"),
        ),
        (
            Post,
            "/api/v1/account/connected-apps/{id}/revoke",
            Person("credentials are made and revoked by people"),
        ),
        (Post, "/api/v1/direct/dms", Tool("conversation_start")),
        (Post, "/api/v1/direct/groups", Tool("conversation_start")),
        (
            Post,
            "/api/v1/conversations/{id}/members",
            Tool("conversation_members"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/members/{actor_id}/remove",
            Tool("conversation_members"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/continue",
            Tool("conversation_continue"),
        ),
        (
            Post,
            "/api/v1/reply-requests/{id}/retry",
            Tool("reply_retry"),
        ),
        (Post, "/api/v1/reply-requests/{id}/stop", Tool("reply_stop")),
        (
            Patch,
            "/api/v1/conversations/{id}/messages/{message_id}",
            Tool("message_edit"),
        ),
        (
            Delete,
            "/api/v1/conversations/{id}/messages/{message_id}",
            Tool("message_delete"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/files",
            Tool("file_upload"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/files",
            Tool("file_upload"),
        ),
        (Delete, "/api/v1/files/{id}", Tool("file_delete")),
        (
            Delete,
            "/api/v1/conversations/{id}/messages/{message_id}/link-previews/{preview_id}",
            Tool("message_preview_hide"),
        ),
        (
            Put,
            "/api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
            Tool("message_react"),
        ),
        (
            Delete,
            "/api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
            Tool("message_react"),
        ),
        (Post, "/api/v1/inbox/update", Tool("notification_update")),
        (
            Put,
            "/api/v1/notification-settings/channels",
            Person("a person's own notification preferences"),
        ),
        (
            Put,
            "/api/v1/notification-settings/mutes",
            Person("a person's own notification preferences"),
        ),
        (
            Put,
            "/api/v1/conversations/{id}/notification-level",
            Person("a person's own notification preferences"),
        ),
        (Post, "/api/v1/later", Tool("later_save")),
        (Patch, "/api/v1/later/{id}", Tool("later_update")),
        (Delete, "/api/v1/later/{id}", Tool("later_update")),
        (
            Post,
            "/api/v1/questions/{id}/answer",
            Tool("question_answer"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/scheduled-messages",
            Tool("message_schedule"),
        ),
        (
            Patch,
            "/api/v1/scheduled-messages/{id}",
            Tool("scheduled_message_update"),
        ),
        (
            Delete,
            "/api/v1/scheduled-messages/{id}",
            Tool("scheduled_message_update"),
        ),
        (
            Post,
            "/api/v1/scheduled-messages/{id}/send-now",
            Tool("scheduled_message_update"),
        ),
        (
            Post,
            "/api/v1/messages/{id}/forward",
            Tool("message_forward"),
        ),
        (
            Post,
            "/api/v1/push/subscriptions",
            Person("a person's own devices"),
        ),
        (
            Delete,
            "/api/v1/push/subscriptions/{id}",
            Person("a person's own devices"),
        ),
        (
            Post,
            "/api/v1/push/subscriptions/{id}/test",
            Person("a person's own devices"),
        ),
        (
            Post,
            "/api/v1/notification-actions",
            Plumbing("a button on a person's push notification"),
        ),
        (
            Post,
            "/api/v1/usage/accounts/{account_id}/refresh",
            Person("a person's own provider account"),
        ),
        (
            Post,
            "/api/v1/usage/accounts/{account_id}/schedules",
            Person("a person's own provider account"),
        ),
        (
            Delete,
            "/api/v1/usage/accounts/{account_id}/schedules/{schedule_id}",
            Person("a person's own provider account"),
        ),
        (
            Put,
            "/api/v1/usage/accounts/{account_id}/rules/{rule}",
            Person("a person's own provider account"),
        ),
        (
            Post,
            "/api/v1/usage/accounts/{account_id}/reset-credits/spend",
            Person("a person's own provider account"),
        ),
        (
            Post,
            "/api/v1/usage/accounts/{account_id}/batch",
            Tool("usage_batch_launch"),
        ),
        (
            Put,
            "/api/v1/organizations/{organization_id}/onboarding",
            Person("a person's own account"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/repositories",
            Tool("repository_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/repositories/{repository_id}/prepare",
            Tool("repository_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/repositories/{repository_id}/archive",
            Tool("repository_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/integrations/github/sync",
            Tool("github_integration_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/integrations/github/connect",
            Person("installs the GitHub app as a person"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/integrations/github/repositories",
            Tool("repository_write"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/integrations/github/automation",
            Tool("github_integration_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
            Tool("run_publish"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/agent-runs/{run_id}/pull-request",
            Tool("pull_request_create"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/pull-requests/{pull_request_id}/refresh",
            Tool("pull_request_refresh"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/pull-requests/{pull_request_id}/merge",
            Tool("pull_request_merge"),
        ),
        (
            Post,
            "/api/v1/integrations/github/webhook",
            Plumbing("GitHub's own deliveries"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/invitations",
            Tool("invitation_write"),
        ),
        (
            Post,
            "/api/v1/project-invitations/{invitation_id}/revoke",
            Tool("invitation_write"),
        ),
        (
            Post,
            "/api/v1/project-invitations/{invitation_id}/replace",
            Tool("invitation_write"),
        ),
        (
            Post,
            "/api/v1/project-invitations/resolve",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Post,
            "/api/v1/project-invitations/accept-new",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Post,
            "/api/v1/project-invitations/accept-existing",
            Plumbing("accepting an invitation is signing up"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/members/{actor_id}",
            Tool("project_member_set"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents",
            Person("adding an agent makes its credential"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/credentials/rotate",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/setup-link",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/remove",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/webhook",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/webhook/test",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
            Person("adding an agent makes its credential"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos/{binding_id}",
            Person("adding an agent makes its credential"),
        ),
        (
            Post,
            "/api/v1/agents/token",
            Plumbing("an agent trading its credential for a token"),
        ),
        (Post, "/oauth/register", Plumbing("OAuth")),
        (Post, "/oauth/authorize", Plumbing("OAuth")),
        (Post, "/connect/confirm", Plumbing("OAuth")),
        (Post, "/oauth/token", Plumbing("OAuth")),
        (Post, "/oauth/revoke", Plumbing("OAuth")),
        (
            Post,
            "/api/public/v1/{tool}",
            Plumbing("the MCP tools themselves, over HTTP"),
        ),
        (Post, "/api/v2/w/{workspace}/ops/{name}", Op),
        (Post, "/api/v2/ops/{name}", Op),
        (Post, "/api/v1/projects", Tool("project_create")),
        (Patch, "/api/v1/projects/{id}", Tool("project_update")),
        (
            Post,
            "/api/v1/projects/{id}/archive",
            Tool("project_archive"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/restore",
            Tool("project_archive"),
        ),
        (
            Put,
            "/api/v1/projects/rail-order",
            Person("each person's own sidebar"),
        ),
        (
            Put,
            "/api/v1/projects/{id}/navigation",
            Person("each person's own sidebar"),
        ),
        (
            Post,
            "/api/v1/projects/{id}/switch",
            Person("each person's own sidebar"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items",
            Tool("task_create"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}",
            Tool("task_update"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
            Tool("task_update"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/move",
            Tool("task_update"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
            Tool("task_move"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
            Tool("task_launch"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/launches",
            Tool("task_launch"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/launches/{launch_id}/cancel",
            Tool("launch_cancel"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/automations",
            Tool("automation_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/automations/{rule_id}",
            Tool("automation_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/automations/{rule_id}",
            Tool("automation_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/automations/{rule_id}/run",
            Tool("automation_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
            Tool("task_update"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
            Tool("task_criteria"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            Tool("task_criteria"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            Tool("task_criteria"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
            Tool("task_comment"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
            Tool("task_comment_edit"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
            Tool("task_comment_delete"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            Tool("task_link"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            Tool("task_link"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            Tool("task_link"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            Tool("task_link"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
            Tool("task_link"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/pull-requests/{pull_request_id}",
            Tool("task_link"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/pull-requests/{pull_request_id}",
            Tool("task_link"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/bulk",
            Tool("task_update_many"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/dependencies",
            Tool("task_depend"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/relations",
            Tool("task_relate"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/watch",
            Tool("task_watch"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/release-claim",
            Tool("task_release"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/labels",
            Tool("label_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/labels/{label_id}",
            Tool("label_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/fields",
            Tool("field_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/fields/{field_id}",
            Tool("field_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/collections",
            Tool("collection_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/collections/{collection_id}",
            Tool("collection_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/collections/{collection_id}/fields",
            Tool("collection_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/collections/{collection_id}/fields/{field_id}",
            Tool("collection_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/collections/{collection_id}/views",
            Tool("collection_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
            Tool("collection_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
            Tool("collection_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/collections/{collection_id}/records",
            Tool("record_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
            Tool("record_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/sheets",
            Tool("sheet_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/sheets/{sheet_id}",
            Tool("sheet_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/sheets/import",
            Tool("sheet_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/sheets/{sheet_id}/changes",
            Tool("sheet_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/milestones",
            Tool("milestone_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/milestones/{milestone_id}",
            Tool("milestone_write"),
        ),
        (
            Post,
            "/api/v1/organizations/{organization_id}/ai/test",
            Tool("ai_analyze"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/test",
            Tool("ai_analyze"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/analyze",
            Tool("ai_analyze"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/accept",
            Tool("ai_suggestion_decide"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/dismiss",
            Tool("ai_suggestion_decide"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/plans",
            Tool("plan_import"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/ai/plans/{import_id}",
            Tool("plan_import"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/ai/plans/{import_id}/commit",
            Tool("plan_import"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/triage",
            Tool("ai_analyze"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/criteria",
            Tool("ai_analyze"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/summary",
            Tool("ai_summarize"),
        ),
        (
            Post,
            "/api/v1/conversations/{id}/ai/summary",
            Tool("ai_summarize"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/views",
            Tool("view_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/views/{view_id}",
            Tool("view_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/views/{view_id}",
            Tool("view_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/views/{view_id}/pin",
            Tool("view_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/task-templates",
            Tool("template_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/task-templates/{template_id}",
            Tool("template_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/task-templates/{template_id}/apply",
            Tool("task_from_template"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/imports",
            Tool("task_import"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/imports/{import_id}",
            Tool("task_import"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/imports/{import_id}/commit",
            Tool("task_import"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/save-as-template",
            Tool("project_template_write"),
        ),
        (
            Patch,
            "/api/v1/organizations/{organization_id}/project-templates/{template_id}",
            Tool("project_template_write"),
        ),
        (
            Post,
            "/api/v1/projects/{project_id}/recurring-tasks",
            Tool("recurring_write"),
        ),
        (
            Patch,
            "/api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
            Tool("recurring_write"),
        ),
        (
            Delete,
            "/api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
            Tool("recurring_write"),
        ),
        (
            Put,
            "/api/v1/projects/{project_id}/work-item-workflow",
            Tool("workflow_update"),
        ),
    ];

    #[test]
    fn every_web_write_has_its_mcp_tool_or_says_why_not() {
        let mut listed = BTreeMap::new();
        for (method, path, parity) in WEB_WRITES {
            assert!(
                listed.insert((*method, *path), parity).is_none(),
                "{method:?} {path} is listed twice in agent_parity"
            );
        }
        let routes = MUTATING_ENTRY_POINTS
            .iter()
            .filter(|entry| entry.method != MutationMethod::Protocol)
            .map(|entry| (entry.method, entry.path))
            .collect::<BTreeSet<_>>();

        let unlisted = routes
            .iter()
            .filter(|route| !listed.contains_key(route))
            .map(|(method, path)| format!("{method:?} {path}"))
            .collect::<Vec<_>>();
        assert!(
            unlisted.is_empty(),
            "these web writes have no MCP tool listed in agent_parity.rs: {unlisted:#?}\n\
             Agents can do everything a person can (AWD-91): add the MCP tool that does the \
             same thing and list it as Tool, or, if only a person should ever do it, as Person \
             with the reason."
        );
        let stale = listed
            .keys()
            .filter(|route| !routes.contains(route))
            .map(|(method, path)| format!("{method:?} {path}"))
            .collect::<Vec<_>>();
        assert!(
            stale.is_empty(),
            "agent_parity.rs lists routes that no longer exist: {stale:#?}"
        );

        let tools = crate::mcp::tool_catalog()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<BTreeSet<_>>();
        for (method, path, parity) in WEB_WRITES {
            match parity {
                Tool(names) => {
                    for name in names.split(", ") {
                        assert!(
                            tools.contains(name),
                            "{method:?} {path} names {name}, which isn't an MCP tool"
                        );
                    }
                }
                Person(why) | Plumbing(why) => {
                    assert!(!why.trim().is_empty(), "{method:?} {path} needs a reason");
                }
                Op => {
                    for entry in crate::ops::registry().entries() {
                        assert!(
                            tools.contains(entry.meta.name),
                            "operation {} isn't an MCP tool",
                            entry.meta.name
                        );
                    }
                }
            }
        }
    }
}
