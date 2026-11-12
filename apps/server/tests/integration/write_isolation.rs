//! Two-workspace isolation for writes (finding F4 of the test suite audit).
//! `workspace_isolation` proves reads: every GET route and read-only MCP tool,
//! called as workspace A with workspace B's IDs, returns nothing of B's. This
//! does the same for everything that changes data. On the same A/B fixture,
//! signed in as A's owner (and through A's connected app and A's agent):
//!
//! - every POST, PUT, PATCH and DELETE route is called with B's IDs, alone
//!   and mixed with A's, with a body that works on A's own resource;
//! - every MCP tool that writes is called the same way.
//!
//! A request that names B's resource in its path, or as the thing it acts
//! on, must be refused with 403 `forbidden` or 404 `not_found` (a tool:
//! `not_found`, `permission_denied`, or the transport's 403). One that acts
//! on A's resource but points at one of B's (a parent, a label, a channel to
//! post to) may also be turned away as invalid. Either way nothing B owns may
//! change: every row, in any table, that mentions one of B's IDs is read
//! before and after, and the two must match. A row of A's that starts
//! pointing at B shows up there too.
//!
//! Then each case sends A's own request, the same body with A's IDs, which
//! must succeed. That shows the body was valid, so the refusals were about
//! B's IDs and not a bad request.
//!
//! # Adding a feature
//!
//! - a new POST, PUT, PATCH or DELETE route: add `"METHOD /path"`, with the
//!   path exactly as written in `.route("...")`, to [`WRITE_ROUTES`] and a
//!   case to `http_cases`. Routes that can't be called this way (sign-in
//!   plumbing, capability tokens) go in [`WRITE_EXCLUSIONS`] with the reason.
//! - a new MCP tool that isn't read-only: add it to [`MCP_WRITE_TOOLS`] and a
//!   case to `mcp_cases`, or to [`MCP_WRITE_EXCLUSIONS`] with the reason.
//! - a new kind of data a write acts on: make B's (and A's) in
//!   `more_targets`, or a spare of A's in `spares` when A's own request uses
//!   it up, if the shared fixture doesn't have one.
//!
//! `every_write_route_and_write_tool_has_an_isolation_case` reads the server
//! sources and fails until that's done.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use agent_workspace_server::{
    domain::allocate_conversation_sequence,
    policy_inventory::{MUTATING_ENTRY_POINTS, MutationMethod},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, send},
    message_edits::text,
    workspace_isolation::{
        MARKER, ORIGIN, TINY_PNG, TwoWorkspaces, World, call_tool, collect_sources,
        first_string_literal, has_call, json_body as rpc_reply, leaked, login_as, route_blocks,
        string_at, structured, succeeded, sweep_ids, tiny_png, two_workspaces_with, upload,
    },
};

/// Every mutating route this suite calls, as `"METHOD /path"` with the path
/// as written in the server's `.route(...)` calls.
const WRITE_ROUTES: &[&str] = &[
    "POST /api/public/v1/{tool}",
    "POST /api/v1/account/api-tokens",
    "POST /api/v1/account/api-tokens/{credential_id}/revoke",
    "PATCH /api/v1/account/connected-apps/{id}",
    "POST /api/v1/account/connected-apps/{id}/revoke",
    "PUT /api/v1/account/default-workspace",
    "POST /api/v1/account/sessions/{session_id}/revoke",
    "POST /api/v1/agent-approvals/{id}/respond",
    "POST /api/v1/agent-runs/{id}/acknowledge-failure",
    "POST /api/v1/agent-runs/{id}/interrupt",
    "POST /api/v1/agent-runs/{id}/retry",
    "DELETE /api/v1/bug-report-links/{link_id}",
    "PATCH /api/v1/bug-report-links/{link_id}",
    "POST /api/v1/bug-report-links/{link_id}/replace",
    "POST /api/v1/channels/{id}/archive",
    "POST /api/v1/channels/{id}/messages",
    "POST /api/v1/channels/{id}/restore",
    "POST /api/v1/channels/{id}/threads",
    "POST /api/v1/conversations/{id}/ai/summary",
    "POST /api/v1/conversations/{id}/continue",
    "POST /api/v1/conversations/{id}/files",
    "POST /api/v1/conversations/{id}/mark-seen",
    "POST /api/v1/conversations/{id}/mark-unread",
    "POST /api/v1/conversations/{id}/members",
    "POST /api/v1/conversations/{id}/members/{actor_id}/remove",
    "POST /api/v1/conversations/{id}/messages",
    "DELETE /api/v1/conversations/{id}/messages/{message_id}",
    "PATCH /api/v1/conversations/{id}/messages/{message_id}",
    "DELETE /api/v1/conversations/{id}/messages/{message_id}/link-previews/{preview_id}",
    "DELETE /api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
    "PUT /api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
    "PUT /api/v1/conversations/{id}/notification-level",
    "POST /api/v1/conversations/{id}/pins",
    "DELETE /api/v1/conversations/{id}/pins/{kind}/{target_id}",
    "POST /api/v1/conversations/{id}/scheduled-messages",
    "POST /api/v1/conversations/{id}/threads",
    "POST /api/v1/direct/dms",
    "POST /api/v1/direct/groups",
    "DELETE /api/v1/files/{id}",
    "POST /api/v1/inbox/update",
    "POST /api/v1/later",
    "DELETE /api/v1/later/{id}",
    "PATCH /api/v1/later/{id}",
    "POST /api/v1/messages/{id}/forward",
    "PUT /api/v1/notification-settings/channels",
    "PUT /api/v1/notification-settings/mutes",
    "PATCH /api/v1/organizations/{organization_id}",
    "POST /api/v1/organizations/{organization_id}/ai/test",
    "POST /api/v1/organizations/{organization_id}/harnesses",
    "DELETE /api/v1/organizations/{organization_id}/harnesses/{harness_id}",
    "PATCH /api/v1/organizations/{organization_id}/harnesses/{harness_id}",
    "POST /api/v1/organizations/{organization_id}/invitations",
    "PUT /api/v1/organizations/{organization_id}/layout",
    "PUT /api/v1/organizations/{organization_id}/layout/mine",
    "PUT /api/v1/organizations/{organization_id}/modules",
    "POST /api/v1/organizations/{organization_id}/workspace-templates",
    "POST /api/v1/organizations/{organization_id}/workspace-templates/apply",
    "DELETE /api/v1/organizations/{organization_id}/workspace-templates/{template_id}",
    "POST /api/v1/organizations/{organization_id}/layout/presets",
    "DELETE /api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
    "PATCH /api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
    "POST /api/v1/organizations/{organization_id}/leave",
    "DELETE /api/v1/organizations/{organization_id}/mail",
    "PUT /api/v1/organizations/{organization_id}/mail",
    "POST /api/v1/organizations/{organization_id}/mail/test",
    "PATCH /api/v1/organizations/{organization_id}/members/{actor_id}",
    "PUT /api/v1/organizations/{organization_id}/members/{actor_id}/agent-limit",
    "PUT /api/v1/organizations/{organization_id}/members/{actor_id}/role",
    "DELETE /api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
    "PUT /api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
    "POST /api/v1/organizations/{organization_id}/model-catalog/{kind}/refresh",
    "PUT /api/v1/organizations/{organization_id}/onboarding",
    "DELETE /api/v1/organizations/{organization_id}/personal-secrets/{name}",
    "PUT /api/v1/organizations/{organization_id}/personal-secrets/{name}",
    "PATCH /api/v1/organizations/{organization_id}/project-templates/{template_id}",
    "POST /api/v1/organizations/{organization_id}/roles",
    "DELETE /api/v1/organizations/{organization_id}/roles/{role_id}",
    "PATCH /api/v1/organizations/{organization_id}/roles/{role_id}",
    "DELETE /api/v1/organizations/{organization_id}/secrets/{name}",
    "PUT /api/v1/organizations/{organization_id}/secrets/{name}",
    "POST /api/v1/organizations/{organization_id}/transfer-ownership",
    "PATCH /api/v1/pages/{id}",
    "POST /api/v1/pages/{id}/comments",
    "DELETE /api/v1/pages/{id}/comments/{comment_id}",
    "PATCH /api/v1/pages/{id}/comments/{comment_id}",
    "POST /api/v1/pages/{id}/document/sync",
    "POST /api/v1/pages/{id}/document/updates",
    "POST /api/v1/pages/{id}/suggestions/{suggestion_id}/decision",
    "POST /api/v1/project-invitations/{invitation_id}/replace",
    "POST /api/v1/project-invitations/{invitation_id}/revoke",
    "POST /api/v1/projects",
    "PUT /api/v1/projects/rail-order",
    "PATCH /api/v1/projects/{id}",
    "POST /api/v1/projects/{id}/archive",
    "POST /api/v1/projects/{id}/channels",
    "POST /api/v1/projects/{id}/decisions",
    "POST /api/v1/projects/{id}/execution-profiles",
    "POST /api/v1/projects/{id}/harnesses/{kind}/catalog/refresh",
    "DELETE /api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
    "PUT /api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
    "PUT /api/v1/projects/{id}/navigation",
    "POST /api/v1/projects/{id}/pages",
    "POST /api/v1/projects/{id}/pages/import",
    "POST /api/v1/projects/{id}/restore",
    "POST /api/v1/projects/{id}/switch",
    "POST /api/v1/projects/{project_id}/agent-profiles",
    "DELETE /api/v1/projects/{project_id}/agent-profiles/{profile_id}",
    "PATCH /api/v1/projects/{project_id}/agent-profiles/{profile_id}",
    "POST /api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
    "POST /api/v1/projects/{project_id}/agent-runs/{run_id}/pull-request",
    "POST /api/v1/projects/{project_id}/ai/analyze",
    "POST /api/v1/projects/{project_id}/ai/plans",
    "PUT /api/v1/projects/{project_id}/ai/plans/{import_id}",
    "POST /api/v1/projects/{project_id}/ai/plans/{import_id}/commit",
    "POST /api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/accept",
    "POST /api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/dismiss",
    "POST /api/v1/projects/{project_id}/ai/test",
    "POST /api/v1/projects/{project_id}/automations",
    "DELETE /api/v1/projects/{project_id}/automations/{rule_id}",
    "PATCH /api/v1/projects/{project_id}/automations/{rule_id}",
    "POST /api/v1/projects/{project_id}/automations/{rule_id}/run",
    "POST /api/v1/projects/{project_id}/bug-report-links",
    "POST /api/v1/projects/{project_id}/environments",
    "PUT /api/v1/projects/{project_id}/environments/{environment_id}",
    "POST /api/v1/projects/{project_id}/environments/{environment_id}/archive",
    "POST /api/v1/projects/{project_id}/environments/{environment_id}/check",
    "POST /api/v1/projects/{project_id}/environments/{environment_id}/default",
    "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
    "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
    "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
    "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
    "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
    "POST /api/v1/projects/{project_id}/external-agents",
    "PATCH /api/v1/projects/{project_id}/external-agents/{connection_id}",
    "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/credentials/rotate",
    "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/remove",
    "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/webhook",
    "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/webhook/test",
    "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
    "DELETE /api/v1/projects/{project_id}/external-agents/{connection_id}/workos/{binding_id}",
    "POST /api/v1/projects/{project_id}/imports",
    "PATCH /api/v1/projects/{project_id}/imports/{import_id}",
    "POST /api/v1/projects/{project_id}/imports/{import_id}/commit",
    "PUT /api/v1/projects/{project_id}/integrations/github/automation",
    "POST /api/v1/projects/{project_id}/integrations/github/connect",
    "POST /api/v1/projects/{project_id}/integrations/github/repositories",
    "POST /api/v1/projects/{project_id}/integrations/github/sync",
    "POST /api/v1/projects/{project_id}/invitations",
    "POST /api/v1/projects/{project_id}/labels",
    "PATCH /api/v1/projects/{project_id}/labels/{label_id}",
    "POST /api/v1/projects/{project_id}/fields",
    "PATCH /api/v1/projects/{project_id}/fields/{field_id}",
    "POST /api/v1/projects/{project_id}/collections",
    "PATCH /api/v1/projects/{project_id}/collections/{collection_id}",
    "POST /api/v1/projects/{project_id}/collections/{collection_id}/fields",
    "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/fields/{field_id}",
    "POST /api/v1/projects/{project_id}/collections/{collection_id}/views",
    "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
    "DELETE /api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
    "POST /api/v1/projects/{project_id}/collections/{collection_id}/records",
    "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
    "POST /api/v1/projects/{project_id}/launches",
    "POST /api/v1/projects/{project_id}/launches/{launch_id}/cancel",
    "PUT /api/v1/projects/{project_id}/layout",
    "PUT /api/v1/projects/{project_id}/layout/rules",
    "POST /api/v1/projects/{project_id}/members",
    "PATCH /api/v1/projects/{project_id}/members/{actor_id}",
    "POST /api/v1/projects/{project_id}/milestones",
    "PATCH /api/v1/projects/{project_id}/milestones/{milestone_id}",
    "POST /api/v1/projects/{project_id}/provider-accounts",
    "DELETE /api/v1/projects/{project_id}/provider-accounts/{account_id}",
    "PATCH /api/v1/projects/{project_id}/provider-accounts/{account_id}",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/api-key",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/browser",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/cancel",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/device-code",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/logout",
    "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/use",
    "POST /api/v1/projects/{project_id}/pull-requests/{pull_request_id}/refresh",
    "POST /api/v1/projects/{project_id}/recurring-tasks",
    "DELETE /api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
    "PATCH /api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
    "POST /api/v1/projects/{project_id}/repositories",
    "POST /api/v1/projects/{project_id}/repositories/{repository_id}/archive",
    "POST /api/v1/projects/{project_id}/repositories/{repository_id}/prepare",
    "POST /api/v1/projects/{project_id}/save-as-template",
    "DELETE /api/v1/projects/{project_id}/secrets/{name}",
    "PUT /api/v1/projects/{project_id}/secrets/{name}",
    "POST /api/v1/projects/{project_id}/task-templates",
    "PATCH /api/v1/projects/{project_id}/task-templates/{template_id}",
    "POST /api/v1/projects/{project_id}/task-templates/{template_id}/apply",
    "POST /api/v1/projects/{project_id}/views",
    "DELETE /api/v1/projects/{project_id}/views/{view_id}",
    "PATCH /api/v1/projects/{project_id}/views/{view_id}",
    "POST /api/v1/projects/{project_id}/views/{view_id}/pin",
    "PUT /api/v1/projects/{project_id}/work-item-workflow",
    "POST /api/v1/projects/{project_id}/work-items",
    "POST /api/v1/projects/{project_id}/work-items/bulk",
    "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
    "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
    "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/criteria",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/summary",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/triage",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
    "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
    "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/dependencies",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/files",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/move",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/relations",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/release-claim",
    "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
    "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/watch",
    "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
    "POST /api/v1/push/subscriptions",
    "DELETE /api/v1/push/subscriptions/{id}",
    "POST /api/v1/push/subscriptions/{id}/test",
    "POST /api/v1/reply-requests/{id}/retry",
    "POST /api/v1/reply-requests/{id}/stop",
    "DELETE /api/v1/scheduled-messages/{id}",
    "PATCH /api/v1/scheduled-messages/{id}",
    "POST /api/v1/scheduled-messages/{id}/send-now",
    "PUT /api/v1/settings/values/{key}",
    "POST /api/v1/settings/values/{key}/reset",
    "PATCH /api/v1/threads/{id}",
    "POST /api/v1/threads/{id}/agent-runs",
    "POST /api/v1/threads/{id}/archive",
    "POST /api/v1/threads/{id}/messages",
    "POST /api/v1/threads/{id}/restore",
    "POST /api/v1/usage/accounts/{account_id}/batch",
    "POST /api/v1/usage/accounts/{account_id}/refresh",
    "POST /api/v1/usage/accounts/{account_id}/reset-credits/spend",
    "PUT /api/v1/usage/accounts/{account_id}/rules/{rule}",
    "POST /api/v1/usage/accounts/{account_id}/schedules",
    "DELETE /api/v1/usage/accounts/{account_id}/schedules/{schedule_id}",
    "POST /api/v1/webhooks",
    "DELETE /api/v1/webhooks/{webhook_id}",
    "PATCH /api/v1/webhooks/{webhook_id}",
    "POST /api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver",
    "POST /api/v1/webhooks/{webhook_id}/rotate-secret",
    "POST /api/v1/webhooks/{webhook_id}/test",
    "POST /api/v1/whiteboard-trash/empty",
    "POST /api/v1/whiteboards",
    "DELETE /api/v1/whiteboards/{id}",
    "PATCH /api/v1/whiteboards/{id}",
    "POST /api/v1/whiteboards/{id}/duplicate",
    "POST /api/v1/whiteboards/{id}/files",
    "POST /api/v1/whiteboards/{id}/links",
    "DELETE /api/v1/whiteboards/{id}/links/{kind}/{target_id}",
    "POST /api/v1/whiteboards/{id}/operations",
    "POST /api/v1/whiteboards/{id}/revisions/{revision}/restore",
    "POST /api/v1/whiteboards/{id}/scene",
    "POST /api/v1/workspace-invitations/{invitation_id}/revoke",
    "POST /connect/confirm",
    "POST /oauth/authorize",
];

const OWN_ACCOUNT: &str = "acts only on the signed-in person's own account and takes no \
                           workspace or resource ID, so there is nothing of B's to name";
const SIGN_IN: &str = "sign-in plumbing: works on the caller's own session or on a \
                       one-time token, takes no workspace or resource ID";
const CAPABILITY: &str = "the secret token in the request is the permission; it takes no \
                          session, and the token, not an ID, picks what it acts on";

/// Mutating routes this suite deliberately doesn't call, and why.
const WRITE_EXCLUSIONS: &[(&str, &str)] = &[
    ("PUT /api/v1/account/appearance", OWN_ACCOUNT),
    ("DELETE /api/v1/account/email", OWN_ACCOUNT),
    ("PUT /api/v1/account/email", OWN_ACCOUNT),
    ("POST /api/v1/account/email/resend", OWN_ACCOUNT),
    ("POST /api/v1/account/password", OWN_ACCOUNT),
    ("PATCH /api/v1/account/profile", OWN_ACCOUNT),
    ("POST /api/v1/account/sessions/revoke-others", OWN_ACCOUNT),
    ("POST /api/v1/account/two-factor/confirm", OWN_ACCOUNT),
    ("POST /api/v1/account/two-factor/disable", OWN_ACCOUNT),
    (
        "POST /api/v1/account/two-factor/recovery-codes",
        OWN_ACCOUNT,
    ),
    ("POST /api/v1/account/two-factor/setup", OWN_ACCOUNT),
    (
        "POST /api/v1/agents/token",
        "mints an agent's token from its own WorkOS refresh token; no session and no \
         workspace ID",
    ),
    ("POST /api/v1/auth/login", SIGN_IN),
    ("POST /api/v1/auth/login/two-factor", SIGN_IN),
    ("POST /api/v1/auth/logout", SIGN_IN),
    ("POST /api/v1/auth/password-reset", SIGN_IN),
    ("POST /api/v1/auth/password-reset/complete", SIGN_IN),
    ("POST /api/v1/auth/password-reset/inspect", SIGN_IN),
    ("POST /api/v1/auth/verify-email", SIGN_IN),
    ("POST /api/v1/auth/workos/start", SIGN_IN),
    ("POST /api/v1/bug-reports/{token}", CAPABILITY),
    (
        "POST /api/v1/client-errors",
        "browser error reports: counted in process metrics, never stored with a \
         workspace, and takes no ID",
    ),
    (
        "POST /api/v1/integrations/github/webhook",
        "GitHub's own signed delivery, checked against the app's webhook secret, not a \
         session; covered in the GitHub tests",
    ),
    (
        "MCP /api/v1/mcp",
        "the MCP transport: every tool behind it that writes has a case in mcp_cases",
    ),
    ("POST /api/v1/notification-actions", CAPABILITY),
    (
        "POST /api/v1/project-invitations/accept-existing",
        CAPABILITY,
    ),
    ("POST /api/v1/project-invitations/accept-new", CAPABILITY),
    ("POST /api/v1/project-invitations/resolve", CAPABILITY),
    (
        "POST /api/v1/workspace-invitations/accept-existing",
        CAPABILITY,
    ),
    ("POST /api/v1/workspace-invitations/accept-new", CAPABILITY),
    ("POST /api/v1/workspace-invitations/resolve", CAPABILITY),
    (
        "POST /oauth/register",
        "OAuth client registration: makes a public client that belongs to no workspace",
    ),
    (
        "POST /oauth/revoke",
        "OAuth: revokes the token it is given, which is the permission; no session",
    ),
    (
        "POST /oauth/token",
        "OAuth: trades a code or refresh token the caller holds; no session and no ID",
    ),
];

/// Every MCP tool that isn't read-only, which `mcp_cases` covers.
const MCP_WRITE_TOOLS: &[&str] = &[
    "ai_analyze",
    "ai_suggestion_decide",
    "ai_summarize",
    "approval_respond",
    "automation_write",
    "channel_create",
    "conversation_continue",
    "conversation_members",
    "conversation_start",
    "conversation_update",
    "decision_record",
    "file_delete",
    "file_upload",
    "invitation_write",
    "label_write",
    "field_write",
    "collection_write",
    "record_write",
    "later_save",
    "later_update",
    "launch_cancel",
    "mark_read",
    "message_delete",
    "message_edit",
    "message_forward",
    "message_react",
    "message_schedule",
    "message_send",
    "milestone_write",
    "notification_update",
    "page_comment",
    "page_comment_update",
    "page_edit",
    "page_suggestion_decide",
    "page_write",
    "pin_set",
    "plan_import",
    "project_archive",
    "project_layout_set",
    "project_member_set",
    "project_update",
    "reply_claim",
    "reply_pass",
    "reply_progress",
    "reply_release",
    "reply_send",
    "recurring_write",
    "reply_stop",
    "reply_retry",
    "role_assign",
    "role_write",
    "run_request",
    "run_retry",
    "run_start",
    "run_stop",
    "scheduled_message_update",
    "session_bind",
    "task_claim",
    "task_comment",
    "task_comment_delete",
    "task_comment_edit",
    "task_create",
    "task_create_many",
    "task_criteria",
    "task_depend",
    "task_from_template",
    "task_import",
    "task_launch",
    "task_link",
    "task_link_pull_request",
    "task_mark_blocked",
    "task_mark_ready_for_review",
    "task_move",
    "task_relate",
    "task_release",
    "task_reorder",
    "task_update",
    "task_update_many",
    "task_watch",
    "template_write",
    "thread_start",
    "thread_update",
    "usage_batch_launch",
    "view_write",
    "webhook_write",
    "whiteboard_create",
    "whiteboard_duplicate",
    "whiteboard_edit",
    "whiteboard_link",
    "whiteboard_update",
    "whiteboard_trash_empty",
    "workflow_update",
    "workspace_template_apply",
    "workspace_template_save",
    "project_template_write",
];

/// Write tools this suite deliberately doesn't call, and why.
const MCP_WRITE_EXCLUSIONS: &[(&str, &str)] = &[(
    "project_create",
    "makes a project in the caller's own workspace and takes no workspace ID; a template it \
     names is looked up only in that workspace",
)];

// ---------------------------------------------------------------------------
// Coverage guard (no database needed)
// ---------------------------------------------------------------------------

#[test]
fn every_write_route_and_write_tool_has_an_isolation_case() {
    let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    collect_sources(&source_root, &mut sources);
    assert!(
        !sources.is_empty(),
        "no server sources under {source_root:?}"
    );

    // Every mutating route the server registers (test-only fakes at the end
    // of a file aside), and every one the policy inventory lists.
    let mut routes = sources
        .iter()
        .flat_map(|(_, source)| mutating_routes(without_tests(source)))
        .collect::<BTreeSet<_>>();
    routes.extend(MUTATING_ENTRY_POINTS.iter().map(|entry| {
        let method = match entry.method {
            MutationMethod::Post => "POST",
            MutationMethod::Put => "PUT",
            MutationMethod::Patch => "PATCH",
            MutationMethod::Delete => "DELETE",
            MutationMethod::Protocol => "MCP",
        };
        format!("{method} {}", entry.path)
    }));
    let covered = WRITE_ROUTES
        .iter()
        .map(|route| (*route).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        covered.len(),
        WRITE_ROUTES.len(),
        "WRITE_ROUTES repeats a route"
    );
    let excluded = WRITE_EXCLUSIONS
        .iter()
        .map(|(route, _)| (*route).to_owned())
        .collect::<BTreeSet<_>>();
    assert!(
        covered.is_disjoint(&excluded),
        "routes both cased and excluded: {:#?}",
        covered.intersection(&excluded).collect::<Vec<_>>()
    );
    let missing = routes
        .iter()
        .filter(|route| !covered.contains(*route) && !excluded.contains(*route))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "mutating routes without a write-isolation case (add them to WRITE_ROUTES and \
         http_cases in tests/integration/write_isolation.rs, or to WRITE_EXCLUSIONS with \
         a reason): {missing:#?}"
    );
    let stale = covered
        .union(&excluded)
        .filter(|route| !routes.contains(*route))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "write-isolation routes that no longer exist: {stale:#?}"
    );

    let mcp_source = sources
        .iter()
        .find(|(path, _)| path.ends_with("src/mcp.rs"))
        .map(|(_, source)| source.as_str())
        .expect("src/mcp.rs");
    let write_tools = write_tools(mcp_source);
    assert!(
        write_tools.len() > 10,
        "parsed write tools: {write_tools:?}"
    );
    let cased = MCP_WRITE_TOOLS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let excluded = MCP_WRITE_EXCLUSIONS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert!(
        cased.is_disjoint(&excluded),
        "tools both cased and excluded: {:#?}",
        cased.intersection(&excluded).collect::<Vec<_>>()
    );
    let missing = write_tools
        .iter()
        .filter(|name| !cased.contains(*name) && !excluded.contains(*name))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "MCP tools that write without a write-isolation case (add them to MCP_WRITE_TOOLS \
         and mcp_cases in tests/integration/write_isolation.rs, or to MCP_WRITE_EXCLUSIONS \
         with a reason): {missing:#?}"
    );
    let stale = cased
        .union(&excluded)
        .filter(|name| !write_tools.contains(*name))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "write-isolation cases for tools that aren't write tools: {stale:#?}"
    );
}

/// The source before a trailing `#[cfg(test)] mod`, so test-only routers
/// (a provider's fake server) don't count as the app's routes.
fn without_tests(source: &str) -> &str {
    source
        .find("\n#[cfg(test)]\nmod ")
        .map_or(source, |end| &source[..end])
}

/// `"METHOD /path"` for each `.route("...", ...)` call that registers a
/// POST, PUT, PATCH or DELETE handler.
fn mutating_routes(source: &str) -> Vec<String> {
    route_blocks(source)
        .into_iter()
        .filter_map(|block| first_string_literal(block).map(|path| (block, path)))
        .flat_map(|(block, path)| {
            [
                ("POST", "post("),
                ("PUT", "put("),
                ("PATCH", "patch("),
                ("DELETE", "delete("),
            ]
            .into_iter()
            .filter(move |(_, token)| has_call(block, token))
            .map(move |(method, _)| format!("{method} {path}"))
        })
        .collect()
}

/// Names of `#[tool(...)]` methods not annotated `read_only_hint = true`.
fn write_tools(source: &str) -> BTreeSet<String> {
    let mut tools = BTreeSet::new();
    for (index, _) in source.match_indices("#[tool(") {
        let rest = &source[index..];
        let Some(function) = rest.find("async fn ") else {
            continue;
        };
        let attribute = rest[..function].split_whitespace().collect::<String>();
        if attribute.contains("read_only_hint=true") {
            continue;
        }
        let name_start = function + "async fn ".len();
        let name = rest[name_start..]
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .next()
            .expect("tool name");
        // An explicit `name = "..."` first in the attribute wins over the
        // method name.
        let explicit = attribute
            .strip_prefix("#[tool(name=\"")
            .and_then(|rest| rest.split('"').next());
        tools.insert(explicit.unwrap_or(name).to_owned());
    }
    tools
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// A request body.
#[derive(Clone)]
enum Payload {
    Json(Value),
    Bytes(&'static str, Vec<u8>),
    /// A form; a `"$csrf"` value becomes the sending session's CSRF token.
    Form(Vec<(&'static str, String)>),
}

#[derive(Clone)]
struct Call {
    method: &'static str,
    uri: String,
    payload: Payload,
    /// Sent with this bearer token instead of one of A's browser sessions.
    bearer: Option<String>,
}

impl Call {
    fn new(method: &'static str, uri: String, payload: Payload) -> Self {
        Self {
            method,
            uri,
            payload,
            bearer: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    /// The request acts on B's resource: 403 `forbidden` or 404 `not_found`.
    Refused,
    /// The request acts on A's resource but points at B's: any refusal
    /// (400, 403, 404, 409, 422), as A's own request shows the rest is valid.
    Rejected,
    /// The request only ever touches the caller's own rows (B's IDs are a
    /// filter, not a lookup), so it may succeed; the digest of B's rows and
    /// the leak check still apply.
    NoEffect,
}

/// A's own request, sent after every foreign one, or why there is none.
enum Own<T> {
    Run(T),
    /// A's own request gets past every access check to a step this suite
    /// can't provide (a signed-in provider account, GitHub), which answers
    /// with this error code; the reason says what's missing. That it gets
    /// that far is what the refusals of B's are measured against.
    Reaches(T, &'static str, &'static str),
    Skip(&'static str),
}

impl<T> Own<T> {
    fn sent(&self) -> Option<&T> {
        match self {
            Self::Run(sent) | Self::Reaches(sent, ..) => Some(sent),
            Self::Skip(_) => None,
        }
    }

    fn reaching(self, code: &'static str, reason: &'static str, what: &str) -> Self {
        match self {
            Self::Run(sent) => Self::Reaches(sent, code, reason),
            _ => panic!("{what}: only A's own request can stop at a step"),
        }
    }
}

struct HttpCase {
    route: &'static str,
    own: Own<Call>,
    foreign: Vec<(Call, Expect)>,
    /// A's own request ends or deletes something later ones need, so it
    /// runs after every other case's.
    last: bool,
}

struct McpCase {
    tool: &'static str,
    /// A's own arguments, sent as `caller`.
    own: Value,
    caller: Caller,
    /// Called as A's connected app and as A's agent, and as `caller` too.
    foreign: Vec<(Value, Expect)>,
    last: bool,
}

/// Who makes a tool call.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Caller {
    /// An app A's owner connected, acting as them.
    App,
    /// A's agent, in the Messages space.
    Agent,
    /// An agent in A's project that may ask a person to run Codex.
    Requester,
}

/// `route`'s path with each `{...}` filled in turn from `values`.
fn fill(route: &str, values: &[&str]) -> String {
    let (_, path) = route.split_once(' ').expect("METHOD /path");
    let mut filled = String::new();
    let mut rest = path;
    let mut values = values.iter();
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}').expect("closing brace") + open;
        filled.push_str(&rest[..open]);
        filled.push_str(
            values
                .next()
                .unwrap_or_else(|| panic!("too few values for {route}")),
        );
        rest = &rest[close + 1..];
    }
    filled.push_str(rest);
    assert!(values.next().is_none(), "too many values for {route}");
    filled
}

fn method_of(route: &'static str) -> &'static str {
    route.split_once(' ').expect("METHOD /path").0
}

/// A path parameter.
#[derive(Clone)]
enum P {
    /// An ID: A's and B's. Swapped to B's on its own and with the others.
    Id(String, String),
    /// An ID inside the resource the other IDs name. With only this one
    /// B's, the request is about A's resource, and the route answers as the
    /// `Expect` says: see [`removed`] and [`looked_up`].
    Within(String, String, Expect),
    /// A name that differs between the workspaces (a secret's): B's only
    /// when every ID is B's, as A's resource with B's name is just another
    /// of A's.
    Along(String, String),
    /// The same in both (a model, a reaction).
    Same(String),
}

fn id(a: impl ToString, b: impl ToString) -> P {
    P::Id(a.to_string(), b.to_string())
}

fn same(value: impl ToString) -> P {
    P::Same(value.to_string())
}

/// An ID the request takes out of the resource the other IDs name (a
/// member, a pin). With only this one B's, there is nothing of A's to take
/// out, and the route may say it's done; the digest still checks B's rows.
fn removed(a: impl ToString, b: impl ToString) -> P {
    P::Within(a.to_string(), b.to_string(), Expect::NoEffect)
}

/// An ID the route looks up inside the resource the other IDs name. With
/// only this one B's, it isn't found there: any refusal will do, as for an
/// ID that doesn't exist.
fn looked_up(a: impl ToString, b: impl ToString) -> P {
    P::Within(a.to_string(), b.to_string(), Expect::Rejected)
}

/// A column of a row as it is when the request is sent, so A's own requests
/// see what earlier ones changed: the row whose `keys` columns have these
/// UUIDs, or `default` when there is none.
fn current(table: &str, column: &str, keys: &[(&str, String)], default: i64) -> Value {
    let keys = keys
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    Value::String(format!("$current:{table}:{column}:{default}:{keys}"))
}

/// A browser's change that adds a paragraph to any page document, and its
/// state vector after it, both base64.
fn new_paragraph_update() -> (String, String) {
    use yrs::{
        Any, ClientID, Doc, OffsetKind, Options, ReadTxn, StateVector, Transact, Xml,
        XmlElementPrelim, XmlFragment, XmlTextPrelim, updates::encoder::Encode,
    };
    let doc = Doc::with_options(Options {
        client_id: ClientID::new(4_242),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    let fragment = doc.get_or_insert_xml_fragment("default");
    {
        let mut txn = doc.transact_mut();
        let paragraph = fragment.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        paragraph.insert_attribute(&mut txn, "id", Any::String("netblock1".into()));
        paragraph.insert(&mut txn, 0, XmlTextPrelim::new("Net"));
    }
    let txn = doc.transact();
    (
        STANDARD.encode(txn.encode_state_as_update_v1(&StateVector::default())),
        STANDARD.encode(txn.state_vector().encode_v1()),
    )
}

/// A row's version when the request is sent.
fn version(table: &str, row: impl ToString) -> Value {
    current(table, "version", &[("id", row.to_string())], 1)
}

/// A case for a route whose path names the resource it acts on. Own fills
/// every ID with A's; the foreign requests use B's for all of them, then for
/// each one alone, so each lookup is checked on its own. `body` gets the
/// path values of the request it's for.
fn path_case(route: &'static str, params: &[P], body: impl Fn(&[String]) -> Payload) -> HttpCase {
    let method = method_of(route);
    let pick = |swap: Option<usize>, all_b: bool| {
        params
            .iter()
            .enumerate()
            .map(|(index, param)| match param {
                P::Id(_, b) | P::Within(_, b, _) if all_b || swap == Some(index) => b.clone(),
                P::Id(a, _) | P::Within(a, _, _) => a.clone(),
                P::Along(_, b) if all_b => b.clone(),
                P::Along(a, _) => a.clone(),
                P::Same(value) => value.clone(),
            })
            .collect::<Vec<_>>()
    };
    let call = |values: Vec<String>| {
        Call::new(
            method,
            fill(
                route,
                &values.iter().map(String::as_str).collect::<Vec<_>>(),
            ),
            body(&values),
        )
    };
    let ids = params
        .iter()
        .enumerate()
        .filter(|(_, param)| matches!(param, P::Id(..) | P::Within(..)))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut variants = vec![(pick(None, true), Expect::Refused)];
    if ids.len() > 1 {
        variants.extend(ids.iter().map(|index| {
            let expect = match &params[*index] {
                P::Within(_, _, expect) => *expect,
                _ => Expect::Refused,
            };
            (pick(Some(*index), false), expect)
        }));
    }
    HttpCase {
        route,
        own: Own::Run(call(pick(None, false))),
        foreign: variants
            .into_iter()
            .map(|(values, expect)| (call(values), expect))
            .collect(),
        last: false,
    }
}

fn json_body(value: Value) -> impl Fn(&[String]) -> Payload {
    move |_| Payload::Json(value.clone())
}

/// `{}`: for routes that take an empty JSON object, and harmless for those
/// that take no body.
fn empty() -> impl Fn(&[String]) -> Payload {
    json_body(json!({}))
}

/// `rest` with `expectedVersion` set to the version of the row the path's
/// `index`th value names.
/// `{key: value}` with a key made at run time, such as a field's ID.
fn keyed(key: impl ToString, value: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(key.to_string(), value);
    Value::Object(object)
}

fn versioned(table: &'static str, index: usize, rest: Value) -> impl Fn(&[String]) -> Payload {
    move |values| {
        let mut body = rest.clone();
        body["expectedVersion"] = version(table, &values[index]);
        Payload::Json(body)
    }
}

impl HttpCase {
    /// More requests on A's path whose body points at B's resources.
    fn rejecting(mut self, bodies: Vec<Value>) -> Self {
        let Some(own) = self.own.sent() else {
            panic!(
                "{}: a body-reference check needs A's own request",
                self.route
            )
        };
        let calls = bodies
            .into_iter()
            .map(|body| {
                (
                    Call::new(own.method, own.uri.clone(), Payload::Json(body)),
                    Expect::Rejected,
                )
            })
            .collect::<Vec<_>>();
        self.foreign.extend(calls);
        self
    }

    /// More requests on A's path that name B's resources where the route
    /// only filters the caller's own rows by them: see [`Expect::NoEffect`].
    fn no_effect(mut self, bodies: Vec<Value>) -> Self {
        let Some(own) = self.own.sent() else {
            panic!("{}: a no-effect check needs A's own request", self.route)
        };
        let calls = bodies
            .into_iter()
            .map(|body| {
                (
                    Call::new(own.method, own.uri.clone(), Payload::Json(body)),
                    Expect::NoEffect,
                )
            })
            .collect::<Vec<_>>();
        self.foreign.extend(calls);
        self
    }

    /// More requests, each expected as given.
    fn and(mut self, calls: Vec<(Call, Expect)>) -> Self {
        self.foreign.extend(calls);
        self
    }

    /// Adds `query` to every request's URI.
    fn query(mut self, query: &str) -> Self {
        if let Own::Run(own) | Own::Reaches(own, ..) = &mut self.own {
            own.uri.push_str(query);
        }
        for (call, _) in &mut self.foreign {
            call.uri.push_str(query);
        }
        self
    }

    fn without_own(mut self, reason: &'static str) -> Self {
        self.own = Own::Skip(reason);
        self
    }

    /// A's own request stops at `code`, past the access checks: see
    /// [`Own::Reaches`].
    fn own_reaches(mut self, code: &'static str, reason: &'static str) -> Self {
        self.own =
            std::mem::replace(&mut self.own, Own::Skip(reason)).reaching(code, reason, self.route);
        self
    }

    fn last(mut self) -> Self {
        self.last = true;
        self
    }
}

/// A case for a route whose target is only in the body: `own` is A's,
/// `foreign` name B's.
fn body_case(route: &'static str, path: &[&str], own: Value, foreign: Vec<Value>) -> HttpCase {
    let method = method_of(route);
    let uri = fill(route, path);
    HttpCase {
        route,
        own: Own::Run(Call::new(method, uri.clone(), Payload::Json(own))),
        foreign: foreign
            .into_iter()
            .map(|body| {
                (
                    Call::new(method, uri.clone(), Payload::Json(body)),
                    Expect::Refused,
                )
            })
            .collect(),
        last: false,
    }
}

fn mcp_case(tool: &'static str, own: Value, foreign: Vec<Value>) -> McpCase {
    McpCase {
        tool,
        own,
        caller: Caller::App,
        foreign: foreign
            .into_iter()
            .map(|arguments| (arguments, Expect::Refused))
            .collect(),
        last: false,
    }
}

impl McpCase {
    fn rejecting(mut self, arguments: Vec<Value>) -> Self {
        self.foreign
            .extend(arguments.into_iter().map(|value| (value, Expect::Rejected)));
        self
    }

    fn no_effect(mut self, arguments: Vec<Value>) -> Self {
        self.foreign
            .extend(arguments.into_iter().map(|value| (value, Expect::NoEffect)));
        self
    }

    fn by_agent(mut self) -> Self {
        self.caller = Caller::Agent;
        self
    }

    fn by_requester(mut self) -> Self {
        self.caller = Caller::Requester;
        self
    }

    fn last(mut self) -> Self {
        self.last = true;
        self
    }
}

/// `value` with `key` set to `to`.
fn with(value: &Value, key: &str, to: Value) -> Value {
    let mut value = value.clone();
    value[key] = to;
    value
}

// ---------------------------------------------------------------------------
// More of each workspace's data, for writes to act on
// ---------------------------------------------------------------------------

/// IDs the shared fixture makes but doesn't keep, and a few more things
/// only writes need, in each workspace.
struct Targets {
    /// More sessions for A's owner: writes are limited per session per
    /// minute, so A's requests rotate through them. Empty for B.
    sessions: Vec<Browser>,
    owner: Uuid,
    /// The agent, as an actor.
    agent: Uuid,
    wren: Uuid,
    label: Uuid,
    /// The first task field (a single choice), and the person field.
    field: Uuid,
    person_field: Uuid,
    milestone: Uuid,
    view: Uuid,
    task_template: Uuid,
    recurring: Uuid,
    project_template: Uuid,
    environment: Uuid,
    agent_profile: Uuid,
    harness: Uuid,
    launch: Uuid,
    bug_link: Uuid,
    project_invitation: Uuid,
    workspace_invitation: Uuid,
    push_subscription: Uuid,
    /// The saved message.
    later: Uuid,
    scheduled: Uuid,
    usage_schedule: Uuid,
    criterion: Uuid,
    decision: Uuid,
    /// A status the task isn't in.
    status: Uuid,
    /// A task in Backlog, where a new task goes (the fixture's are in
    /// Ready), to place another before.
    backlog_task: Uuid,
    /// The workspace's saved layout preset.
    layout_preset: Uuid,
    /// A custom role with everything a member has (AWD-74), so whoever is
    /// given it can still do what other cases need.
    custom_role: Uuid,
    /// A template the workspace saved (AWD-76).
    workspace_template: Uuid,
    task_type: Uuid,
    suggestion: Uuid,
    /// The connected app's grant.
    grant: Uuid,
    /// The owner's newest session: B's is the one A tries to revoke.
    session: Uuid,
    api_token: Uuid,
    notification: Uuid,
    /// A failed delivery of the webhook.
    delivery: Uuid,
    /// A reply request waiting for the agent, and one it holds.
    reply: Uuid,
    claimed_reply: Uuid,
    /// The group the claimed request is in.
    claimed_conversation: String,
    /// Uploads nobody has sent yet, in the channel and for a comment on the
    /// task. A's own message and comment attach A's. A sent file can't be
    /// attached again anyway, so only B's unsent ones show that attaching
    /// checks whose files they are.
    channel_upload: Uuid,
    comment_upload: Uuid,
    /// The same in the thread and the DM, and the agent's own in the group
    /// of the request it holds.
    thread_upload: Uuid,
    dm_upload: Uuid,
    claimed_upload: Uuid,
    /// A reply request the owner stopped, to retry.
    stopped_reply: Uuid,
    /// Threads with no repository or run yet, to start runs in: one over
    /// HTTP, one through MCP.
    run_threads: [Uuid; 2],
    /// A run that failed, and the event that says so, to acknowledge and
    /// retry.
    failed_run: Uuid,
    failed_event: Uuid,
    /// A board in the trash. B's is one A's "Delete forever" and "Empty
    /// trash" must leave alone; A's goes when A empties its trash.
    trashed_whiteboard: Uuid,
    /// The collection's person, task link and page link fields (AWD-90).
    record_fields: [Uuid; 3],
}

async fn more_targets(app: &axum::Router, pool: &PgPool, world: &World) -> Targets {
    let a_side = world.marker.is_empty();
    let login = if a_side {
        "owner".to_owned()
    } else {
        format!("owner-{MARKER}")
    };
    let tag = if a_side { "a" } else { MARKER };
    let browser = &world.browser;
    let project = Uuid::parse_str(&world.project).expect("project");
    let organization = world.organization;
    let mut sessions = Vec::new();
    if a_side {
        for _ in 0..6 {
            sessions.push(login_as(app, &login).await);
        }
    }
    let owner = scalar(
        pool,
        "SELECT actor_id FROM users WHERE login_name = $1",
        &login,
    )
    .await;
    let wren = scalar(
        pool,
        "SELECT actor_id FROM users WHERE login_name = $1",
        format!("wren-{tag}"),
    )
    .await;
    let agent = scalar(
        pool,
        "SELECT actor_id FROM external_agent_connections WHERE id = $1",
        world.agent_connection,
    )
    .await;

    // The fake model, so the AI routes and tools run.
    ok(
        app,
        browser,
        "PUT",
        "/api/v1/settings/values/ai.provider",
        json!({"scope": "workspace", "organizationId": organization, "value": "fake"}),
    )
    .await;
    // So refusing to remove these proves something: each exists in both.
    for (method, uri, body) in [
        (
            "PUT",
            format!(
                "/api/v1/conversations/{}/messages/{}/reactions/thumbsup",
                world.channel, world.message
            ),
            json!({}),
        ),
        (
            "PUT",
            format!("/api/v1/organizations/{organization}/mail"),
            json!({
                "provider": "smtp",
                "smtp": {"host": "smtp.example.com", "port": 587, "security": "starttls"},
                "fromAddress": format!("{tag}@example.com"),
            }),
        ),
        (
            "PUT",
            format!(
                "/api/v1/organizations/{organization}/model-catalog/codex_app_server/models/net-model"
            ),
            json!({"displayName": format!("Model {tag}")}),
        ),
        (
            "PUT",
            format!(
                "/api/v1/projects/{project}/harnesses/codex_app_server/models/net-project-model"
            ),
            json!({"displayName": format!("Project model {tag}")}),
        ),
        (
            "POST",
            format!(
                "/api/v1/projects/{project}/work-items/{}/threads/{}",
                world.task, world.thread
            ),
            json!({}),
        ),
        (
            "POST",
            format!("/api/v1/organizations/{organization}/invitations"),
            json!({"role": "member", "expiresInHours": 24}),
        ),
        (
            "POST",
            "/api/v1/account/api-tokens".to_owned(),
            json!({"projectId": project, "name": format!("Token {tag}"), "scopes": ["read"], "expiresInDays": 30}),
        ),
    ] {
        ok(app, browser, method, &uri, body).await;
    }
    // After the thread link, which moves the task's version on.
    let version = version_now(pool, "work_items", world.task).await;
    ok(
        app,
        browser,
        "POST",
        &format!(
            "/api/v1/projects/{project}/work-items/{}/repositories/{}",
            world.task, world.repository
        ),
        json!({"expectedVersion": version, "role": "related"}),
    )
    .await;
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        json!({"title": "Net backlog task"}),
    )
    .await;
    let trashed = ok(
        app,
        browser,
        "POST",
        "/api/v1/whiteboards",
        json!({"title": format!("Net trashed {tag}"), "organizationId": organization}),
    )
    .await;
    let trashed_whiteboard =
        Uuid::parse_str(&string_at(&trashed, "/whiteboard/id")).expect("trashed board");
    ok(
        app,
        browser,
        "PATCH",
        &format!("/api/v1/whiteboards/{trashed_whiteboard}"),
        json!({"deleted": true}),
    )
    .await;

    // The agent's reply requests: one waiting, one it holds, one stopped.
    let (reply, _) = reply_request(app, pool, browser, agent, "pending").await;
    let (claimed_reply, claimed_conversation) =
        reply_request(app, pool, browser, agent, "claimed").await;
    let (stopped_reply, _) = reply_request(app, pool, browser, agent, "cancelled").await;

    // An unread notification for the owner, and a failed webhook delivery.
    let notification = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO notifications (id, project_id, recipient_actor_id, kind, subject_kind, subject_id, group_key)
        VALUES ($1, $2, $3, 'mention', 'message', $4, $5)
        "#,
    )
    .bind(notification)
    .bind(project)
    .bind(owner)
    .bind(Uuid::parse_str(&world.message).expect("message"))
    .bind(format!("net:{notification}"))
    .execute(pool)
    .await
    .expect("notification");
    let delivery = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO webhook_deliveries (id, subscription_id, organization_id, event_type, event_key, state, attempts)
        VALUES ($1, $2, $3, 'run.finished', $4, 'failed', 3)
        "#,
    )
    .bind(delivery)
    .bind(world.webhook)
    .bind(organization)
    .bind(format!("net:{delivery}"))
    .execute(pool)
    .await
    .expect("webhook delivery");

    let channel_upload = upload(
        app,
        browser,
        &format!(
            "/api/v1/conversations/{}/files?name=net-unsent-{tag}.txt",
            world.channel
        ),
        "text/plain",
        format!("Net unsent {tag}").into_bytes(),
    )
    .await;
    let comment_upload = upload(
        app,
        browser,
        &format!(
            "/api/v1/projects/{project}/work-items/{}/files?name=net-unsent-{tag}.txt&for=comment",
            world.task
        ),
        "text/plain",
        format!("Net unsent comment {tag}").into_bytes(),
    )
    .await;
    let mut uploads = Vec::new();
    for conversation in [&world.thread, &world.dm] {
        uploads.push(
            upload(
                app,
                browser,
                &format!("/api/v1/conversations/{conversation}/files?name=net-unsent-{tag}.txt"),
                "text/plain",
                format!("Net unsent {tag}").into_bytes(),
            )
            .await,
        );
    }
    let agent_session = initialize(app, &world.agent_token)
        .await
        .expect("agent MCP");
    let (_, uploaded) = call_tool(
        app,
        &world.agent_token,
        &agent_session,
        "file_upload",
        json!({
            "conversationId": claimed_conversation,
            "name": format!("net-unsent-{tag}.txt"),
            "text": format!("Net unsent reply {tag}"),
        }),
    )
    .await;
    let claimed_upload = structured(&uploaded)["value"]["fileId"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(|| panic!("agent upload: {uploaded}"));

    // Runs. The owner's Codex profile counts as signed in (admission checks
    // the stored state), so runs can start; nothing here runs them. Two
    // threads with no repository yet to start runs in, then one with a run
    // that failed, made the way the fixture makes its run.
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(world.profile)
        .execute(pool)
        .await
        .expect("profile signed in");
    let mut run_threads = Vec::new();
    for index in 0..3 {
        let thread = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
            VALUES ($1, $2, 'thread', $3::uuid, $4, $5)
            "#,
        )
        .bind(thread)
        .bind(project)
        .bind(&world.channel)
        .bind((index == 2).then_some(world.repository))
        .bind(format!("Net run thread {index}"))
        .execute(pool)
        .await
        .expect("run thread");
        run_threads.push(thread);
    }
    let failed_run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, failure_reason, started_at, finished_at
        )
        SELECT $1, $2, execution_worker_id, $3, $4, $5, 'failed', 'Net run', '{}'::jsonb,
               'Net failure.', now(), now()
        FROM agent_runs WHERE id = $6
        "#,
    )
    .bind(failed_run)
    .bind(run_threads[2])
    .bind(owner)
    .bind(world.profile)
    .bind(world.repository)
    .bind(world.run)
    .execute(pool)
    .await
    .expect("failed run");
    let failed_event = Uuid::now_v7();
    let mut transaction = pool.begin().await.expect("failure transaction");
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (id, agent_run_id, event_key, kind, summary, payload)
        VALUES ($1, $2, 'net:failed', 'run_failed', 'Net failure.', '{}'::jsonb)
        "#,
    )
    .bind(failed_event)
    .bind(failed_run)
    .execute(&mut *transaction)
    .await
    .expect("failure event");
    let sequence = allocate_conversation_sequence(&mut transaction, run_threads[2])
        .await
        .expect("failure sequence");
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind, agent_run_event_id)
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(run_threads[2])
    .bind(sequence)
    .bind(failed_event)
    .execute(&mut *transaction)
    .await
    .expect("failure entry");
    transaction.commit().await.expect("commit failure");

    let custom_role = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'net-role', 'member', $2,
                ARRAY['projects.view', 'tasks.view', 'tasks.create', 'tasks.edit', 'tasks.delete',
                      'channels.post', 'channels.create', 'pages.edit', 'whiteboards.edit',
                      'collections.edit', 'collections.build', 'runs.launch', 'runs.approve', 'runs.interrupt', 'runs.accounts',
                      'code.view', 'code.publish'])
        RETURNING id
        "#,
    )
    .bind(organization)
    .bind(format!("Net role {tag}"))
    .fetch_one(pool)
    .await
    .expect("custom role");
    let workspace_template = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_templates (id, organization_id, name, definition, created_by_actor_id)
        VALUES (gen_random_uuid(), $1, $2, '{"modules": ["messages"]}'::jsonb, $3)
        RETURNING id
        "#,
    )
    .bind(organization)
    .bind(format!("Net template {tag}"))
    .bind(owner)
    .fetch_one(pool)
    .await
    .expect("workspace template");
    let in_project = |sql: &'static str| scalar(pool, sql, project);
    let in_organization = |sql: &'static str| scalar(pool, sql, organization);
    Targets {
        custom_role,
        workspace_template,
        sessions,
        owner,
        agent,
        wren,
        label: in_project("SELECT id FROM project_labels WHERE project_id = $1").await,
        field: in_project(
            "SELECT id FROM field_definitions WHERE project_id = $1 AND collection_id IS NULL AND field_type = 'single_choice'",
        )
        .await,
        person_field: in_project(
            "SELECT id FROM field_definitions WHERE project_id = $1 AND collection_id IS NULL AND field_type = 'person'",
        )
        .await,
        milestone: in_project("SELECT id FROM project_milestones WHERE project_id = $1").await,
        view: in_project("SELECT id FROM saved_views WHERE project_id = $1 AND builtin IS NULL")
            .await,
        task_template: in_project("SELECT id FROM task_templates WHERE project_id = $1").await,
        recurring: in_project("SELECT id FROM recurring_tasks WHERE project_id = $1").await,
        project_template: in_organization(
            "SELECT id FROM project_templates WHERE organization_id = $1",
        )
        .await,
        environment: in_project("SELECT id FROM project_environments WHERE project_id = $1")
            .await,
        agent_profile: in_project("SELECT id FROM agent_profiles WHERE project_id = $1").await,
        harness: in_organization("SELECT id FROM custom_harnesses WHERE organization_id = $1")
            .await,
        launch: in_project("SELECT id FROM launches WHERE project_id = $1").await,
        bug_link: in_project("SELECT id FROM bug_report_links WHERE project_id = $1").await,
        project_invitation: in_project(
            "SELECT id FROM project_invitations WHERE project_id = $1",
        )
        .await,
        workspace_invitation: in_organization(
            "SELECT id FROM workspace_invitations WHERE organization_id = $1",
        )
        .await,
        push_subscription: in_organization(
            "SELECT id FROM push_subscriptions WHERE organization_id = $1",
        )
        .await,
        later: scalar(
            pool,
            "SELECT id FROM saved_items WHERE message_id = $1",
            Uuid::parse_str(&world.message).expect("message"),
        )
        .await,
        scheduled: scalar(
            pool,
            "SELECT id FROM scheduled_messages WHERE conversation_id = $1",
            Uuid::parse_str(&world.channel).expect("channel"),
        )
        .await,
        usage_schedule: in_organization(
            "SELECT id FROM usage_schedules WHERE organization_id = $1",
        )
        .await,
        criterion: scalar(
            pool,
            "SELECT id FROM work_item_acceptance_criteria WHERE work_item_id = $1",
            world.task,
        )
        .await,
        decision: in_project("SELECT id FROM project_decisions WHERE project_id = $1").await,
        status: scalar(
            pool,
            r#"
            SELECT status.id FROM work_item_statuses status
            JOIN work_items task ON task.project_id = status.project_id
            WHERE task.id = $1 AND status.enabled AND status.id <> task.status_id
            ORDER BY status.position LIMIT 1
            "#,
            world.task,
        )
        .await,
        backlog_task: in_project(
            "SELECT id FROM work_items WHERE project_id = $1 AND title = 'Net backlog task'",
        )
        .await,
        layout_preset: in_organization("SELECT id FROM layout_presets WHERE organization_id = $1")
            .await,
        task_type: in_project(
            "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position LIMIT 1",
        )
        .await,
        suggestion: in_project("SELECT id FROM ai_suggestions WHERE project_id = $1").await,
        grant: scalar(
            pool,
            r#"
            SELECT id FROM mcp_oauth_grants
            WHERE actor_id = $1 AND external_agent_connection_id IS NULL
            ORDER BY created_at LIMIT 1
            "#,
            owner,
        )
        .await,
        session: newest_session(pool, owner).await,
        api_token: scalar(
            pool,
            r#"
            SELECT id FROM mcp_client_credentials
            WHERE actor_id = $1 AND external_agent_connection_id IS NULL AND client_name LIKE 'Token %'
            "#,
            owner,
        )
        .await,
        notification,
        delivery,
        reply,
        claimed_reply,
        claimed_conversation,
        channel_upload,
        comment_upload,
        thread_upload: uploads[0],
        dm_upload: uploads[1],
        claimed_upload,
        stopped_reply,
        run_threads: [run_threads[0], run_threads[1]],
        failed_run,
        failed_event,
        trashed_whiteboard,
        record_fields: [
            collection_field(pool, world.collection, "person").await,
            collection_field(pool, world.collection, "task").await,
            collection_field(pool, world.collection, "page").await,
        ],
    }
}

/// A collection's person field, or its link field to tasks or pages.
async fn collection_field(pool: &PgPool, collection: Uuid, kind: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM field_definitions
        WHERE collection_id = $1
          AND CASE $2 WHEN 'person' THEN field_type = 'person'
                      ELSE field_type = 'link' AND config ->> 'target' = $2 END
        "#,
    )
    .bind(collection)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("collection {kind} field: {error}"))
}

/// What A's own requests use up: deleted, archived, revoked or answered
/// once. B needs none, as nothing of B's may change.
struct Spares {
    channel: String,
    thread: String,
    /// Archived and restored, linked, then moved to `projects[0]`.
    task: Uuid,
    /// In the same column as `task`, to reorder after it.
    second_task: Uuid,
    /// Archived over HTTP (the first, then restored) and through MCP.
    projects: [String; 2],
    /// Deleted over HTTP and through MCP.
    messages: [String; 2],
    file: Uuid,
    /// Unsent uploads A's own tool calls attach: in the channel and for a
    /// task comment, and the agent's, in the group its reply goes to.
    tool_upload: Uuid,
    tool_comment_upload: Uuid,
    /// An unsent upload A's own file_delete removes.
    tool_delete_upload: Uuid,
    reply_upload: Uuid,
    later: Uuid,
    /// Sent now over HTTP, and changed through MCP.
    scheduled: [Uuid; 2],
    harness: Uuid,
    bug_link: Uuid,
    project_invitation: Uuid,
    /// An agent in the project to change, rotate and remove.
    agent_connection: Uuid,
    /// Approvals runs are blocked on: one of the owner's runs', answered
    /// over HTTP, and one of rook's, answered through MCP (where nobody
    /// answers their own run's).
    approvals: [Uuid; 2],
    /// Dismissed over HTTP and through MCP.
    suggestions: [Uuid; 2],
    /// The owner's approval run, stopped through MCP once its approval is
    /// answered, and a failed run in its own thread, retried after that.
    run_stop: Uuid,
    run_retry: Uuid,
    /// A launch queued for next year, cancelled through MCP.
    launch: Uuid,
    /// A second connected app, revoked.
    grant: Uuid,
    oauth_client: String,
    /// Reply requests: stopped over HTTP; claimed, then progress and a
    /// reply; passed; released (held already); stopped through MCP.
    reply_stop: Uuid,
    reply_claim: Uuid,
    reply_pass: Uuid,
    reply_release: Uuid,
    reply_stop_mcp: Uuid,
    /// Stopped, for A's own reply_retry.
    reply_retry_mcp: Uuid,
    /// The project's statuses and types as they are, as workflow drafts.
    statuses: Vec<Value>,
    types: Vec<Value>,
    /// A sign-in of A's owner that nothing else uses, revoked.
    session: Uuid,
    /// An agent in A's project that may ask a person to run Codex.
    requester_token: String,
    /// A repository with no runs, archived.
    repository: Uuid,
    /// The owner's comments on the task, deleted over HTTP and through MCP.
    comments: [Uuid; 2],
    /// A board in the trash, deleted for good.
    trashed_whiteboard: Uuid,
    /// An agent and an account for runs, deleted.
    agent_profile: Uuid,
    provider_account: Uuid,
    /// A custom role nobody holds, removed.
    role: Uuid,
    /// A second view in A's collection, removed (a collection keeps one).
    collection_view: Uuid,
}

/// The redirect a hosted OAuth client registers.
const REDIRECT: &str = "https://agent.example.com/oauth/callback";
const VERIFIER: &str = "net-verifier-net-verifier-net-verifier-net-verifier";

async fn spares(app: &axum::Router, pool: &PgPool, a: &World, ax: &Targets) -> Spares {
    let browser = &a.browser;
    let project = Uuid::parse_str(&a.project).expect("project");
    let soon = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();

    let channel = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/channels"),
        json!({"name": "net-spare"}),
    )
    .await;
    let channel = string_at(&channel, "/conversation/id");
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/channels/{}/threads", a.channel),
        json!({"name": "Net spare thread"}),
    )
    .await;
    let thread = scalar(
        pool,
        "SELECT id FROM conversations WHERE kind = 'thread' AND name = $1",
        "Net spare thread",
    )
    .await
    .to_string();
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        json!({"title": "Net spare task"}),
    )
    .await;
    let task = scalar(
        pool,
        "SELECT id FROM work_items WHERE title = $1",
        "Net spare task",
    )
    .await;
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        json!({"title": "Net second task"}),
    )
    .await;
    let second_task = scalar(
        pool,
        "SELECT id FROM work_items WHERE title = $1",
        "Net second task",
    )
    .await;
    let mut projects = Vec::new();
    for key in ["SPARE", "EXTRA"] {
        ok(
            app,
            browser,
            "POST",
            "/api/v1/projects",
            json!({"organizationId": a.organization, "key": key, "name": format!("Net {key}")}),
        )
        .await;
        projects.push(
            scalar(pool, "SELECT id FROM projects WHERE project_key = $1", key)
                .await
                .to_string(),
        );
    }
    let mut messages = Vec::new();
    for text_value in ["Net spare one", "Net spare two"] {
        let sent = ok(
            app,
            browser,
            "POST",
            &format!("/api/v1/conversations/{}/messages", a.channel),
            json!({"content": text(text_value)}),
        )
        .await;
        messages.push(string_at(&sent, "/message/id"));
    }
    let file = upload(
        app,
        browser,
        &format!(
            "/api/v1/conversations/{}/files?name=net-spare.txt",
            a.channel
        ),
        "text/plain",
        b"Net spare".to_vec(),
    )
    .await;
    let mut uploads = Vec::new();
    for uri in [
        format!(
            "/api/v1/conversations/{}/files?name=net-tool.txt",
            a.channel
        ),
        format!(
            "/api/v1/projects/{project}/work-items/{}/files?name=net-tool.txt&for=comment",
            a.task
        ),
        format!(
            "/api/v1/conversations/{}/files?name=net-delete.txt",
            a.channel
        ),
    ] {
        uploads.push(upload(app, browser, &uri, "text/plain", b"Net unsent".to_vec()).await);
    }
    ok(
        app,
        browser,
        "POST",
        "/api/v1/later",
        json!({"note": "Net spare", "remindAt": soon, "organizationId": a.organization}),
    )
    .await;
    let later = scalar(
        pool,
        "SELECT id FROM saved_items WHERE note = $1",
        "Net spare",
    )
    .await;
    for text_value in ["Net spare one", "Net spare two"] {
        ok(
            app,
            browser,
            "POST",
            &format!("/api/v1/conversations/{}/scheduled-messages", a.channel),
            json!({"content": text(text_value), "sendAt": soon}),
        )
        .await;
    }
    let scheduled = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM scheduled_messages WHERE conversation_id = $1 AND id <> $2 ORDER BY created_at",
    )
    .bind(Uuid::parse_str(&a.channel).expect("channel"))
    .bind(ax.scheduled)
    .fetch_all(pool)
    .await
    .expect("spare scheduled messages");
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/organizations/{}/harnesses", a.organization),
        json!({
            "name": "Net spare runner",
            "transport": "runner",
            "runnerActorId": ax.agent,
            "manifest": {"description": "Net", "models": [{"id": "net-spare-model", "displayName": "Net"}]},
        }),
    )
    .await;
    let harness = scalar(
        pool,
        "SELECT id FROM custom_harnesses WHERE name = $1",
        "Net spare runner",
    )
    .await;
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/bug-report-links"),
        json!({"name": "Net spare"}),
    )
    .await;
    let bug_link = scalar(
        pool,
        "SELECT id FROM bug_report_links WHERE name = $1",
        "Net spare",
    )
    .await;
    ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/invitations"),
        json!({"role": "viewer", "expiresInHours": 24}),
    )
    .await;
    let project_invitation = scalar(
        pool,
        "SELECT id FROM project_invitations WHERE project_id = $1 ORDER BY created_at DESC LIMIT 1",
        project,
    )
    .await;
    let created = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/external-agents"),
        json!({
            "connectionName": "Net spare agent",
            "displayName": "Net spare agent",
            "handle": "netspare",
            "role": "member",
            "scopes": ["read", "write"],
        }),
    )
    .await;
    let agent_connection =
        Uuid::parse_str(&string_at(&created, "/connection/id")).expect("connection");
    let requester = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/external-agents"),
        json!({
            "connectionName": "Net requester",
            "displayName": "Net requester",
            "handle": "netrequester",
            "role": "member",
            "scopes": ["read", "write", "worker_request"],
        }),
    )
    .await;
    let requester_token = string_at(&requester, "/oneTimeCredential/token");

    // A repository with no runs, to archive (one with a run that hasn't
    // finished stays). It isn't prepared, so runs never pick it.
    let repository = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/repositories"),
        json!({
            "githubRepositoryId": 9002,
            "owner": "acme",
            "name": "net-spare",
            "cloneUrl": "https://github.com/acme/net-spare.git",
            "defaultBranch": "main",
            "trustAcknowledged": true,
        }),
    )
    .await;
    let repository = Uuid::parse_str(&string_at(&repository, "/id")).expect("repository");

    let mut comments = Vec::new();
    for text_value in ["Net spare comment one", "Net spare comment two"] {
        let created = ok(
            app,
            browser,
            "POST",
            &format!("/api/v1/projects/{project}/work-items/{}/comments", a.task),
            json!({"content": text(text_value)}),
        )
        .await;
        comments.push(Uuid::parse_str(&string_at(&created, "/comment/id")).expect("comment"));
    }
    let trashed = ok(
        app,
        browser,
        "POST",
        "/api/v1/whiteboards",
        json!({"title": "Net spare trashed", "organizationId": a.organization}),
    )
    .await;
    let trashed_whiteboard =
        Uuid::parse_str(&string_at(&trashed, "/whiteboard/id")).expect("trashed board");
    ok(
        app,
        browser,
        "PATCH",
        &format!("/api/v1/whiteboards/{trashed_whiteboard}"),
        json!({"deleted": true}),
    )
    .await;
    let profile = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/agent-profiles"),
        json!({"template": "reviewer", "name": "Net spare reviewer"}),
    )
    .await;
    let agent_profile = Uuid::parse_str(&string_at(&profile, "/id")).expect("agent profile");
    let account = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/provider-accounts"),
        json!({"adapterKind": "codex_app_server", "name": "Net spare account"}),
    )
    .await;
    let provider_account = Uuid::parse_str(&string_at(&account, "/id")).expect("account");

    // Two runs waiting on an approval, each in a thread of its own, made the
    // way `more_targets` makes its failed run: the owner's, and one on
    // rook's own profile, signed in the same way, whose approval the owner
    // answers through MCP (a run only works on its requester's profile).
    let rook = login_as(app, "rook-a").await;
    ok(
        app,
        &rook,
        "POST",
        &format!("/api/v1/projects/{project}/execution-profiles"),
        json!({}),
    )
    .await;
    let rook_profile = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE execution_profiles SET status = 'available', account_state = 'authenticated'
        WHERE project_id = $1 AND owner_actor_id = $2
        RETURNING id
        "#,
    )
    .bind(project)
    .bind(a.rook)
    .fetch_one(pool)
    .await
    .expect("rook's profile signed in");
    let worker = scalar(
        pool,
        "SELECT execution_worker_id FROM agent_runs WHERE id = $1",
        a.run,
    )
    .await;
    let mut runs = Vec::new();
    for (index, (requester, profile)) in [(ax.owner, a.profile), (a.rook, rook_profile)]
        .into_iter()
        .enumerate()
    {
        let thread = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
            VALUES ($1, $2, 'thread', $3::uuid, $4, $5)
            "#,
        )
        .bind(thread)
        .bind(project)
        .bind(&a.channel)
        .bind(a.repository)
        .bind(format!("Net approval thread {index}"))
        .execute(pool)
        .await
        .expect("approval thread");
        let run = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO agent_runs (
                id, conversation_id, execution_worker_id, requesting_actor_id,
                execution_profile_id, repository_id, state, prompt,
                authorization_snapshot, failure_reason, started_at, finished_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'failed', 'Net run', '{}'::jsonb,
                    'Net failure.', now(), now())
            "#,
        )
        .bind(run)
        .bind(thread)
        .bind(worker)
        .bind(requester)
        .bind(profile)
        .bind(a.repository)
        .execute(pool)
        .await
        .expect("run");
        runs.push(run);
    }
    sqlx::query(
        "UPDATE agent_runs SET state = 'blocked', failure_reason = NULL, finished_at = NULL WHERE id = ANY($1)",
    )
    .bind(&runs)
    .execute(pool)
    .await
    .expect("runs blocked");
    let mut approvals = Vec::new();
    for (run, requester) in [(runs[0], ax.owner), (runs[1], a.rook)] {
        let approval = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO agent_approvals (
                id, agent_run_id, requesting_actor_id, provider_request_id,
                kind, summary, options, eligible_role, human_required
            )
            VALUES ($1, $2, $3, $4, 'approval', 'Net approval',
                    '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
                    'any_authorized', false)
            "#,
        )
        .bind(approval)
        .bind(run)
        .bind(requester)
        .bind(format!("net-{approval}"))
        .execute(pool)
        .await
        .expect("approval");
        approvals.push(approval);
    }

    // Run control through MCP: a failed run to retry, in its own thread, and
    // a launch queued for next year. The run stopped through MCP is the
    // owner's approval run, after its approval is answered over HTTP: the
    // stop makes room under the owner's cap of five runs at once through
    // apps for the retry, which runs after it.
    let retry_thread = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
        VALUES ($1, $2, 'thread', $3::uuid, $4, 'Net run retry')
        "#,
    )
    .bind(retry_thread)
    .bind(project)
    .bind(&a.channel)
    .bind(a.repository)
    .execute(pool)
    .await
    .expect("retry thread");
    let retry_run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, failure_reason, started_at, finished_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'failed', 'Net run', '{}'::jsonb,
                'Net failure.', now(), now())
        "#,
    )
    .bind(retry_run)
    .bind(retry_thread)
    .bind(worker)
    .bind(ax.owner)
    .bind(a.profile)
    .bind(a.repository)
    .execute(pool)
    .await
    .expect("retry run");
    let queued_launch = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO launches (
            id, organization_id, project_id, work_item_id, requested_by_actor_id,
            source, start_when, start_at, position
        )
        SELECT $1, organization_id, id, $3, $4, 'button', 'at', now() + interval '1 year',
               (SELECT COALESCE(max(position), 0) + 1 FROM launches WHERE project_id = $2)
        FROM projects WHERE id = $2
        "#,
    )
    .bind(queued_launch)
    .bind(project)
    .bind(a.subtask)
    .bind(ax.owner)
    .execute(pool)
    .await
    .expect("queued launch");

    let mut suggestions = Vec::new();
    for (work_item, other) in [(a.task, task), (a.subtask, task)] {
        let suggestion = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO ai_suggestions (
                id, organization_id, project_id, work_item_id, kind,
                other_project_id, other_work_item_id, confidence, reason, sources,
                pair_key, origin, provider, model, prompt_id, prompt_version
            )
            VALUES ($1, $2, $3, $4, 'relates_to', $3, $5, 0.7, 'Net related', '[]'::jsonb,
                    $6, 'analyze', 'fake', 'rules', 'dependency_judge', 1)
            "#,
        )
        .bind(suggestion)
        .bind(a.organization)
        .bind(project)
        .bind(work_item)
        .bind(other)
        .bind(format!("net:{suggestion}"))
        .execute(pool)
        .await
        .expect("spare suggestion");
        suggestions.push(suggestion);
    }

    connect_app(app, browser, "Net spare app", "full").await;
    let grant = scalar(
        pool,
        r#"
        SELECT id FROM mcp_oauth_grants
        WHERE actor_id = $1 AND external_agent_connection_id IS NULL AND revoked_at IS NULL
        ORDER BY created_at DESC LIMIT 1
        "#,
        ax.owner,
    )
    .await;
    let (status, registration) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "client_name": "Net client",
                    "redirect_uris": [REDIRECT],
                    "token_endpoint_auth_method": "none",
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"],
                }))
                .expect("registration"),
            ))
            .expect("registration request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "OAuth client: {registration}");
    let oauth_client = string_at(&registration, "/client_id");

    let mut replies = Vec::new();
    for status in [
        "pending",
        "pending",
        "pending",
        "claimed",
        "pending",
        "cancelled",
    ] {
        replies.push(reply_request(app, pool, browser, ax.agent, status).await.0);
    }
    // The agent's own upload, in the group of the request it answers.
    let agent_session = initialize(app, &a.agent_token).await.expect("A agent MCP");
    let group = scalar(
        pool,
        "SELECT conversation_id FROM reply_requests WHERE id = $1",
        replies[1],
    )
    .await;
    let (_, uploaded) = call_tool(
        app,
        &a.agent_token,
        &agent_session,
        "file_upload",
        json!({"conversationId": group, "name": "net-reply.txt", "text": "Net reply file"}),
    )
    .await;
    let reply_upload = structured(&uploaded)["value"]["fileId"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(|| panic!("agent upload: {uploaded}"));

    let statuses = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "SELECT id, name, semantic_category, is_default FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("statuses")
    .into_iter()
    .map(|(id, name, category, is_default)| {
        json!({"id": id, "name": name, "semanticCategory": category, "isDefault": is_default})
    })
    .collect();
    let types = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, is_default FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("types")
    .into_iter()
    .map(|(id, name, is_default)| json!({"id": id, "name": name, "isDefault": is_default}))
    .collect();

    let role = scalar(
        pool,
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'net-spare-role', 'viewer', 'Net spare role',
                ARRAY['projects.view'])
        RETURNING id
        "#,
        a.organization,
    )
    .await;

    let view = ok(
        app,
        browser,
        "POST",
        &format!(
            "/api/v1/projects/{project}/collections/{}/views",
            a.collection
        ),
        json!({"name": "Net spare view"}),
    )
    .await;
    let collection_view = Uuid::parse_str(&string_at(&view, "/id")).expect("spare collection view");

    // Last, so it's the newest of A's sessions: revoked by A's own request.
    login_as(app, "owner").await;
    let session = newest_session(pool, ax.owner).await;

    Spares {
        role,
        channel,
        thread,
        task,
        second_task,
        projects: [projects[0].clone(), projects[1].clone()],
        messages: [messages[0].clone(), messages[1].clone()],
        file,
        tool_upload: uploads[0],
        tool_comment_upload: uploads[1],
        tool_delete_upload: uploads[2],
        reply_upload,
        later,
        scheduled: [scheduled[0], scheduled[1]],
        harness,
        bug_link,
        project_invitation,
        agent_connection,
        approvals: [approvals[0], approvals[1]],
        suggestions: [suggestions[0], suggestions[1]],
        run_stop: runs[0],
        run_retry: retry_run,
        launch: queued_launch,
        grant,
        oauth_client,
        reply_stop: replies[0],
        reply_claim: replies[1],
        reply_pass: replies[2],
        reply_release: replies[3],
        reply_stop_mcp: replies[4],
        reply_retry_mcp: replies[5],
        statuses,
        types,
        session,
        requester_token,
        repository,
        comments: [comments[0], comments[1]],
        trashed_whiteboard,
        agent_profile,
        provider_account,
        collection_view,
    }
}

/// Sends a setup request as `browser` and returns the body; it must work.
async fn ok(app: &axum::Router, browser: &Browser, method: &str, uri: &str, body: Value) -> Value {
    let key = format!("net-setup-{}", Uuid::now_v7().simple());
    let (status, value) = send(app, browser.request(method, uri, Some(&key), body)).await;
    assert!(
        status.is_success(),
        "setup {method} {uri}: {status} {value}"
    );
    value
}

async fn scalar<T>(pool: &PgPool, query: &str, bind: T) -> Uuid
where
    T: for<'q> sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send,
{
    sqlx::query_scalar::<_, Uuid>(query)
        .bind(bind)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("{query}: {error}"))
}

async fn version_now(pool: &PgPool, table: &str, row: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT version::bigint FROM {table} WHERE id = $1"
    ))
    .bind(row)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("version of {table} {row}: {error}"))
}

async fn newest_session(pool: &PgPool, actor: Uuid) -> Uuid {
    scalar(
        pool,
        r#"
        SELECT session.id FROM sessions session
        JOIN users person ON person.id = session.user_id
        WHERE person.actor_id = $1 AND session.revoked_at IS NULL
        ORDER BY session.created_at DESC LIMIT 1
        "#,
        actor,
    )
    .await
}

/// A message to the agent and its reply request for it, set to `status`
/// (`claimed`: held by the agent, generation 1), and the conversation. Each
/// is in a group of its own with the agent: a new message supersedes the
/// agent's other waiting requests in a conversation, and stopping one stops
/// those behind it.
async fn reply_request(
    app: &axum::Router,
    pool: &PgPool,
    browser: &Browser,
    agent: Uuid,
    status: &str,
) -> (Uuid, String) {
    let group = ok(
        app,
        browser,
        "POST",
        "/api/v1/direct/groups",
        json!({"name": format!("Net ask {}", Uuid::now_v7().simple()), "memberIds": [agent]}),
    )
    .await;
    let conversation = string_at(&group, "/conversation/id");
    let sent = ok(
        app,
        browser,
        "POST",
        &format!("/api/v1/conversations/{conversation}/messages"),
        json!({"content": text("Net, could you look?")}),
    )
    .await;
    let message = Uuid::parse_str(&string_at(&sent, "/message/id")).expect("message");
    let claimed = status == "claimed";
    let request = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO reply_requests (
            id, project_id, conversation_id, message_id, agent_actor_id, required, mentioned,
            status, generation, lease_expires_at
        )
        SELECT $1, conversation.project_id, conversation.id, $3, $4, false, false,
               $5, CASE WHEN $6 THEN 1 ELSE 0 END,
               CASE WHEN $6 THEN now() + interval '1 hour' END
        FROM conversations conversation WHERE conversation.id = $2
        ON CONFLICT (message_id, agent_actor_id) DO UPDATE
        SET required = false, status = EXCLUDED.status, generation = EXCLUDED.generation,
            lease_expires_at = EXCLUDED.lease_expires_at, reason = NULL
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::parse_str(&conversation).expect("group"))
    .bind(message)
    .bind(agent)
    .bind(status)
    .bind(claimed)
    .fetch_one(pool)
    .await
    .expect("reply request");
    (request, conversation)
}

// ---------------------------------------------------------------------------
// The digest of B's rows
// ---------------------------------------------------------------------------

/// Every row, in any table, whose JSON form mentions one of B's IDs, by
/// table. A new row that points at B shows up as well as a change.
async fn digest(pool: &PgPool, pattern: &str) -> BTreeMap<String, BTreeSet<String>> {
    let tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT table_name::text FROM information_schema.tables
        WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'
        ORDER BY 1
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("tables");
    let mut rows = BTreeMap::new();
    for table in tables {
        let found = sqlx::query_scalar::<_, String>(&format!(
            r#"SELECT listed.row_text FROM (SELECT row_to_json(row_value)::text AS row_text FROM "{table}" row_value) listed WHERE listed.row_text ~ $1"#
        ))
        .bind(pattern)
        .fetch_all(pool)
        .await
        .unwrap_or_else(|error| panic!("digest {table}: {error}"));
        if !found.is_empty() {
            rows.insert(table, found.into_iter().collect::<BTreeSet<_>>());
        }
    }
    rows
}

/// What changed between two digests, one line per row.
fn digest_changes(
    before: &BTreeMap<String, BTreeSet<String>>,
    after: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let empty = BTreeSet::new();
    let tables = before.keys().chain(after.keys()).collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for table in tables {
        let was = before.get(table).unwrap_or(&empty);
        let now = after.get(table).unwrap_or(&empty);
        for row in was.difference(now) {
            changes.push(format!("{table} lost or changed: {}", excerpt(row, 400)));
        }
        for row in now.difference(was) {
            changes.push(format!("{table} gained or changed: {}", excerpt(row, 400)));
        }
    }
    changes
}

fn excerpt(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    failures: Vec<String>,
    notes: Vec<String>,
    http_requests: usize,
    mcp_calls: usize,
}

#[tokio::test]
async fn one_workspace_never_writes_to_another() {
    // Server errors (what a 500 was) show in the output.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=error")
        .try_init();
    // GitHub configured, so its routes get as far as their access checks.
    let Some(fixture) = two_workspaces_with("write_isolation", true).await else {
        return;
    };
    let TwoWorkspaces {
        app, pool, a, b, ..
    } = &fixture;
    let ax = more_targets(app, pool, a).await;
    let bx = more_targets(app, pool, b).await;
    let sp = spares(app, pool, a, &ax).await;
    let b_ids = sweep_ids(pool, b.organization).await;
    let pattern = b_ids.iter().cloned().collect::<Vec<_>>().join("|");
    let before = digest(pool, &pattern).await;
    eprintln!(
        "B's digest: {} rows in {} tables",
        before.values().map(BTreeSet::len).sum::<usize>(),
        before.len()
    );
    for table in [
        "work_items",
        "messages",
        "project_labels",
        "webhook_subscriptions",
    ] {
        assert!(before.contains_key(table), "B's digest has no {table} rows");
    }

    let http = http_cases(a, &ax, &sp, b, &bx);
    let cased = http.iter().map(|case| case.route).collect::<BTreeSet<_>>();
    let listed = WRITE_ROUTES.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        cased, listed,
        "WRITE_ROUTES and http_cases must name the same routes"
    );
    assert_eq!(cased.len(), http.len(), "a route with two cases");
    let mcp = mcp_cases(a, &ax, &sp, b, &bx);
    let cased = mcp.iter().map(|case| case.tool).collect::<BTreeSet<_>>();
    let listed = MCP_WRITE_TOOLS.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(cased, listed, "MCP_WRITE_TOOLS and mcp_cases must agree");
    assert_eq!(cased.len(), mcp.len(), "a tool with two cases");

    let mut report = Report::default();
    let mut sender = Sender::new(app, pool, &a.browser, &ax.sessions);
    let app_session = initialize(app, &a.app_token).await.expect("A app MCP");
    let agent_session = initialize(app, &a.agent_token).await.expect("A agent MCP");
    let requester_session = initialize(app, &sp.requester_token)
        .await
        .expect("A requester MCP");
    let mcp_callers = McpCallers {
        app: (&a.app_token, &app_session),
        agent: (&a.agent_token, &agent_session),
        requester: (&sp.requester_token, &requester_session),
    };

    for case in &http {
        let stops_at = match &case.own {
            Own::Reaches(_, code, _) => Some(*code),
            _ => None,
        };
        for (call, expect) in &case.foreign {
            check_foreign_http(
                &mut sender,
                case.route,
                call,
                *expect,
                stops_at,
                &b_ids,
                &mut report,
            )
            .await;
        }
    }
    for case in &mcp {
        for (arguments, expect) in &case.foreign {
            check_foreign_mcp(
                app,
                pool,
                &mcp_callers,
                case,
                arguments,
                *expect,
                &b_ids,
                &mut report,
            )
            .await;
        }
    }
    let after_foreign = digest(pool, &pattern).await;
    for change in digest_changes(&before, &after_foreign) {
        report
            .failures
            .push(format!("B CHANGED by A's refused requests: {change}"));
    }

    // A's own requests: first everything that keeps the world as it is,
    // then what ends or deletes things, the routes' last (they revoke and
    // archive what the tools still use).
    for case in http.iter().filter(|case| !case.last) {
        own_http(&mut sender, case, &b_ids, &mut report).await;
    }
    for last in [false, true] {
        for case in mcp.iter().filter(|case| case.last == last) {
            check_own_mcp(app, pool, &mcp_callers, case, &b_ids, &mut report).await;
        }
    }
    for case in http.iter().filter(|case| case.last) {
        own_http(&mut sender, case, &b_ids, &mut report).await;
    }
    let after_own = digest(pool, &pattern).await;
    for change in digest_changes(&after_foreign, &after_own) {
        report
            .failures
            .push(format!("B CHANGED by A's own requests: {change}"));
    }

    for note in &report.notes {
        eprintln!("note: {note}");
    }
    eprintln!(
        "checked {} HTTP requests and {} MCP calls",
        report.http_requests, report.mcp_calls
    );
    let failures = report.failures.clone();
    fixture.finish().await;
    assert!(
        failures.is_empty(),
        "{} write isolation failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// AWD-71: answering an approval on a run the database won't admit again (a
/// blocked run on a provider account that isn't its requester's, as this net
/// once made) is a 409 that says why, over HTTP and through MCP, never a 500
/// or a "retry later". The approval stays open, so it can still be denied.
#[tokio::test]
async fn answering_an_approval_on_a_run_that_cant_be_admitted_is_a_conflict() {
    let Some(fixture) = two_workspaces_with("approval_admission", false).await else {
        return;
    };
    let TwoWorkspaces { app, pool, a, .. } = &fixture;
    // The owner's profile, on a run rook asked for: the database refuses
    // it when the run goes back to working.
    let owner = scalar(
        pool,
        "SELECT owner_actor_id FROM execution_profiles WHERE id = $1",
        a.profile,
    )
    .await;
    assert_ne!(owner, a.rook, "the profile must be someone else's");
    let worker = scalar(
        pool,
        "SELECT execution_worker_id FROM agent_runs WHERE id = $1",
        a.run,
    )
    .await;
    let project = Uuid::parse_str(&a.project).expect("project");
    let mut approvals = Vec::new();
    for index in 0..2 {
        let thread = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
            VALUES ($1, $2, 'thread', $3::uuid, $4, $5)
            "#,
        )
        .bind(thread)
        .bind(project)
        .bind(&a.channel)
        .bind(a.repository)
        .bind(format!("Unadmitted run {index}"))
        .execute(pool)
        .await
        .expect("thread");
        let run = Uuid::now_v7();
        // Made failed (no admission check), then blocked, as `spares` does.
        sqlx::query(
            r#"
            INSERT INTO agent_runs (
                id, conversation_id, execution_worker_id, requesting_actor_id,
                execution_profile_id, repository_id, state, prompt,
                authorization_snapshot, failure_reason, started_at, finished_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'failed', 'Unadmitted run', '{}'::jsonb,
                    'Net failure.', now(), now())
            "#,
        )
        .bind(run)
        .bind(thread)
        .bind(worker)
        .bind(a.rook)
        .bind(a.profile)
        .bind(a.repository)
        .execute(pool)
        .await
        .expect("run");
        sqlx::query(
            "UPDATE agent_runs SET state = 'blocked', failure_reason = NULL, finished_at = NULL WHERE id = $1",
        )
        .bind(run)
        .execute(pool)
        .await
        .expect("run blocked");
        let approval = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO agent_approvals (
                id, agent_run_id, requesting_actor_id, provider_request_id,
                kind, summary, options, eligible_role, human_required
            )
            VALUES ($1, $2, $3, $4, 'approval', 'Unadmitted approval',
                    '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
                    'any_authorized', false)
            "#,
        )
        .bind(approval)
        .bind(run)
        .bind(a.rook)
        .bind(format!("unadmitted-{approval}"))
        .execute(pool)
        .await
        .expect("approval");
        approvals.push((approval, run));
    }
    let reason = "This run uses a provider account that isn't its requester's";

    // Over HTTP, as the owner.
    let (approval, run) = approvals[0];
    let (status, body) = send(
        app,
        a.browser.request(
            "POST",
            &format!("/api/v1/agent-approvals/{approval}/respond"),
            Some("unadmitted-http"),
            json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "run_not_admitted", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with(reason)),
        "{body}"
    );
    let (approval_status, run_state) = sqlx::query_as::<_, (String, String)>(
        "SELECT approval.status, run.state FROM agent_approvals approval \
         JOIN agent_runs run ON run.id = approval.agent_run_id WHERE approval.id = $1",
    )
    .bind(approval)
    .fetch_one(pool)
    .await
    .expect("approval after");
    assert_eq!(
        (approval_status.as_str(), run_state.as_str()),
        ("pending", "blocked"),
        "nothing changes when the run can't carry on ({run})"
    );
    // Denying still works: an interrupted run needs no admission.
    let (status, body) = send(
        app,
        a.browser.request(
            "POST",
            &format!("/api/v1/agent-approvals/{approval}/respond"),
            Some("unadmitted-http-deny"),
            json!({"decision": "deny"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Through MCP, as the owner's connected app.
    let (approval, _) = approvals[1];
    let session = initialize(app, &a.app_token).await.expect("app MCP");
    let version = version_now(pool, "agent_approvals", approval).await;
    let (_, body) = call_tool(
        app,
        &a.app_token,
        &session,
        "approval_respond",
        json!({
            "approvalId": approval,
            "expectedVersion": version,
            "decision": "approve",
            "clientRequestId": "unadmitted-mcp",
        }),
    )
    .await;
    let result = structured(&body);
    assert_eq!(result["outcome"], "conflict", "{body}");
    assert_eq!(result["retryCategory"], "do_not_retry", "{body}");
    assert_eq!(result["current"]["status"], "pending", "{body}");
    assert!(
        result["reason"]
            .as_str()
            .is_some_and(|message| message.starts_with(reason)),
        "{body}"
    );
    fixture.finish().await;
}

async fn own_http(
    sender: &mut Sender<'_>,
    case: &HttpCase,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    match &case.own {
        Own::Run(call) => check_own_http(sender, case.route, call, None, b_ids, report).await,
        Own::Reaches(call, code, reason) => {
            check_own_http(
                sender,
                case.route,
                call,
                Some((code, reason)),
                b_ids,
                report,
            )
            .await;
        }
        Own::Skip(reason) => report
            .notes
            .push(format!("{} has no own request: {reason}", case.route)),
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// Sends A's browser requests, spreading them over A's sessions so the
/// per-session write limit never answers instead of the route.
struct Sender<'a> {
    app: &'a axum::Router,
    pool: &'a PgPool,
    browsers: Vec<&'a Browser>,
    sent: usize,
}

impl<'a> Sender<'a> {
    fn new(
        app: &'a axum::Router,
        pool: &'a PgPool,
        first: &'a Browser,
        more: &'a [Browser],
    ) -> Self {
        let mut browsers = vec![first];
        browsers.extend(more.iter());
        Self {
            app,
            pool,
            browsers,
            sent: 0,
        }
    }

    async fn send(&mut self, call: &Call) -> (StatusCode, String) {
        let mut builder = Request::builder().method(call.method).uri(&call.uri);
        let mut csrf = String::new();
        if let Some(token) = &call.bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        } else {
            let browser = self.browsers[self.sent % self.browsers.len()];
            self.sent += 1;
            builder = builder
                .header("origin", ORIGIN)
                .header("cookie", &browser.cookie)
                .header("x-csrf-token", &browser.csrf)
                .header(
                    "idempotency-key",
                    format!("net-{}", Uuid::now_v7().simple()),
                );
            csrf.clone_from(&browser.csrf);
        }
        let request = match &call.payload {
            Payload::Json(value) => {
                let value = resolve(self.pool, value.clone()).await;
                builder
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&value).expect("body")))
            }
            Payload::Bytes(content_type, bytes) => builder
                .header(header::CONTENT_TYPE, *content_type)
                .body(Body::from(bytes.clone())),
            Payload::Form(fields) => {
                let fields = fields
                    .iter()
                    .map(|(key, value)| (*key, if value == "$csrf" { &csrf } else { value }))
                    .collect::<Vec<_>>();
                builder
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        serde_urlencoded::to_string(fields).expect("form"),
                    ))
            }
        }
        .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 8_000_000)
            .await
            .expect("body");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Replaces `current(...)` markers with the column's value now.
async fn resolve(pool: &PgPool, value: Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("$current:") => {
            let mut parts = text["$current:".len()..].splitn(4, ':');
            let table = parts.next().expect("table");
            let column = parts.next().expect("column");
            let default = parts
                .next()
                .and_then(|value| value.parse::<i64>().ok())
                .expect("default");
            let keys = parts
                .next()
                .expect("keys")
                .split(',')
                .map(|pair| {
                    let (key, value) = pair.split_once('=').expect("key=value");
                    (key, Uuid::parse_str(value).expect("key value"))
                })
                .collect::<Vec<_>>();
            let conditions = keys
                .iter()
                .enumerate()
                .map(|(index, (key, _))| format!("{key} = ${}", index + 1))
                .collect::<Vec<_>>()
                .join(" AND ");
            let query = format!("SELECT {column}::bigint FROM {table} WHERE {conditions}");
            let mut select = sqlx::query_scalar::<_, i64>(&query);
            for (_, value) in &keys {
                select = select.bind(*value);
            }
            let found = select
                .fetch_optional(pool)
                .await
                .unwrap_or_else(|error| panic!("{query}: {error}"));
            json!(found.unwrap_or(default))
        }
        Value::Array(items) => {
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(Box::pin(resolve(pool, item)).await);
            }
            Value::Array(resolved)
        }
        Value::Object(fields) => {
            let mut resolved = serde_json::Map::with_capacity(fields.len());
            for (key, item) in fields {
                resolved.insert(key, Box::pin(resolve(pool, item)).await);
            }
            Value::Object(resolved)
        }
        other => other,
    }
}

/// The error's code: the API's `error.code`, OAuth's `error`, or the public
/// API's tool `outcome`.
fn error_code(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str))
        .or_else(|| value.get("outcome").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

fn describe(call: &Call) -> String {
    match &call.payload {
        Payload::Json(value) => format!("{} {} {value}", call.method, call.uri),
        Payload::Bytes(content_type, bytes) => {
            format!(
                "{} {} ({} bytes of {content_type})",
                call.method,
                call.uri,
                bytes.len()
            )
        }
        Payload::Form(fields) => format!("{} {} {fields:?}", call.method, call.uri),
    }
}

/// `stops_at`: where A's own request stops (see [`Own::Reaches`]). A
/// request naming B's that stops there too got as far as A's own did, so it
/// wasn't refused.
async fn check_foreign_http(
    sender: &mut Sender<'_>,
    route: &str,
    call: &Call,
    expect: Expect,
    stops_at: Option<&str>,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    report.http_requests += 1;
    let (status, body) = sender.send(call).await;
    let code = error_code(&body);
    let what = describe(call);
    // Only a refusal of the resource counts: a 403 for a bad origin or CSRF
    // token, or the API's fallback 404 for an unknown path, would pass for
    // one otherwise.
    let refused = matches!(
        (status, code.as_str()),
        (
            StatusCode::FORBIDDEN,
            "forbidden" | "access_denied" | "permission_denied"
        ) | (StatusCode::NOT_FOUND, "not_found")
    );
    let accepted = match expect {
        Expect::Refused => refused,
        Expect::Rejected => {
            refused
                || matches!(
                    status,
                    StatusCode::BAD_REQUEST
                        | StatusCode::CONFLICT
                        | StatusCode::UNPROCESSABLE_ENTITY
                )
        }
        Expect::NoEffect => true,
    } && (expect == Expect::NoEffect || stops_at != Some(code.as_str()));
    if !accepted {
        report.failures.push(format!(
            "HOLE? {route}: A {what} -> {status} {code} (expected {}): {}",
            match expect {
                Expect::Refused => "403 forbidden or 404 not_found",
                _ => "a 4xx refusal",
            },
            excerpt(&body, 300)
        ));
    }
    leak(report, &format!("{route}: A {what}"), &what, &body, b_ids);
}

async fn check_own_http(
    sender: &mut Sender<'_>,
    route: &str,
    call: &Call,
    stops_at: Option<(&str, &str)>,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    report.http_requests += 1;
    let (status, body) = sender.send(call).await;
    let what = describe(call);
    let code = error_code(&body);
    match stops_at {
        None if !status.is_success() => report.failures.push(format!(
            "OWN {route}: A {what} -> {status} {code} (A's own request must succeed, or the \
             refusals of B's prove nothing): {}",
            excerpt(&body, 300)
        )),
        Some((expected, reason)) if status.is_success() || code != expected => {
            report.failures.push(format!(
                "OWN {route}: A {what} -> {status} {code} (A's own request should get past \
                 the access checks and stop with {expected}, as {reason}): {}",
                excerpt(&body, 300)
            ));
        }
        Some((expected, reason)) => report.notes.push(format!(
            "{route}: A's own request stops with {expected}, as {reason}"
        )),
        None => {}
    }
    leak(
        report,
        &format!("{route}: A's own {what}"),
        &what,
        &body,
        b_ids,
    );
}

/// A failure if `body` carries B's marker or B IDs the request didn't name.
fn leak(report: &mut Report, what: &str, input: &str, body: &str, b_ids: &HashSet<String>) {
    let input = input.to_ascii_lowercase();
    let found = leaked(body, b_ids)
        .into_iter()
        .filter(|item| {
            item.strip_prefix("id ")
                .is_none_or(|id| !input.contains(id))
        })
        .collect::<Vec<_>>();
    if !found.is_empty() {
        report.failures.push(format!(
            "LEAK {what}: carries B data {found:?}\n    body: {}",
            excerpt(body, 600)
        ));
    }
}

/// The OAuth consent, connecting as A's agent; B's agent must be refused.
fn consent_case(route: &'static str, client: &str, a_agent: Uuid, b_agent: Uuid) -> HttpCase {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let form = |agent: Uuid| {
        Payload::Form(vec![
            ("response_type", "code".to_owned()),
            ("client_id", client.to_owned()),
            ("redirect_uri", REDIRECT.to_owned()),
            ("scope", "read write".to_owned()),
            ("state", "net-state".to_owned()),
            ("code_challenge", challenge.clone()),
            ("code_challenge_method", "S256".to_owned()),
            ("connect_as", format!("agent:{agent}")),
            ("csrf_token", "$csrf".to_owned()),
            ("decision", "approve".to_owned()),
        ])
    };
    let (method, path) = route.split_once(' ').expect("METHOD /path");
    HttpCase {
        route,
        own: Own::Run(Call::new(method_of(route), path.to_owned(), form(a_agent))),
        foreign: vec![(
            Call::new(method, path.to_owned(), form(b_agent)),
            Expect::Refused,
        )],
        last: false,
    }
}

#[allow(clippy::too_many_lines)]
fn http_cases(a: &World, ax: &Targets, sp: &Spares, b: &World, bx: &Targets) -> Vec<HttpCase> {
    let project = || id(&a.project, &b.project);
    let organization = || id(a.organization, b.organization);
    let task = || id(a.task, b.task);
    let channel = || id(&a.channel, &b.channel);
    let soon = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let far = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    // Which workspace a request's first path value belongs to.
    let is_a = |values: &[String], a_value: &str| values[0] == a_value;
    let secret = || P::Along("launch_key".to_owned(), format!("{MARKER}_key"));
    let harness_manifest =
        json!({"description": "Net", "models": [{"id": "net-runner-model", "displayName": "Net"}]});
    let device = aws_lc_rs::agreement::PrivateKey::generate(&aws_lc_rs::agreement::ECDH_P256)
        .expect("device key");
    let device_public = device.compute_public_key().expect("device public key");
    let push_keys = json!({
        "p256dh": URL_SAFE_NO_PAD.encode(device_public.as_ref()),
        "auth": URL_SAFE_NO_PAD.encode([9_u8; 16]),
    });
    let workflow = |statuses: &[Value], types: &[Value]| {
        json!({
            "statuses": statuses,
            "types": types,
            "transitions": "all",
        })
    };
    // A's statuses and types, each with one more: new in A's own request,
    // and B's in the others, which are then valid but for B's ID.
    let added_status =
        json!({"name": "Net added status", "semanticCategory": "review", "isDefault": false});
    let added_type = json!({"name": "Net added type", "isDefault": false});
    let plus = |entries: &[Value], entry: Value| {
        let mut entries = entries.to_vec();
        entries.push(entry);
        entries
    };
    let a_workflow = workflow(
        &plus(&sp.statuses, added_status.clone()),
        &plus(&sp.types, added_type.clone()),
    );
    let b_status_workflow = workflow(
        &plus(&sp.statuses, with(&added_status, "id", json!(bx.status))),
        &plus(&sp.types, added_type.clone()),
    );
    let b_type_workflow = workflow(
        &plus(&sp.statuses, added_status),
        &plus(&sp.types, with(&added_type, "id", json!(bx.task_type))),
    );
    let workflow_version = |project: &str| {
        current(
            "project_task_settings",
            "workflow_version",
            &[("project_id", project.to_owned())],
            1,
        )
    };
    let navigation_version = |project: &str| {
        current(
            "project_navigation_states",
            "version",
            &[
                ("project_id", project.to_owned()),
                ("actor_id", ax.owner.to_string()),
            ],
            0,
        )
    };
    let public = |tool: &str, arguments: Value| Call {
        method: "POST",
        uri: format!("/api/public/v1/{tool}"),
        payload: Payload::Json(arguments),
        bearer: Some(a.app_token.clone()),
    };

    vec![
        // The public API: the same tools as MCP, by URL.
        HttpCase {
            route: "POST /api/public/v1/{tool}",
            own: Own::Run(public(
                "task_comment",
                json!({"taskId": a.task, "text": "Net public comment", "clientRequestId": "net-public-own"}),
            )),
            foreign: vec![
                (
                    public(
                        "task_comment",
                        json!({"taskId": b.task, "text": "Net public comment", "clientRequestId": "net-public-b"}),
                    ),
                    Expect::Refused,
                ),
                (
                    public(
                        "label_write",
                        json!({"projectId": b.project, "name": "Net public label"}),
                    ),
                    Expect::Refused,
                ),
            ],
            last: false,
        },
        // Account
        body_case(
            "POST /api/v1/account/api-tokens",
            &[],
            json!({"projectId": a.project, "name": "Net token", "scopes": ["read"], "expiresInDays": 30}),
            vec![json!({"projectId": b.project, "name": "Net token", "scopes": ["read"], "expiresInDays": 30})],
        ),
        path_case(
            "POST /api/v1/account/api-tokens/{credential_id}/revoke",
            &[id(ax.api_token, bx.api_token)],
            versioned("mcp_client_credentials", 0, json!({})),
        ),
        path_case(
            "PATCH /api/v1/account/connected-apps/{id}",
            &[id(ax.grant, bx.grant)],
            json_body(json!({"showVia": true})),
        ),
        path_case(
            "POST /api/v1/account/connected-apps/{id}/revoke",
            &[id(sp.grant, bx.grant)],
            empty(),
        ),
        body_case(
            "PUT /api/v1/account/default-workspace",
            &[],
            json!({"organizationId": a.organization}),
            vec![json!({"organizationId": b.organization})],
        ),
        path_case(
            "POST /api/v1/account/sessions/{session_id}/revoke",
            &[id(sp.session, bx.session)],
            empty(),
        ),
        // Runs and approvals
        path_case(
            "POST /api/v1/agent-approvals/{id}/respond",
            &[id(sp.approvals[0], b.approval)],
            json_body(json!({"decision": "approve"})),
        ),
        path_case(
            "POST /api/v1/agent-runs/{id}/acknowledge-failure",
            &[id(ax.failed_run, bx.failed_run)],
            |values| {
                Payload::Json(json!({
                    "eventId": if is_a(values, &ax.failed_run.to_string()) { ax.failed_event } else { bx.failed_event }
                }))
            },
        )
        .rejecting(vec![json!({"eventId": bx.failed_event})]),
        path_case(
            "POST /api/v1/agent-runs/{id}/interrupt",
            &[id(a.run, b.run)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/agent-runs/{id}/retry",
            &[id(ax.failed_run, bx.failed_run)],
            empty(),
        ),
        // Bug report links
        path_case(
            "DELETE /api/v1/bug-report-links/{link_id}",
            &[id(sp.bug_link, bx.bug_link)],
            empty(),
        ),
        path_case(
            "PATCH /api/v1/bug-report-links/{link_id}",
            &[id(ax.bug_link, bx.bug_link)],
            json_body(json!({"name": "Net game"})),
        ),
        path_case(
            "POST /api/v1/bug-report-links/{link_id}/replace",
            &[id(ax.bug_link, bx.bug_link)],
            empty(),
        ),
        // Channels and threads
        path_case(
            "POST /api/v1/channels/{id}/archive",
            &[id(&sp.channel, &b.channel)],
            versioned("conversations", 0, json!({})),
        ),
        path_case(
            "POST /api/v1/channels/{id}/restore",
            &[id(&sp.channel, &b.channel)],
            versioned("conversations", 0, json!({})),
        ),
        path_case(
            "POST /api/v1/channels/{id}/messages",
            &[channel()],
            json_body(json!({"content": text("Net message"), "attachments": [ax.channel_upload]})),
        )
        .rejecting(vec![
            json!({"content": text("Net message"), "attachments": [bx.channel_upload]}),
        ]),
        path_case(
            "POST /api/v1/channels/{id}/threads",
            &[channel()],
            json_body(json!({"name": "Net thread"})),
        )
        .rejecting(vec![json!({"name": "Net thread", "originMessageId": b.message})]),
        path_case(
            "PATCH /api/v1/threads/{id}",
            &[id(&a.thread, &b.thread)],
            json_body(json!({"name": "Launch thread"})),
        ),
        path_case(
            "POST /api/v1/threads/{id}/archive",
            &[id(&sp.thread, &b.thread)],
            versioned("conversations", 0, json!({})),
        ),
        path_case(
            "POST /api/v1/threads/{id}/restore",
            &[id(&sp.thread, &b.thread)],
            versioned("conversations", 0, json!({})),
        ),
        path_case(
            "POST /api/v1/threads/{id}/messages",
            &[id(&a.thread, &b.thread)],
            json_body(json!({"content": text("Net reply"), "attachments": [ax.thread_upload]})),
        )
        .rejecting(vec![
            json!({"content": text("Net reply"), "attachments": [bx.thread_upload]}),
        ]),
        path_case(
            "POST /api/v1/threads/{id}/agent-runs",
            &[id(ax.run_threads[0], bx.run_threads[0])],
            json_body(json!({"prompt": "Net run"})),
        )
        .rejecting(vec![
            json!({"prompt": "Net run", "repositoryId": b.repository}),
            json!({"prompt": "Net run", "providerAccountId": b.provider_account}),
            json!({"prompt": "Net run", "agentProfileId": bx.agent_profile}),
        ]),
        // Conversations
        path_case(
            "POST /api/v1/conversations/{id}/ai/summary",
            &[channel()],
            empty(),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/continue",
            &[id(&a.group, &b.group)],
            empty(),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/files",
            &[channel()],
            |_| Payload::Bytes("text/plain", b"Net file".to_vec()),
        )
        .query("?name=net.txt"),
        path_case(
            "POST /api/v1/conversations/{id}/mark-seen",
            &[channel()],
            json_body(json!({"highestSeenSequence": 1})),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/mark-unread",
            &[channel()],
            |values| {
                Payload::Json(json!({
                    "messageId": if is_a(values, &a.channel) { &a.message } else { &b.message }
                }))
            },
        )
        .rejecting(vec![json!({"messageId": b.message})]),
        path_case(
            "POST /api/v1/conversations/{id}/members",
            &[id(&a.group, &b.group)],
            |values| {
                Payload::Json(json!({
                    "actorId": if is_a(values, &a.group) { ax.agent } else { bx.agent }
                }))
            },
        )
        .rejecting(vec![json!({"actorId": bx.agent})]),
        path_case(
            "POST /api/v1/conversations/{id}/members/{actor_id}/remove",
            &[id(&a.group, &b.group), removed(ax.wren, bx.wren)],
            empty(),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/messages",
            &[id(&a.dm, &b.dm)],
            json_body(json!({"content": text("Net DM"), "attachments": [ax.dm_upload]})),
        )
        .rejecting(vec![
            json!({"content": text("Net DM"), "attachments": [bx.dm_upload]}),
        ])
        .and(vec![
            (
                Call::new(
                    "POST",
                    format!("/api/v1/conversations/{}/messages", b.channel),
                    Payload::Json(json!({"content": text("Net")})),
                ),
                Expect::Refused,
            ),
            (
                Call::new(
                    "POST",
                    format!("/api/v1/conversations/{}/messages", b.group),
                    Payload::Json(json!({"content": text("Net")})),
                ),
                Expect::Refused,
            ),
        ]),
        path_case(
            "PATCH /api/v1/conversations/{id}/messages/{message_id}",
            &[channel(), id(&a.message, &b.message)],
            json_body(json!({"content": text("Launch message, edited")})),
        ),
        path_case(
            "DELETE /api/v1/conversations/{id}/messages/{message_id}",
            &[channel(), id(&sp.messages[0], &b.message)],
            empty(),
        ),
        path_case(
            "DELETE /api/v1/conversations/{id}/messages/{message_id}/link-previews/{preview_id}",
            &[
                channel(),
                id(&a.file_message, &b.file_message),
                id(a.link_preview, b.link_preview),
            ],
            empty(),
        ),
        path_case(
            "PUT /api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
            &[channel(), id(&a.message, &b.message), same("thumbsup")],
            empty(),
        ),
        path_case(
            "DELETE /api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
            &[channel(), id(&a.message, &b.message), same("thumbsup")],
            empty(),
        ),
        path_case(
            "PUT /api/v1/conversations/{id}/notification-level",
            &[channel()],
            json_body(json!({"level": "all"})),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/pins",
            &[channel()],
            |values| {
                Payload::Json(json!({
                    "kind": "message",
                    "targetId": if is_a(values, &a.channel) { &a.file_message } else { &b.file_message }
                }))
            },
        )
        .rejecting(vec![
            json!({"kind": "message", "targetId": b.message}),
            json!({"kind": "page", "targetId": b.page}),
            json!({"kind": "decision", "targetId": bx.decision}),
        ]),
        path_case(
            "DELETE /api/v1/conversations/{id}/pins/{kind}/{target_id}",
            &[channel(), same("message"), removed(&a.message, &b.message)],
            empty(),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/scheduled-messages",
            &[channel()],
            json_body(json!({"content": text("Net later"), "sendAt": soon})),
        ),
        path_case(
            "POST /api/v1/conversations/{id}/threads",
            &[channel()],
            json_body(json!({"name": "Net thread two"})),
        )
        .rejecting(vec![json!({"name": "Net thread two", "originMessageId": b.message})]),
        path_case(
            "POST /api/v1/messages/{id}/forward",
            &[id(&a.message, &b.message)],
            json_body(json!({"conversationId": a.dm, "text": "Net forward"})),
        )
        .rejecting(vec![json!({"conversationId": b.channel}), json!({"conversationId": b.dm})]),
        // Direct messages and groups
        body_case(
            "POST /api/v1/direct/dms",
            &[],
            json!({"actorId": a.rook}),
            vec![json!({"actorId": b.rook})],
        ),
        body_case(
            "POST /api/v1/direct/groups",
            &[],
            json!({"name": "Net group", "memberIds": [a.rook, ax.wren]}),
            vec![
                json!({"name": "Net group", "memberIds": [b.rook, bx.wren]}),
                json!({"name": "Net group", "memberIds": [a.rook, bx.wren]}),
            ],
        ),
        // Files
        path_case(
            "DELETE /api/v1/files/{id}",
            &[id(sp.file, bx.channel_upload)],
            empty(),
        )
        // And a task's own file, which anyone who may edit the task removes.
        // A message's files go with the message, for anyone.
        .and(vec![(
            Call::new(
                "DELETE",
                format!("/api/v1/files/{}", b.task_file),
                Payload::Json(json!({})),
            ),
            Expect::Refused,
        )]),
        // Inbox, later and scheduled messages
        body_case(
            "POST /api/v1/inbox/update",
            &[],
            json!({"action": "read", "ids": [ax.notification]}),
            Vec::new(),
        )
        // Only ever the caller's own notifications: B's project and B's
        // notification are filters that match none of them.
        .no_effect(vec![
            json!({"action": "read", "all": true, "projectId": b.project}),
            json!({"action": "read", "ids": [bx.notification]}),
        ]),
        body_case(
            "POST /api/v1/later",
            &[],
            json!({"messageId": a.dm_message, "note": "Net"}),
            vec![
                json!({"messageId": b.message}),
                json!({"workItemId": b.task}),
                json!({"note": "Net", "remindAt": soon, "organizationId": b.organization}),
            ],
        ),
        path_case(
            "PATCH /api/v1/later/{id}",
            &[id(ax.later, bx.later)],
            json_body(json!({"note": "Net note"})),
        ),
        path_case(
            "DELETE /api/v1/later/{id}",
            &[id(sp.later, bx.later)],
            empty(),
        ),
        path_case(
            "PATCH /api/v1/scheduled-messages/{id}",
            &[id(ax.scheduled, bx.scheduled)],
            json_body(json!({"content": text("Net later, edited")})),
        ),
        path_case(
            "DELETE /api/v1/scheduled-messages/{id}",
            &[id(ax.scheduled, bx.scheduled)],
            empty(),
        ),
        path_case(
            "POST /api/v1/scheduled-messages/{id}/send-now",
            &[id(sp.scheduled[0], bx.scheduled)],
            empty(),
        ),
        // Notification settings
        body_case(
            "PUT /api/v1/notification-settings/channels",
            &[],
            json!({"organizationId": a.organization, "group": "runs", "channel": "push", "enabled": false}),
            vec![json!({"organizationId": b.organization, "group": "runs", "channel": "push", "enabled": false})],
        ),
        body_case(
            "PUT /api/v1/notification-settings/mutes",
            &[],
            json!({"projectId": a.project, "group": "runs", "muted": false}),
            vec![json!({"projectId": b.project, "group": "runs", "muted": false})],
        ),
        // The workspace
        path_case(
            "PATCH /api/v1/organizations/{organization_id}",
            &[organization()],
            versioned("organizations", 0, json!({"name": "Acme net"})),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/ai/test",
            &[organization()],
            empty(),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/harnesses",
            &[organization()],
            json_body(json!({
                "name": "Net runner",
                "transport": "runner",
                "runnerActorId": ax.agent,
                "manifest": harness_manifest,
            })),
        )
        .rejecting(vec![json!({
            "name": "Net runner b",
            "transport": "runner",
            "runnerActorId": bx.agent,
            "manifest": harness_manifest,
        })]),
        path_case(
            "PATCH /api/v1/organizations/{organization_id}/harnesses/{harness_id}",
            &[organization(), id(ax.harness, bx.harness)],
            |values| {
                Payload::Json(json!({
                    "version": version("custom_harnesses", &values[1]),
                    "name": "Net runner renamed",
                }))
            },
        )
        .rejecting(vec![json!({
            "version": version("custom_harnesses", ax.harness),
            "runnerActorId": bx.agent,
        })]),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/harnesses/{harness_id}",
            &[organization(), id(sp.harness, bx.harness)],
            empty(),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/invitations",
            &[organization()],
            json_body(json!({"role": "member", "expiresInHours": 24})),
        ),
        // Layouts: a saved preset is the workspace's own, so B's is refused
        // wherever A's would do.
        path_case(
            "PUT /api/v1/organizations/{organization_id}/layout",
            &[organization()],
            |values| {
                let preset = if is_a(values, &a.organization.to_string()) {
                    ax.layout_preset
                } else {
                    bx.layout_preset
                };
                Payload::Json(json!({"layout": {"preset": preset}}))
            },
        )
        .rejecting(vec![json!({"layout": {"preset": bx.layout_preset}})]),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/layout/mine",
            &[organization()],
            json_body(json!({"hidden": [], "showHidden": true})),
        ),
        // Modules: switching one on that is on changes nothing in A, so the
        // rest of the run keeps every module.
        path_case(
            "PUT /api/v1/organizations/{organization_id}/modules",
            &[organization()],
            json_body(json!({"switches": {"whiteboards": true}})),
        ),
        // Workspace templates (AWD-76): Everything changes nothing in A, whose
        // modules are all on. B's saved template and project are refused.
        path_case(
            "POST /api/v1/organizations/{organization_id}/workspace-templates",
            &[organization()],
            |values| {
                let project = if is_a(values, &a.organization.to_string()) {
                    a.project.clone()
                } else {
                    b.project.clone()
                };
                Payload::Json(json!({"name": "Net saved", "projectId": project}))
            },
        )
        .rejecting(vec![json!({"name": "Net saved b", "projectId": b.project})]),
        path_case(
            "POST /api/v1/organizations/{organization_id}/workspace-templates/apply",
            &[organization()],
            json_body(json!({"template": "everything"})),
        )
        .rejecting(vec![json!({"template": bx.workspace_template})]),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/workspace-templates/{template_id}",
            &[organization(), id(ax.workspace_template, bx.workspace_template)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/organizations/{organization_id}/layout/presets",
            &[organization()],
            |values| {
                let preset = if is_a(values, &a.organization.to_string()) {
                    ax.layout_preset
                } else {
                    bx.layout_preset
                };
                Payload::Json(json!({"name": "Net preset", "layout": {"preset": preset}}))
            },
        )
        .rejecting(vec![json!({"name": "Net preset b", "layout": {"preset": bx.layout_preset}})]),
        path_case(
            "PATCH /api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
            &[organization(), id(ax.layout_preset, bx.layout_preset)],
            json_body(json!({"description": "Net preset"})),
        )
        .rejecting(vec![json!({"layout": {"preset": bx.layout_preset}})]),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
            &[organization(), id(ax.layout_preset, bx.layout_preset)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/organizations/{organization_id}/leave",
            &[organization()],
            empty(),
        )
        .without_own("A's only owner can't leave A; leaving is checked in the workspace tests"),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/mail",
            &[organization()],
            json_body(json!({
                "provider": "smtp",
                "smtp": {"host": "mail.example.com", "port": 587, "security": "starttls"},
                "fromAddress": "net@example.com",
            })),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/mail/test",
            &[organization()],
            json_body(json!({"to": "net@example.com"})),
        ),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/mail",
            &[organization()],
            empty(),
        ),
        path_case(
            "PATCH /api/v1/organizations/{organization_id}/members/{actor_id}",
            &[organization(), id(a.rook, b.rook)],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": current(
                        "organization_memberships",
                        "version",
                        &[("organization_id", values[0].clone()), ("actor_id", values[1].clone())],
                        1,
                    ),
                    "role": "member",
                }))
            },
        ),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/members/{actor_id}/role",
            &[organization(), id(a.rook, b.rook)],
            |values| {
                Payload::Json(json!({
                    "roleId": ax.custom_role,
                    "applyToProjects": false,
                    "expectedVersion": current(
                        "organization_memberships",
                        "version",
                        &[("organization_id", values[0].clone()), ("actor_id", values[1].clone())],
                        1,
                    ),
                }))
            },
        )
        .rejecting(vec![json!({"roleId": bx.custom_role, "applyToProjects": true, "expectedVersion": 1})]),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/members/{actor_id}/agent-limit",
            &[organization(), id(a.rook, b.rook)],
            json_body(json!({"permissions": null})),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/roles",
            &[organization()],
            json_body(json!({"name": "Net made role", "baseRole": "member", "permissions": ["tasks.view"]})),
        ),
        path_case(
            "PATCH /api/v1/organizations/{organization_id}/roles/{role_id}",
            &[organization(), id(ax.custom_role, bx.custom_role)],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": current("workspace_roles", "version", &[("id", values[1].clone())], 1),
                    "description": "Net changed",
                }))
            },
        ),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/roles/{role_id}",
            &[organization(), id(sp.role, bx.custom_role)],
            empty(),
        ),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
            &[organization(), same("codex_app_server"), same("net-model")],
            json_body(json!({"displayName": "Net model"})),
        ),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/model-catalog/{kind}/models/{model_id}",
            &[organization(), same("codex_app_server"), same("net-model")],
            empty(),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/model-catalog/{kind}/refresh",
            &[organization(), same("codex_app_server")],
            empty(),
        )
        .own_reaches("execution_profile_required", NEEDS_ACCOUNT),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/onboarding",
            &[organization()],
            json_body(json!({"dismissed": false})),
        )
        .rejecting(vec![json!({"projectId": b.project, "dismissed": false})]),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/personal-secrets/{name}",
            &[organization(), secret()],
            json_body(json!({"value": "net-personal"})),
        ),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/personal-secrets/{name}",
            &[organization(), secret()],
            empty(),
        ),
        path_case(
            "PATCH /api/v1/organizations/{organization_id}/project-templates/{template_id}",
            &[organization(), id(ax.project_template, bx.project_template)],
            versioned("project_templates", 1, json!({"name": "Net setup"})),
        ),
        path_case(
            "PUT /api/v1/organizations/{organization_id}/secrets/{name}",
            &[organization(), secret()],
            json_body(json!({"value": "net-workspace"})),
        ),
        path_case(
            "DELETE /api/v1/organizations/{organization_id}/secrets/{name}",
            &[organization(), secret()],
            empty(),
        ),
        path_case(
            "POST /api/v1/organizations/{organization_id}/transfer-ownership",
            &[organization()],
            |values| {
                Payload::Json(json!({
                    "actorId": if values[0] == a.organization.to_string() { a.rook } else { b.rook }
                }))
            },
        )
        .rejecting(vec![json!({"actorId": b.rook})])
        .without_own("it would hand A to someone else; transfers are checked in the workspace tests"),
        path_case(
            "POST /api/v1/workspace-invitations/{invitation_id}/revoke",
            &[id(ax.workspace_invitation, bx.workspace_invitation)],
            versioned("workspace_invitations", 0, json!({})),
        ),
        // Pages
        path_case(
            "PATCH /api/v1/pages/{id}",
            &[id(&a.page, &b.page)],
            versioned("project_pages", 0, json!({"title": "Launch page"})),
        ),
        // Comments and suggestions on a page (AWD-53). Deleting the thread's
        // comment comes last, after the tools reopen it.
        path_case(
            "POST /api/v1/pages/{id}/comments",
            &[id(&a.page, &b.page)],
            json_body(json!({"body": "Net comment"})),
        ),
        path_case(
            "PATCH /api/v1/pages/{id}/comments/{comment_id}",
            &[id(&a.page, &b.page), id(a.page_thread, b.page_thread)],
            json_body(json!({"resolved": true})),
        ),
        path_case(
            "DELETE /api/v1/pages/{id}/comments/{comment_id}",
            &[id(&a.page, &b.page), id(a.page_thread, b.page_thread)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/pages/{id}/suggestions/{suggestion_id}/decision",
            &[
                id(&a.page, &b.page),
                id(a.page_suggestions[0], b.page_suggestions[0]),
            ],
            json_body(json!({"decision": "reject"})),
        ),
        // A page's live document: its generation is the page's ID (see the
        // fixture), and the change adds a paragraph to any document.
        path_case(
            "POST /api/v1/pages/{id}/document/sync",
            &[id(&a.page, &b.page)],
            |values| {
                Payload::Json(json!({"generation": values[0], "stateVector": STANDARD.encode([0_u8])}))
            },
        ),
        {
            let (update, state_vector) = new_paragraph_update();
            path_case(
                "POST /api/v1/pages/{id}/document/updates",
                &[id(&a.page, &b.page)],
                move |values| {
                    Payload::Json(json!({
                        "generation": values[0],
                        "update": update,
                        "stateVector": state_vector,
                    }))
                },
            )
        },
        // Project invitations
        path_case(
            "POST /api/v1/project-invitations/{invitation_id}/replace",
            &[id(ax.project_invitation, bx.project_invitation)],
            versioned("project_invitations", 0, json!({"expiresInHours": 48})),
        ),
        path_case(
            "POST /api/v1/project-invitations/{invitation_id}/revoke",
            &[id(sp.project_invitation, bx.project_invitation)],
            versioned("project_invitations", 0, json!({})),
        ),
        // Projects
        body_case(
            "POST /api/v1/projects",
            &[],
            json!({"organizationId": a.organization, "key": "NET", "name": "Net"}),
            vec![json!({"organizationId": b.organization, "key": "NETB", "name": "Net"})],
        )
        .rejecting(vec![json!({
            "organizationId": a.organization,
            "key": "NETC",
            "name": "Net",
            "templateId": bx.project_template,
        })]),
        // The rail's order is the caller's own; projects they can't open in
        // it are skipped.
        body_case(
            "PUT /api/v1/projects/rail-order",
            &[],
            json!({"projectIds": [a.project]}),
            Vec::new(),
        )
        .no_effect(vec![
            json!({"projectIds": [b.project]}),
            json!({"projectIds": [b.project, a.project]}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{id}",
            &[project()],
            versioned("projects", 0, json!({"name": "Work"})),
        ),
        path_case(
            "POST /api/v1/projects/{id}/archive",
            &[id(&sp.projects[0], &b.project)],
            versioned("projects", 0, json!({})),
        )
        .last(),
        // After the archive above, which runs first.
        path_case(
            "POST /api/v1/projects/{id}/restore",
            &[id(&sp.projects[0], &b.project)],
            versioned("projects", 0, json!({})),
        )
        .last(),
        path_case(
            "POST /api/v1/projects/{id}/channels",
            &[project()],
            json_body(json!({"name": "net-channel"})),
        ),
        path_case(
            "POST /api/v1/projects/{id}/decisions",
            &[project()],
            json_body(json!({"title": "Net decision"})),
        )
        .rejecting(vec![
            json!({"title": "Net decision", "sourceMessageId": b.message}),
            json!({"title": "Net decision", "sourceConversationId": b.channel}),
            json!({"title": "Net decision", "supersedes": bx.decision}),
        ]),
        path_case(
            "POST /api/v1/projects/{id}/execution-profiles",
            &[project()],
            empty(),
        )
        .rejecting(vec![json!({"providerAccountId": b.provider_account})]),
        path_case(
            "POST /api/v1/projects/{id}/harnesses/{kind}/catalog/refresh",
            &[project(), same("codex_app_server")],
            empty(),
        )
        .own_reaches("execution_profile_required", NEEDS_ACCOUNT),
        path_case(
            "PUT /api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
            &[project(), same("codex_app_server"), same("net-project-model")],
            json_body(json!({"displayName": "Net project model"})),
        ),
        path_case(
            "DELETE /api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
            &[project(), same("codex_app_server"), same("net-project-model")],
            empty(),
        ),
        path_case("PUT /api/v1/projects/{id}/navigation", &[project()], |values| {
            Payload::Json(json!({
                "location": {
                    "kind": "channel",
                    "conversationId": if is_a(values, &a.project) { &a.channel } else { &b.channel },
                },
                "expectedVersion": navigation_version(&values[0]),
            }))
        })
        .rejecting(vec![json!({
            "location": {"kind": "channel", "conversationId": b.channel},
            "expectedVersion": navigation_version(&a.project),
        })]),
        path_case(
            "POST /api/v1/projects/{id}/pages",
            &[project()],
            json_body(json!({"title": "Net page", "body": "Net"})),
        ),
        path_case(
            "POST /api/v1/projects/{id}/pages/import",
            &[project()],
            |_| Payload::Bytes("text/markdown", b"# Net import\n\nNet".to_vec()),
        )
        .query("?name=net.md"),
        path_case("POST /api/v1/projects/{id}/switch", &[project()], |values| {
            Payload::Json(json!({"expectedVersion": navigation_version(&values[0])}))
        }),
        // Agent profiles and runs
        path_case(
            "POST /api/v1/projects/{project_id}/agent-profiles",
            &[project()],
            json_body(json!({"template": "reviewer", "name": "Net reviewer"})),
        )
        .rejecting(vec![json!({
            "template": "reviewer",
            "name": "Net reviewer b",
            "environmentId": bx.environment,
        })]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/agent-profiles/{profile_id}",
            &[project(), id(ax.agent_profile, bx.agent_profile)],
            |values| {
                Payload::Json(json!({
                    "version": version("agent_profiles", &values[1]),
                    "description": "Net reviews",
                }))
            },
        )
        .rejecting(vec![json!({
            "version": version("agent_profiles", ax.agent_profile),
            "environmentId": bx.environment,
        })]),
        path_case(
            "DELETE /api/v1/projects/{project_id}/agent-profiles/{profile_id}",
            &[project(), id(sp.agent_profile, bx.agent_profile)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
            &[project(), id(a.run, b.run)],
            empty(),
        )
        .without_own("publishing pushes to GitHub, which tests can't reach"),
        path_case(
            "POST /api/v1/projects/{project_id}/agent-runs/{run_id}/pull-request",
            &[project(), looked_up(a.run, b.run)],
            json_body(json!({"title": "Net pull request", "body": "Net"})),
        )
        .without_own("opening a pull request asks GitHub, which tests can't reach"),
        // AI
        path_case(
            "POST /api/v1/projects/{project_id}/ai/analyze",
            &[project()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/ai/test",
            &[project()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/ai/plans",
            &[project()],
            json_body(json!({
                "title": "Net plan",
                "source": {"kind": "structured", "plan": {"tasks": [{"ref": "t1", "title": "Net step"}]}},
            })),
        ),
        path_case(
            "PUT /api/v1/projects/{project_id}/ai/plans/{import_id}",
            &[project(), id(a.plan, b.plan)],
            versioned(
                "ai_plan_imports",
                1,
                json!({"draft": {"tasks": [{"ref": "t1", "title": "Net step, edited"}]}}),
            ),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/ai/plans/{import_id}/commit",
            &[project(), id(a.plan, b.plan)],
            versioned("ai_plan_imports", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/accept",
            &[project(), id(ax.suggestion, bx.suggestion)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/dismiss",
            &[project(), id(sp.suggestions[0], bx.suggestion)],
            empty(),
        ),
        // Automations
        path_case(
            "POST /api/v1/projects/{project_id}/automations",
            &[project()],
            json_body(json!({
                "name": "Net rule",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "notify", "recipients": "owner", "message": "Net"}],
            })),
        )
        .rejecting(vec![
            json!({
                "name": "Net rule b",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "post_message", "conversationId": b.channel, "text": "Net"}],
            }),
            json!({
                "name": "Net rule c",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "launch", "environmentId": bx.environment}],
            }),
            json!({
                "name": "Net rule d",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "launch", "agentProfileId": bx.agent_profile}],
            }),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/automations/{rule_id}",
            &[project(), id(a.rule, b.rule)],
            versioned("automation_rules", 1, json!({"name": "Rule"})),
        )
        .rejecting(vec![json!({
            "expectedVersion": version("automation_rules", a.rule),
            "actions": [{"kind": "post_message", "conversationId": b.channel, "text": "Net"}],
        })]),
        path_case(
            "POST /api/v1/projects/{project_id}/automations/{rule_id}/run",
            &[project(), id(a.rule, b.rule)],
            empty(),
        )
        .rejecting(vec![json!({"workItemId": b.task})]),
        path_case(
            "DELETE /api/v1/projects/{project_id}/automations/{rule_id}",
            &[project(), id(a.rule, b.rule)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/projects/{project_id}/bug-report-links",
            &[project()],
            json_body(json!({"name": "Net game"})),
        ),
        // Environments
        path_case(
            "POST /api/v1/projects/{project_id}/environments",
            &[project()],
            json_body(json!({"name": "Net cloud", "harness": "codex_cloud", "providerEnvironmentId": "env-net"})),
        )
        .rejecting(vec![json!({
            "name": "Net cloud b",
            "harness": "codex_cloud",
            "providerEnvironmentId": "env-net-b",
            "repositoryId": b.repository,
        })]),
        path_case(
            "PUT /api/v1/projects/{project_id}/environments/{environment_id}",
            &[project(), id(ax.environment, bx.environment)],
            versioned(
                "project_environments",
                1,
                json!({"name": "Cloud", "harness": "codex_cloud", "providerEnvironmentId": "env-launch"}),
            ),
        )
        .rejecting(vec![json!({
            "name": "Cloud",
            "harness": "codex_cloud",
            "providerEnvironmentId": "env-launch",
            "repositoryId": b.repository,
            "expectedVersion": version("project_environments", ax.environment),
        })]),
        path_case(
            "POST /api/v1/projects/{project_id}/environments/{environment_id}/default",
            &[project(), id(ax.environment, bx.environment)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/environments/{environment_id}/check",
            &[project(), id(ax.environment, bx.environment)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/environments/{environment_id}/archive",
            &[project(), id(ax.environment, bx.environment)],
            empty(),
        )
        .last(),
        // Personal provider sign-in
        path_case(
            "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
            &[project(), id(a.profile, b.profile)],
            json_body(json!({"apiKey": "sk-net"})),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
            &[project(), id(a.profile, b.profile)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
            &[project(), id(a.profile, b.profile)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
            &[project(), id(a.profile, b.profile)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
            &[project(), id(a.profile, b.profile)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        // External agents. B's agent lives in B's Messages space.
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents",
            &[project()],
            json_body(json!({
                "connectionName": "Net agent",
                "displayName": "Net agent",
                "handle": "netagent",
                "role": "member",
                "scopes": ["read"],
            })),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/external-agents/{connection_id}",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            versioned("external_agent_connections", 1, json!({"enabled": true})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/credentials/rotate",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            versioned("external_agent_connections", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/webhook",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            json_body(json!({"url": "https://hooks.example.com/net-agent"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/webhook/test",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            json_body(json!({"registrationId": "reg_net"})),
        )
        .without_own("needs WorkOS, which tests don't have"),
        path_case(
            "DELETE /api/v1/projects/{project_id}/external-agents/{connection_id}/workos/{binding_id}",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
                same(Uuid::now_v7()),
            ],
            empty(),
        )
        .without_own("needs a WorkOS binding, which tests can't make"),
        path_case(
            "POST /api/v1/projects/{project_id}/external-agents/{connection_id}/remove",
            &[
                id(&a.project, &b.space),
                id(sp.agent_connection, b.agent_connection),
            ],
            versioned("external_agent_connections", 1, json!({})),
        ),
        // Imports
        path_case(
            "POST /api/v1/projects/{project_id}/imports",
            &[project()],
            json_body(json!({"source": "markdown", "text": "# Net\n- [ ] Net import\n", "fileName": "net.md"})),
        )
        .rejecting(vec![json!({"source": "github", "repositoryId": b.repository})]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/imports/{import_id}",
            &[project(), id(&a.import, &b.import)],
            versioned("task_imports", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/imports/{import_id}/commit",
            &[project(), id(&a.import, &b.import)],
            versioned("task_imports", 1, json!({})),
        ),
        // GitHub
        path_case(
            "PUT /api/v1/projects/{project_id}/integrations/github/automation",
            &[project()],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": current(
                        "project_github_automation_settings",
                        "version",
                        &[("project_id", values[0].clone())],
                        0,
                    ),
                    "pullRequestOpenEnabled": true,
                    "pullRequestMergeEnabled": true,
                }))
            },
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/integrations/github/connect",
            &[project()],
            json_body(json!({"mode": "install"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/integrations/github/repositories",
            &[project()],
            json_body(json!({"installationId": 1, "githubRepositoryId": 1})),
        )
        .without_own(NEEDS_GITHUB),
        path_case(
            "POST /api/v1/projects/{project_id}/integrations/github/sync",
            &[project()],
            |values| {
                Payload::Json(json!({
                    "installationId": 1,
                    "repositoryIds": [if is_a(values, &a.project) { a.repository } else { b.repository }],
                }))
            },
        )
        .rejecting(vec![json!({"installationId": 1, "repositoryIds": [b.repository]})])
        .without_own(NEEDS_GITHUB),
        // Invitations, labels, launches, members, milestones
        path_case(
            "POST /api/v1/projects/{project_id}/invitations",
            &[project()],
            json_body(json!({"role": "viewer", "expiresInHours": 24})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/labels",
            &[project()],
            json_body(json!({"name": "Net label"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/labels/{label_id}",
            &[project(), id(ax.label, bx.label)],
            versioned("project_labels", 1, json!({"description": "Net"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/fields",
            &[project()],
            json_body(json!({"name": "Net field", "type": "text"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/fields/{field_id}",
            &[project(), id(ax.field, bx.field)],
            versioned("field_definitions", 1, json!({"description": "Net"})),
        ),
        // Collections (AWD-90). A view or a record may only name A's fields,
        // people, tasks and pages.
        path_case(
            "POST /api/v1/projects/{project_id}/collections",
            &[project()],
            json_body(json!({
                "name": "Net collection",
                "fields": [{"name": "Who", "type": "person"}],
                "views": [{"name": "Mine", "filters": [{"field": "Who", "op": "is", "values": ["me"]}]}],
            })),
        )
        .rejecting(vec![
            json!({
                "name": "Net collection",
                "fields": [{"name": "Who", "type": "person"}],
                "views": [{"name": "Theirs", "filters": [{"field": "Who", "op": "is", "values": [b.rook]}]}],
            }),
            json!({
                "name": "Net collection",
                "fields": [{"name": "Built", "type": "link", "target": "task"}],
                "views": [{"name": "Theirs", "filters": [{"field": "Built", "op": "is", "values": [b.task]}]}],
            }),
            json!({
                "name": "Net collection",
                "fields": [{"name": "Spec", "type": "link", "target": "page"}],
                "views": [{"name": "Theirs", "filters": [{"field": "Spec", "op": "is", "values": [b.page]}]}],
            }),
            json!({"name": "Net collection", "fields": [{"field": b.collection_field, "name": "Net"}]}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/collections/{collection_id}",
            &[project(), id(a.collection, b.collection)],
            versioned("collections", 1, json!({"description": "Net"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/collections/{collection_id}/fields",
            &[project(), id(a.collection, b.collection)],
            json_body(json!({"name": "Net field", "type": "text"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/fields/{field_id}",
            &[
                project(),
                id(a.collection, b.collection),
                id(a.collection_field, b.collection_field),
            ],
            versioned("field_definitions", 2, json!({"description": "Net"})),
        )
        .and(vec![(
            // A task field isn't one of the collection's.
            Call::new(
                "PATCH",
                format!(
                    "/api/v1/projects/{}/collections/{}/fields/{}",
                    a.project, a.collection, ax.field
                ),
                Payload::Json(json!({
                    "description": "Net",
                    "expectedVersion": version("field_definitions", ax.field),
                })),
            ),
            Expect::Rejected,
        )]),
        path_case(
            "POST /api/v1/projects/{project_id}/collections/{collection_id}/views",
            &[project(), id(a.collection, b.collection)],
            json_body(json!({"name": "Net view", "config": {"sort": [{"field": "title"}]}})),
        )
        .rejecting(vec![
            json!({"name": "Net view", "config": {"filters": [{"field": b.collection_field, "op": "notEmpty"}]}}),
            json!({"name": "Net view", "config": {"groupBy": b.collection_field}}),
            json!({"name": "Net view", "config": {"sort": [{"field": b.collection_field.to_string()}]}}),
            json!({"name": "Net view", "config": {"columns": [{"field": b.collection_field}]}}),
            json!({"name": "Net view", "config": {"filters": [{"field": ax.record_fields[0], "op": "is", "values": [b.rook]}]}}),
            json!({"name": "Net view", "config": {"filters": [{"field": ax.record_fields[1], "op": "is", "values": [b.task]}]}}),
            json!({"name": "Net view", "config": {"filters": [{"field": ax.record_fields[2], "op": "is", "values": [b.page]}]}}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
            &[
                project(),
                id(a.collection, b.collection),
                id(a.collection_view, b.collection_view),
            ],
            versioned("collection_views", 2, json!({"name": "Net renamed view"})),
        )
        .rejecting(vec![json!({
            "config": {"groupBy": b.collection_field},
            "expectedVersion": version("collection_views", a.collection_view),
        })]),
        path_case(
            "DELETE /api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
            &[
                project(),
                id(a.collection, b.collection),
                id(sp.collection_view, b.collection_view),
            ],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/collections/{collection_id}/records",
            &[project(), id(a.collection, b.collection)],
            json_body(json!({"title": "Net record", "values": keyed(ax.record_fields[0], json!("me"))})),
        )
        .rejecting(vec![
            json!({"title": "Net record", "values": keyed(b.collection_field, json!("Net"))}),
            json!({"title": "Net record", "values": keyed(ax.record_fields[0], json!(b.rook))}),
            json!({"title": "Net record", "values": keyed(ax.record_fields[1], json!(b.task))}),
            json!({"title": "Net record", "values": keyed(ax.record_fields[2], json!(b.page))}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
            &[
                project(),
                id(a.collection, b.collection),
                id(a.record, b.record),
            ],
            versioned("collection_records", 2, json!({"title": "Net record title"})),
        )
        .rejecting(vec![
            json!({
                "values": keyed(ax.record_fields[0], json!(b.rook)),
                "expectedVersion": version("collection_records", a.record),
            }),
            json!({
                "values": keyed(ax.record_fields[2], json!(b.page)),
                "expectedVersion": version("collection_records", a.record),
            }),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/launches",
            &[project()],
            |values| {
                Payload::Json(json!({
                    "workItemIds": [if is_a(values, &a.project) { a.subtask } else { b.subtask }],
                    "startWhen": "at",
                    "startAt": far,
                    "prompt": "Net launch",
                }))
            },
        )
        .rejecting(vec![
            json!({"workItemIds": [b.task], "startWhen": "at", "startAt": far}),
            json!({"workItemIds": [a.subtask, b.task], "startWhen": "at", "startAt": far}),
            json!({"workItemIds": [a.subtask], "startWhen": "at", "startAt": far, "environmentId": bx.environment}),
            json!({"workItemIds": [a.subtask], "startWhen": "at", "startAt": far, "providerAccountId": b.provider_account}),
            json!({"workItemIds": [a.subtask], "startWhen": "at", "startAt": far, "agentProfileId": bx.agent_profile}),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/launches/{launch_id}/cancel",
            &[project(), id(ax.launch, bx.launch)],
            empty(),
        ),
        // Layouts
        path_case(
            "PUT /api/v1/projects/{project_id}/layout",
            &[project()],
            |values| {
                let preset = if is_a(values, &a.project) {
                    ax.layout_preset
                } else {
                    bx.layout_preset
                };
                Payload::Json(json!({"layout": {"preset": preset}}))
            },
        )
        .rejecting(vec![json!({"layout": {"preset": bx.layout_preset}})]),
        path_case(
            "PUT /api/v1/projects/{project_id}/layout/rules",
            &[project()],
            |values| {
                let (preset, person) = if is_a(values, &a.project) {
                    (ax.layout_preset, a.rook)
                } else {
                    (bx.layout_preset, b.rook)
                };
                Payload::Json(json!({"rules": [
                    {"role": "viewer", "preset": preset},
                    {"actorId": person, "preset": "bug_reporting"},
                ]}))
            },
        )
        .rejecting(vec![
            json!({"rules": [
                {"role": "viewer", "preset": bx.layout_preset},
                {"actorId": a.rook, "preset": "bug_reporting"},
            ]}),
            json!({"rules": [
                {"role": "viewer", "preset": ax.layout_preset},
                {"actorId": b.rook, "preset": "bug_reporting"},
            ]}),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/members",
            &[project()],
            json_body(json!({"actorId": ax.wren, "role": "member"})),
        )
        .rejecting(vec![json!({"actorId": bx.wren, "role": "member"})]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/members/{actor_id}",
            &[project(), id(a.rook, b.rook)],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": current(
                        "project_memberships",
                        "version",
                        &[("project_id", values[0].clone()), ("actor_id", values[1].clone())],
                        1,
                    ),
                    "enabled": true,
                }))
            },
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/milestones",
            &[project()],
            json_body(json!({"name": "Net milestone"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/milestones/{milestone_id}",
            &[project(), id(ax.milestone, bx.milestone)],
            versioned("project_milestones", 1, json!({"description": "Net"})),
        ),
        // Provider accounts
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts",
            &[project()],
            json_body(json!({"adapterKind": "codex_app_server", "name": "Net account"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/provider-accounts/{account_id}",
            &[project(), id(a.provider_account, b.provider_account)],
            versioned("provider_accounts", 1, json!({"name": "Net account renamed"})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/provider-accounts/{account_id}",
            &[project(), id(sp.provider_account, b.provider_account)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/use",
            &[project(), id(a.provider_account, b.provider_account)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/api-key",
            &[project(), id(a.provider_account, b.provider_account)],
            json_body(json!({"apiKey": "sk-net"})),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/browser",
            &[project(), id(a.provider_account, b.provider_account)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/cancel",
            &[project(), id(a.provider_account, b.provider_account)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/login/device-code",
            &[project(), id(a.provider_account, b.provider_account)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/provider-accounts/{account_id}/logout",
            &[project(), id(a.provider_account, b.provider_account)],
            empty(),
        )
        .without_own(NEEDS_CLI),
        path_case(
            "POST /api/v1/projects/{project_id}/pull-requests/{pull_request_id}/refresh",
            &[project(), id(a.pull_request, b.pull_request)],
            empty(),
        )
        .without_own(NEEDS_GITHUB),
        // Recurring tasks and templates
        path_case(
            "POST /api/v1/projects/{project_id}/recurring-tasks",
            &[project()],
            |values| {
                Payload::Json(json!({
                    "templateId": if is_a(values, &a.project) { ax.task_template } else { bx.task_template },
                    "frequency": "weekly",
                    "weekdays": [2],
                }))
            },
        )
        .rejecting(vec![
            json!({"templateId": bx.task_template, "frequency": "weekly", "weekdays": [2]}),
            json!({"templateId": ax.task_template, "frequency": "weekly", "weekdays": [3], "assigneeActorId": b.rook}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
            &[project(), id(ax.recurring, bx.recurring)],
            versioned("recurring_tasks", 1, json!({"enabled": true})),
        )
        .rejecting(vec![json!({
            "expectedVersion": version("recurring_tasks", ax.recurring),
            "assigneeActorId": b.rook,
        })]),
        path_case(
            "DELETE /api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
            &[project(), id(ax.recurring, bx.recurring)],
            empty(),
        )
        .last(),
        path_case(
            "POST /api/v1/projects/{project_id}/task-templates",
            &[project()],
            json_body(json!({"name": "Net template", "title": "Net review"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/task-templates/{template_id}",
            &[project(), id(ax.task_template, bx.task_template)],
            versioned("task_templates", 1, json!({"archived": false})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/task-templates/{template_id}/apply",
            &[project(), looked_up(ax.task_template, bx.task_template)],
            json_body(json!({"title": "Net applied"})),
        )
        .rejecting(vec![
            json!({"title": "Net applied", "assigneeActorId": b.rook}),
            json!({"title": "Net applied", "milestoneId": bx.milestone}),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/save-as-template",
            &[project()],
            json_body(json!({"name": "Net setup two", "description": "Net"})),
        ),
        // Repositories
        path_case(
            "POST /api/v1/projects/{project_id}/repositories",
            &[project()],
            json_body(json!({
                "githubRepositoryId": 9001,
                "owner": "acme",
                "name": "net",
                "cloneUrl": "https://github.com/acme/net.git",
                "defaultBranch": "main",
                "trustAcknowledged": true,
            })),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/repositories/{repository_id}/prepare",
            &[project(), id(a.repository, b.repository)],
            empty(),
        )
        .without_own("preparing clones the repository from GitHub"),
        path_case(
            "POST /api/v1/projects/{project_id}/repositories/{repository_id}/archive",
            &[project(), id(sp.repository, b.repository)],
            versioned("repositories", 1, json!({})),
        )
        .last(),
        // Project secrets
        path_case(
            "PUT /api/v1/projects/{project_id}/secrets/{name}",
            &[project(), secret()],
            json_body(json!({"value": "net-project"})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/secrets/{name}",
            &[project(), secret()],
            empty(),
        ),
        // Views
        path_case(
            "POST /api/v1/projects/{project_id}/views",
            &[project()],
            json_body(json!({"name": "Net view"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/views/{view_id}",
            &[project(), id(ax.view, bx.view)],
            versioned("saved_views", 1, json!({"name": "Launch view"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/views/{view_id}/pin",
            &[project(), id(ax.view, bx.view)],
            json_body(json!({"pinned": true})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/views/{view_id}",
            &[project(), id(ax.view, bx.view)],
            empty(),
        )
        .last(),
        // The workflow
        path_case(
            "PUT /api/v1/projects/{project_id}/work-item-workflow",
            &[project()],
            |values| {
                Payload::Json(with(&a_workflow, "expectedVersion", workflow_version(&values[0])))
            },
        )
        .rejecting(vec![
            with(
                &b_status_workflow,
                "expectedVersion",
                workflow_version(&a.project),
            ),
            with(
                &b_type_workflow,
                "expectedVersion",
                workflow_version(&a.project),
            ),
        ]),
        // Tasks
        path_case(
            "POST /api/v1/projects/{project_id}/work-items",
            &[project()],
            json_body(json!({"title": "Net task"})),
        )
        .rejecting(vec![
            json!({"title": "Net task", "typeId": bx.task_type}),
            json!({"title": "Net task", "statusId": bx.status}),
            json!({"title": "Net task", "assigneeActorId": b.rook}),
            json!({"title": "Net task", "parentWorkItemId": b.task}),
            json!({"title": "Net task", "milestoneId": bx.milestone}),
            json!({"title": "Net task", "labelIds": [bx.label]}),
            json!({"title": "Net task", "sourceMessageId": b.message}),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/bulk",
            &[project()],
            |values| {
                let task = if is_a(values, &a.project) { a.subtask } else { b.subtask };
                Payload::Json(json!({
                    "items": [{"id": task, "expectedVersion": version("work_items", task)}],
                    "changes": {"priority": "high"},
                }))
            },
        )
        .rejecting(vec![
            json!({
                "items": [{"id": b.task, "expectedVersion": version("work_items", b.task)}],
                "changes": {"priority": "high"},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"statusId": bx.status},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"assigneeActorId": b.rook},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"milestoneId": bx.milestone},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"addLabelIds": [bx.label]},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"fieldValues": keyed(bx.field, json!("Net"))},
            }),
            json!({
                "items": [{"id": a.subtask, "expectedVersion": version("work_items", a.subtask)}],
                "changes": {"fieldValues": keyed(ax.person_field, json!(b.rook))},
            }),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}",
            &[project(), task()],
            versioned("work_items", 1, json!({"title": "Launch plan"})),
        )
        .rejecting(
            [
                json!({"typeId": bx.task_type}),
                json!({"statusId": bx.status}),
                json!({"assigneeActorId": b.rook}),
                json!({"parentWorkItemId": b.task}),
                json!({"milestoneId": bx.milestone}),
                json!({"addLabelIds": [bx.label]}),
                json!({"fieldValues": keyed(bx.field, json!("Net"))}),
                json!({"fieldValues": keyed(ax.person_field, json!(b.rook))}),
            ]
            .into_iter()
            .map(|change| with(&change, "expectedVersion", version("work_items", a.task)))
            .collect(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
            &[project(), task()],
            versioned("work_items", 1, json!({"text": "Net criterion"})),
        ),
        path_case(
            "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            &[project(), task(), id(ax.criterion, bx.criterion)],
            versioned("work_items", 1, json!({"completed": true})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            &[project(), task(), id(ax.criterion, bx.criterion)],
            versioned("work_items", 1, json!({})),
        )
        .last(),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/criteria",
            &[project(), task()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/summary",
            &[project(), task()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/ai/triage",
            &[project(), task()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
            &[project(), id(sp.task, b.task)],
            versioned("work_items", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
            &[project(), id(sp.task, b.task)],
            versioned("work_items", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
            &[project(), task()],
            json_body(json!({"content": text("Net comment"), "attachments": [ax.comment_upload]})),
        )
        .rejecting(vec![
            json!({"content": text("Net comment"), "attachments": [bx.comment_upload]}),
        ]),
        path_case(
            "PATCH /api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
            &[project(), task(), id(a.comment, b.comment)],
            json_body(json!({"content": text("Launch comment, edited over HTTP")})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
            &[project(), task(), id(sp.comments[0], b.comment)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/dependencies",
            &[project(), task()],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": version("work_items", &values[1]),
                    "blockedBy": [if is_a(values, &a.project) { sp.task } else { b.subtask }.to_string()],
                }))
            },
        )
        .rejecting(vec![
            json!({"expectedVersion": version("work_items", a.task), "blockedBy": [b.task.to_string()]}),
            json!({"expectedVersion": version("work_items", a.task), "blocks": [b.task.to_string()]}),
        ]),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/files",
            &[project(), task()],
            |_| Payload::Bytes("image/png", tiny_png()),
        )
        .query("?name=net.png"),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/move",
            &[project(), task()],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": version("work_items", &values[1]),
                    "targetStatusId": if is_a(values, &a.project) { ax.status } else { bx.status },
                    "targetPosition": 1,
                }))
            },
        )
        .rejecting(vec![json!({
            "expectedVersion": version("work_items", a.task),
            "targetStatusId": bx.status,
            "targetPosition": 1,
        })]),
        // AWD-102: moving to another project. A's own request names a spare
        // project of A's with a made-up preview snapshot, so it passes every
        // access check and stops at the snapshot (a real move is in
        // awd102_task_move); B's task, project or target is refused first.
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
            &[project(), task()],
            |values| {
                Payload::Json(json!({
                    "targetProjectId": if is_a(values, &a.project) { sp.projects[0].clone() } else { a.project.clone() },
                    "snapshot": "0".repeat(64),
                }))
            },
        )
        .rejecting(vec![json!({"targetProjectId": b.project, "snapshot": "0".repeat(64)})])
        .own_reaches(
            "conflict",
            "a move needs its own fresh preview; a made-up snapshot stops there, past every access check",
        )
        .last(),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/relations",
            &[project(), task()],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": version("work_items", &values[1]),
                    "kind": "relates_to",
                    "target": if is_a(values, &a.project) { sp.task } else { b.subtask }.to_string(),
                }))
            },
        )
        .rejecting(vec![json!({
            "expectedVersion": version("work_items", a.task),
            "kind": "relates_to",
            "target": b.task.to_string(),
        })]),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/release-claim",
            &[project(), task()],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            &[project(), task(), id(a.repository, b.repository)],
            versioned("work_items", 1, json!({"role": "related"})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
            &[project(), task(), id(a.repository, b.repository)],
            versioned("work_items", 1, json!({})),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            &[project(), task(), id(a.repository, b.repository)],
            versioned("work_items", 1, json!({})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            &[project(), task(), id(&a.thread, &b.thread)],
            empty(),
        ),
        path_case(
            "DELETE /api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            &[project(), task(), id(&a.thread, &b.thread)],
            empty(),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/watch",
            &[project(), task()],
            json_body(json!({"watching": true})),
        ),
        path_case(
            "POST /api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
            &[project(), task()],
            |values| {
                Payload::Json(json!({
                    "expectedVersion": version("work_items", &values[1]),
                    "channelId": if is_a(values, &a.project) { &a.channel } else { &b.channel },
                    "prompt": "Net work",
                }))
            },
        )
        .rejecting(vec![
            json!({"expectedVersion": version("work_items", a.task), "channelId": b.channel, "prompt": "Net work"}),
            json!({
                "expectedVersion": version("work_items", a.task),
                "channelId": a.channel,
                "prompt": "Net work",
                "repositoryId": b.repository,
            }),
        ]),
        // Push
        body_case(
            "POST /api/v1/push/subscriptions",
            &[],
            json!({
                "endpoint": "https://fcm.googleapis.com/fcm/send/net-device",
                "keys": push_keys,
                "label": "Net phone",
                "organizationId": a.organization,
            }),
            vec![json!({
                "endpoint": "https://fcm.googleapis.com/fcm/send/net-device-b",
                "keys": push_keys,
                "label": "Net phone",
                "organizationId": b.organization,
            })],
        ),
        path_case(
            "POST /api/v1/push/subscriptions/{id}/test",
            &[id(ax.push_subscription, bx.push_subscription)],
            empty(),
        )
        .without_own("it sends a real notification through the device's push service"),
        path_case(
            "DELETE /api/v1/push/subscriptions/{id}",
            &[id(ax.push_subscription, bx.push_subscription)],
            empty(),
        )
        .last(),
        // Reply requests
        path_case(
            "POST /api/v1/reply-requests/{id}/stop",
            &[id(sp.reply_stop, bx.reply)],
            empty(),
        ),
        path_case(
            "POST /api/v1/reply-requests/{id}/retry",
            &[id(ax.stopped_reply, bx.stopped_reply)],
            empty(),
        ),
        // Settings
        body_case(
            "PUT /api/v1/settings/values/{key}",
            &["run.model"],
            json!({"scope": "workspace", "organizationId": a.organization, "value": "net-model"}),
            vec![
                json!({"scope": "workspace", "organizationId": b.organization, "value": "net-model"}),
                json!({"scope": "project", "projectId": b.project, "value": "net-model"}),
            ],
        ),
        body_case(
            "POST /api/v1/settings/values/{key}/reset",
            &["run.model"],
            json!({"scope": "project", "projectId": a.project}),
            vec![
                json!({"scope": "project", "projectId": b.project}),
                json!({"scope": "workspace", "organizationId": b.organization}),
            ],
        ),
        // Usage
        path_case(
            "POST /api/v1/usage/accounts/{account_id}/batch",
            &[id(a.provider_account, b.provider_account)],
            |values| {
                let (project, task) = if is_a(values, &a.provider_account.to_string()) {
                    (&a.project, a.subtask)
                } else {
                    (&b.project, b.subtask)
                };
                Payload::Json(json!({"tasks": [{"projectId": project, "workItemId": task}]}))
            },
        )
        // A batch answers for each project; B's is listed as one the account
        // doesn't run tasks in, and nothing launches.
        .no_effect(vec![json!({"tasks": [{"projectId": b.project, "workItemId": b.task}]})]),
        path_case(
            "POST /api/v1/usage/accounts/{account_id}/refresh",
            &[id(a.provider_account, b.provider_account)],
            empty(),
        ),
        path_case(
            "POST /api/v1/usage/accounts/{account_id}/reset-credits/spend",
            &[id(a.provider_account, b.provider_account)],
            json_body(json!({"confirm": true})),
        )
        .own_reaches("conflict", NEEDS_ACCOUNT),
        path_case(
            "PUT /api/v1/usage/accounts/{account_id}/rules/{rule}",
            &[id(a.provider_account, b.provider_account), same("high_usage")],
            json_body(json!({"enabled": true})),
        ),
        path_case(
            "POST /api/v1/usage/accounts/{account_id}/schedules",
            &[id(a.provider_account, b.provider_account)],
            json_body(json!({
                "label": "Net weekly",
                "cadence": "weekly",
                "weekday": 2,
                "minuteOfDay": 600,
                "timeZone": "UTC",
            })),
        ),
        path_case(
            "DELETE /api/v1/usage/accounts/{account_id}/schedules/{schedule_id}",
            &[
                id(a.provider_account, b.provider_account),
                id(ax.usage_schedule, bx.usage_schedule),
            ],
            empty(),
        )
        .last(),
        // Webhooks
        body_case(
            "POST /api/v1/webhooks",
            &[],
            json!({
                "name": "Net hook",
                "url": "https://hooks.example.com/net",
                "eventTypes": ["run.finished"],
                "organizationId": a.organization,
            }),
            vec![
                json!({
                    "name": "Net hook",
                    "url": "https://hooks.example.com/net",
                    "eventTypes": ["run.finished"],
                    "organizationId": b.organization,
                }),
                json!({
                    "name": "Net hook",
                    "url": "https://hooks.example.com/net",
                    "eventTypes": ["run.finished"],
                    "projectId": b.project,
                }),
            ],
        ),
        path_case(
            "PATCH /api/v1/webhooks/{webhook_id}",
            &[id(a.webhook, b.webhook)],
            versioned("webhook_subscriptions", 0, json!({"name": "Hook"})),
        ),
        path_case(
            "POST /api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver",
            &[id(a.webhook, b.webhook), id(ax.delivery, bx.delivery)],
            empty(),
        ),
        path_case(
            "POST /api/v1/webhooks/{webhook_id}/rotate-secret",
            &[id(a.webhook, b.webhook)],
            empty(),
        ),
        path_case(
            "POST /api/v1/webhooks/{webhook_id}/test",
            &[id(a.webhook, b.webhook)],
            empty(),
        ),
        path_case(
            "DELETE /api/v1/webhooks/{webhook_id}",
            &[id(a.webhook, b.webhook)],
            empty(),
        )
        .last(),
        // Whiteboards
        body_case(
            "POST /api/v1/whiteboards",
            &[],
            json!({"title": "Net sketch", "organizationId": a.organization}),
            vec![
                json!({"title": "Net sketch", "organizationId": b.organization}),
                json!({"title": "Net sketch", "projectId": b.project}),
            ],
        )
        .rejecting(vec![json!({
            "title": "Net sketch",
            "projectId": a.project,
            "link": {"targetKind": "task", "targetId": b.task},
        })]),
        path_case(
            "PATCH /api/v1/whiteboards/{id}",
            &[id(a.project_whiteboard, b.project_whiteboard)],
            versioned("whiteboards", 0, json!({"title": "Plan"})),
        )
        .rejecting(vec![json!({
            "projectId": b.project,
            "expectedVersion": version("whiteboards", a.project_whiteboard),
        })]),
        path_case(
            "POST /api/v1/whiteboards/{id}/duplicate",
            &[id(a.whiteboard, b.whiteboard)],
            json_body(json!({"title": "Net copy"})),
        ),
        path_case(
            "POST /api/v1/whiteboards/{id}/files",
            &[id(a.whiteboard, b.whiteboard)],
            json_body(json!({"id": "netphoto", "mimeType": "image/png", "dataURL": TINY_PNG})),
        ),
        path_case(
            "POST /api/v1/whiteboards/{id}/links",
            &[id(a.project_whiteboard, b.project_whiteboard)],
            |values| {
                Payload::Json(json!({
                    "targetKind": "task",
                    "targetId": if values[0] == a.project_whiteboard.to_string() { a.subtask } else { b.subtask },
                }))
            },
        )
        .rejecting(vec![
            json!({"targetKind": "task", "targetId": b.task}),
            json!({"targetKind": "page", "targetId": b.page}),
            json!({"targetKind": "project", "targetId": b.project}),
        ]),
        path_case(
            "DELETE /api/v1/whiteboards/{id}/links/{kind}/{target_id}",
            &[id(a.whiteboard, b.whiteboard), same("project"), project()],
            empty(),
        ),
        path_case(
            "POST /api/v1/whiteboards/{id}/operations",
            &[id(a.whiteboard, b.whiteboard)],
            json_body(json!({"operations": [{"op": "add", "type": "rectangle", "id": "net", "text": "Net"}]})),
        ),
        path_case(
            "POST /api/v1/whiteboards/{id}/scene",
            &[id(a.project_whiteboard, b.project_whiteboard)],
            json_body(json!({"elements": []})),
        ),
        path_case(
            "POST /api/v1/whiteboards/{id}/revisions/{revision}/restore",
            &[id(a.whiteboard, b.whiteboard), same(1)],
            empty(),
        ),
        // Before Empty trash, which would take this board too.
        path_case(
            "DELETE /api/v1/whiteboards/{id}",
            &[id(sp.trashed_whiteboard, bx.trashed_whiteboard)],
            empty(),
        ),
        // Only ever the caller's own trash: B's workspace or project narrows
        // it to nothing.
        body_case(
            "POST /api/v1/whiteboard-trash/empty",
            &[],
            json!({"organizationId": a.organization}),
            Vec::new(),
        )
        .no_effect(vec![
            json!({"organizationId": b.organization}),
            json!({"projectId": b.project}),
        ]),
        // OAuth consent: connecting an app as B's agent.
        consent_case(
            "POST /connect/confirm",
            &sp.oauth_client,
            a.agent_connection,
            b.agent_connection,
        ),
        consent_case(
            "POST /oauth/authorize",
            &sp.oauth_client,
            a.agent_connection,
            b.agent_connection,
        ),
    ]
}

const NEEDS_CLI: &str =
    "signing in to a provider runs its command-line tool, which tests don't have";
const NEEDS_ACCOUNT: &str = "it needs a signed-in provider account, which tests don't have";
const NEEDS_GITHUB: &str = "it asks GitHub, which tests can't reach";

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

/// Each caller's bearer token and MCP session.
struct McpCallers<'a> {
    app: (&'a str, &'a str),
    agent: (&'a str, &'a str),
    requester: (&'a str, &'a str),
}

impl McpCallers<'_> {
    fn of(&self, caller: Caller) -> (&'static str, (&str, &str)) {
        match caller {
            Caller::App => ("A's app", self.app),
            Caller::Agent => ("A's agent", self.agent),
            Caller::Requester => ("A's requesting agent", self.requester),
        }
    }
}

/// What a tool call answered, for a failure message: its structured result,
/// or the JSON-RPC reply when it has none.
fn answer(body: &str) -> String {
    let result = structured(body);
    let shown = if result.is_null() {
        rpc_reply(body).to_string()
    } else {
        result.to_string()
    };
    excerpt(&shown, 500)
}

#[allow(clippy::too_many_arguments)]
async fn check_foreign_mcp(
    app: &axum::Router,
    pool: &PgPool,
    callers: &McpCallers<'_>,
    case: &McpCase,
    arguments: &Value,
    expect: Expect,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    let tool = case.tool;
    let arguments = resolve(pool, arguments.clone()).await;
    let mut who = vec![Caller::App, Caller::Agent];
    if !who.contains(&case.caller) {
        who.push(case.caller);
    }
    for caller in who {
        let (who, (token, session)) = callers.of(caller);
        report.mcp_calls += 1;
        let (status, body) = call_tool(app, token, session, tool, arguments.clone()).await;
        let result = structured(&body);
        let outcome = result["outcome"].clone();
        // A batch that changed nothing, naming each task it couldn't use.
        let rejected_batch = outcome == "rejected"
            && result["problems"]
                .as_array()
                .is_some_and(|problems| !problems.is_empty());
        let refused = status == StatusCode::FORBIDDEN
            || rejected_batch
            || matches!(outcome.as_str(), Some("not_found" | "permission_denied"));
        let accepted = match expect {
            Expect::Refused => refused,
            Expect::Rejected => {
                refused || matches!(outcome.as_str(), Some("invalid_request" | "conflict"))
            }
            Expect::NoEffect => true,
        };
        if !accepted {
            report.failures.push(format!(
                "HOLE? {tool}: {who} {arguments} -> {status} outcome {outcome} (expected {}): {}",
                match expect {
                    Expect::Refused => "not_found or permission_denied",
                    _ => "a refusal",
                },
                answer(&body)
            ));
        }
        leak(
            report,
            &format!("{tool}: {who} {arguments}"),
            &arguments.to_string(),
            &body,
            b_ids,
        );
    }
}

async fn check_own_mcp(
    app: &axum::Router,
    pool: &PgPool,
    callers: &McpCallers<'_>,
    case: &McpCase,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    let arguments = resolve(pool, case.own.clone()).await;
    let (who, (token, session)) = callers.of(case.caller);
    report.mcp_calls += 1;
    let (status, body) = call_tool(app, token, session, case.tool, arguments.clone()).await;
    let outcome = structured(&body)["outcome"].clone();
    // `requested`: run_request's, as a person approves the run.
    if !succeeded(&outcome) && outcome != "requested" {
        report.failures.push(format!(
            "OWN {}: {who} {arguments} -> {status} outcome {outcome} (A's own call must \
             succeed, or the refusals of B's prove nothing): {}",
            case.tool,
            answer(&body)
        ));
    }
    leak(
        report,
        &format!("{}: {who}'s own {arguments}", case.tool),
        &arguments.to_string(),
        &body,
        b_ids,
    );
}

#[allow(clippy::too_many_lines)]
fn mcp_cases(a: &World, ax: &Targets, sp: &Spares, b: &World, bx: &Targets) -> Vec<McpCase> {
    let soon = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let far = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    let task_version = |task: Uuid| version("work_items", task);
    let generation = |request: Uuid| {
        current(
            "reply_requests",
            "generation",
            &[("id", request.to_string())],
            0,
        )
    };
    let workflow_version = |project: &str| {
        current(
            "project_task_settings",
            "workflow_version",
            &[("project_id", project.to_owned())],
            1,
        )
    };
    // The tool names a status's category `category`.
    let drafts = |statuses: &[Value]| {
        statuses
            .iter()
            .map(|status| {
                json!({
                    "id": status["id"],
                    "name": status["name"],
                    "category": status["semanticCategory"],
                    "isDefault": status["isDefault"],
                })
            })
            .collect::<Vec<_>>()
    };
    // As over HTTP: A's statuses and types with one more each, new in A's
    // own call and B's in the others. The HTTP request added its own, which
    // A's call retires.
    let added_status = json!({"name": "Net tool status", "category": "review", "isDefault": false});
    let added_type = json!({"name": "Net tool type", "isDefault": false});
    let plus = |entries: &[Value], entry: Value| {
        let mut entries = entries.to_vec();
        entries.push(entry);
        entries
    };
    let a_statuses = drafts(&sp.statuses);
    let own_statuses = plus(&a_statuses, added_status.clone());
    let own_types = plus(&sp.types, added_type.clone());
    let rule = json!({
        "trigger": {"kind": "task_created"},
        "actions": [{"kind": "notify", "recipients": "owner", "message": "Net"}],
    });

    vec![
        mcp_case(
            "usage_batch_launch",
            json!({"accountId": a.provider_account, "taskIds": [a.subtask], "clientRequestId": "net-batch-own"}),
            vec![json!({"accountId": b.provider_account, "taskIds": [b.subtask], "clientRequestId": "net-batch-b"})],
        )
        .rejecting(vec![json!({"accountId": a.provider_account, "taskIds": [b.task], "clientRequestId": "net-batch-mixed"})]),
        mcp_case(
            "project_layout_set",
            json!({"projectId": a.project, "preset": "Software team"}),
            vec![json!({"projectId": b.project, "preset": "Software team"})],
        )
        .rejecting(vec![json!({"projectId": a.project, "preset": bx.layout_preset})]),
        mcp_case(
            "project_member_set",
            json!({"projectId": a.project, "person": "@wren", "role": "viewer"}),
            vec![json!({"projectId": b.project, "person": "@wren", "role": "viewer"})],
        )
        .rejecting(vec![json!({"projectId": a.project, "person": bx.wren, "role": "viewer"})]),
        mcp_case(
            "invitation_write",
            json!({"projectId": a.project, "role": "viewer", "expiresInHours": 24}),
            vec![json!({"projectId": b.project, "role": "viewer"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "invitation": bx.project_invitation, "revoke": true}),
            json!({"projectId": a.project, "invitation": bx.project_invitation, "replace": true}),
            json!({"projectId": a.project, "role": bx.custom_role}),
        ]),
        mcp_case(
            "role_write",
            json!({"projectId": a.project, "name": "Net tool role", "baseRole": "viewer", "permissions": ["tasks.view"]}),
            vec![json!({"projectId": b.project, "name": "Net tool role b", "baseRole": "viewer", "permissions": ["tasks.view"]})],
        )
        .rejecting(vec![json!({"projectId": a.project, "role": bx.custom_role, "description": "Net"})]),
        mcp_case(
            "workspace_template_apply",
            json!({"projectId": a.project, "template": "everything"}),
            vec![json!({"projectId": b.project, "template": "everything"})],
        )
        .rejecting(vec![json!({"projectId": a.project, "template": bx.workspace_template})]),
        mcp_case(
            "workspace_template_save",
            json!({"projectId": a.project, "name": "Net tool template", "includeProject": true}),
            vec![json!({"projectId": b.project, "name": "Net tool template b"})],
        )
        .rejecting(vec![json!({"projectId": a.project, "remove": bx.workspace_template})]),
        mcp_case(
            "project_template_write",
            json!({"projectId": a.project, "name": "Net mcp setup"}),
            vec![json!({"projectId": b.project, "name": "Net mcp setup b"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "template": bx.project_template, "name": "Net"}),
        ]),
        mcp_case(
            "role_assign",
            json!({"projectId": a.project, "person": "@wren", "role": ax.custom_role}),
            vec![json!({"projectId": b.project, "person": "@wren", "role": "member"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "person": bx.wren, "role": "member"}),
            json!({"projectId": a.project, "person": "@wren", "role": bx.custom_role}),
        ]),
        mcp_case(
            "project_update",
            json!({"projectId": a.project, "name": "Work"}),
            vec![json!({"projectId": b.project, "name": "Work"})],
        ),
        mcp_case(
            "project_archive",
            json!({"project": sp.projects[1], "archived": true}),
            vec![json!({"project": b.project, "archived": true})],
        )
        .last(),
        mcp_case(
            "conversation_start",
            json!({"people": ["@rook"], "name": "Net group", "text": "Net hello"}),
            Vec::new(),
        )
        // People are looked up among those the caller can message: B's
        // aren't, the same as for an ID nobody has.
        .rejecting(vec![
            json!({"people": [b.rook, bx.wren], "name": "Net group", "text": "Net hello"}),
            json!({"people": [a.rook, bx.wren], "name": "Net group", "text": "Net hello"}),
        ]),
        mcp_case(
            "message_send",
            json!({
                "conversationId": a.channel,
                "text": "Net message",
                "attachmentIds": [sp.tool_upload],
                "clientRequestId": "net-send-own",
            }),
            vec![
                json!({"conversationId": b.channel, "text": "Net message", "clientRequestId": "net-send-b1"}),
                json!({"conversationId": b.dm, "text": "Net message", "clientRequestId": "net-send-b2"}),
                json!({"conversationId": b.group, "text": "Net message", "clientRequestId": "net-send-b3"}),
            ],
        )
        .rejecting(vec![json!({
            "conversationId": a.channel,
            "text": "Net message",
            "attachmentIds": [bx.channel_upload],
            "clientRequestId": "net-send-mixed",
        })]),
        mcp_case(
            "file_upload",
            json!({"conversationId": a.channel, "name": "net.txt", "text": "Net"}),
            vec![
                json!({"conversationId": b.channel, "name": "net.txt", "text": "Net"}),
                json!({"taskId": b.task, "name": "net.txt", "text": "Net"}),
            ],
        ),
        mcp_case(
            "file_delete",
            json!({"fileId": sp.tool_delete_upload}),
            vec![
                json!({"fileId": bx.channel_upload}),
                json!({"fileId": b.task_file}),
            ],
        ),
        mcp_case(
            "message_forward",
            json!({
                "messageId": a.message,
                "conversationId": a.group,
                "text": "Net forward",
                "clientRequestId": "net-forward-own",
            }),
            vec![
                json!({"messageId": b.message, "conversationId": a.group, "clientRequestId": "net-forward-b1"}),
                json!({"messageId": a.message, "conversationId": b.channel, "clientRequestId": "net-forward-b2"}),
                json!({"messageId": a.message, "conversationId": b.group, "clientRequestId": "net-forward-b3"}),
            ],
        ),
        mcp_case(
            "message_edit",
            json!({"messageId": a.message, "text": "Launch message, edited again"}),
            vec![json!({"messageId": b.message, "text": "Net edit"})],
        ),
        mcp_case(
            "message_delete",
            json!({"messageId": sp.messages[1]}),
            vec![json!({"messageId": b.message})],
        ),
        mcp_case(
            "conversation_members",
            json!({"conversationId": a.group, "add": ["@wren"]}),
            vec![json!({"conversationId": b.group, "add": ["@mara"]})],
        )
        .rejecting(vec![json!({"conversationId": a.group, "add": [bx.agent]})]),
        mcp_case(
            "conversation_continue",
            json!({"conversationId": a.group}),
            vec![json!({"conversationId": b.group})],
        ),
        mcp_case(
            "mark_read",
            json!({"conversationId": a.channel}),
            vec![json!({"conversationId": b.channel})],
        ),
        mcp_case(
            "notification_update",
            json!({"action": "archive", "ids": [ax.notification]}),
            vec![json!({"action": "read", "all": true, "projectId": b.project})],
        )
        .no_effect(vec![json!({"action": "read", "ids": [bx.notification]})]),
        mcp_case(
            "later_save",
            json!({"messageId": a.file_message}),
            vec![json!({"messageId": b.message}), json!({"task": b.task})],
        ),
        mcp_case(
            "later_update",
            json!({"id": ax.later, "text": "Net"}),
            vec![json!({"id": bx.later, "text": "Net"})],
        ),
        mcp_case(
            "message_schedule",
            json!({"conversationId": a.channel, "text": "Net later", "sendAt": soon}),
            vec![json!({"conversationId": b.channel, "text": "Net later", "sendAt": soon})],
        ),
        mcp_case(
            "scheduled_message_update",
            json!({"id": sp.scheduled[1], "text": "Net later, edited"}),
            vec![json!({"id": bx.scheduled, "text": "Net later, edited"})],
        ),
        mcp_case(
            "thread_start",
            json!({"conversationId": a.channel, "name": "Net mcp thread", "text": "Net"}),
            vec![json!({"conversationId": b.channel, "name": "Net mcp thread", "text": "Net"})],
        )
        .rejecting(vec![json!({
            "conversationId": a.channel,
            "fromMessageId": b.message,
            "name": "Net mcp thread",
            "text": "Net",
        })]),
        mcp_case(
            "thread_update",
            json!({"threadId": a.thread, "name": "Launch thread"}),
            vec![json!({"threadId": b.thread, "name": "Net"})],
        ),
        mcp_case(
            "channel_create",
            json!({"projectId": a.project, "name": "net-mcp"}),
            vec![json!({"projectId": b.project, "name": "net-mcp"})],
        ),
        mcp_case(
            "conversation_update",
            json!({"conversationId": sp.channel, "name": "net-spare-renamed"}),
            vec![json!({"conversationId": b.channel, "name": "net"})],
        ),
        mcp_case(
            "pin_set",
            json!({"conversationId": a.channel, "messageId": a.message, "pinned": true}),
            vec![json!({"conversationId": b.channel, "messageId": b.message, "pinned": false})],
        )
        .rejecting(vec![
            json!({"conversationId": a.channel, "messageId": b.message}),
            json!({"conversationId": a.channel, "pageId": b.page}),
            json!({"conversationId": a.channel, "decisionId": bx.decision}),
        ]),
        mcp_case(
            "message_react",
            json!({"messageId": a.message, "emoji": "eyes"}),
            vec![json!({"messageId": b.message, "emoji": "eyes"})],
        ),
        // The agent's replies.
        mcp_case(
            "reply_claim",
            json!({"requestId": sp.reply_claim}),
            vec![json!({"requestId": bx.reply})],
        )
        .by_agent(),
        mcp_case(
            "reply_progress",
            json!({"requestId": sp.reply_claim, "generation": generation(sp.reply_claim), "status": "Looking"}),
            vec![json!({"requestId": bx.claimed_reply, "generation": generation(bx.claimed_reply), "status": "Looking"})],
        )
        .by_agent(),
        mcp_case(
            "reply_send",
            json!({
                "requestId": sp.reply_claim,
                "generation": generation(sp.reply_claim),
                "text": "Net reply",
                "final": true,
                "attachmentIds": [sp.reply_upload],
                "clientRequestId": "net-reply-own",
            }),
            vec![json!({
                "requestId": bx.claimed_reply,
                "generation": generation(bx.claimed_reply),
                "text": "Net reply",
                "final": true,
                "clientRequestId": "net-reply-b",
            })],
        )
        // On the request the agent already holds, as `reply_claim` hasn't
        // claimed the own call's one yet when this runs.
        .rejecting(vec![json!({
            "requestId": ax.claimed_reply,
            "generation": generation(ax.claimed_reply),
            "text": "Net reply",
            "final": true,
            "attachmentIds": [bx.claimed_upload],
            "clientRequestId": "net-reply-mixed",
        })])
        .by_agent(),
        mcp_case(
            "reply_pass",
            json!({"requestId": sp.reply_pass, "reason": "Net"}),
            vec![json!({"requestId": bx.reply, "reason": "Net"})],
        )
        .by_agent(),
        mcp_case(
            "reply_release",
            json!({"requestId": sp.reply_release, "reason": "Net"}),
            vec![json!({"requestId": bx.claimed_reply, "reason": "Net"})],
        )
        .by_agent(),
        mcp_case(
            "reply_stop",
            json!({"requestId": sp.reply_stop_mcp}),
            vec![json!({"requestId": bx.reply})],
        ),
        mcp_case(
            "reply_retry",
            json!({"requestId": sp.reply_retry_mcp}),
            vec![json!({"requestId": bx.stopped_reply})],
        ),
        mcp_case(
            "session_bind",
            json!({"conversationId": ax.claimed_conversation, "externalRef": "net-session", "harness": "codex"}),
            vec![json!({"conversationId": bx.claimed_conversation, "externalRef": "net-session-b", "harness": "codex"})],
        )
        .by_agent(),
        // Tasks
        mcp_case(
            "task_create",
            json!({"projectId": a.project, "title": "Net mcp task", "clientRequestId": "net-create-own"}),
            vec![json!({"projectId": b.project, "title": "Net mcp task", "clientRequestId": "net-create-b"})],
        )
        .rejecting(
            [
                ("parentId", json!(b.task)),
                ("assignee", json!(b.rook)),
                ("milestone", json!(bx.milestone)),
                ("status", json!(bx.status)),
                ("type", json!(bx.task_type)),
                ("beforeTaskId", json!(bx.backlog_task)),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (key, value))| {
                with(
                    &json!({"projectId": a.project, "title": "Net mcp task", "clientRequestId": format!("net-create-mixed-{index}")}),
                    key,
                    value,
                )
            })
            .collect(),
        ),
        mcp_case(
            "task_update",
            json!({"taskId": a.task, "expectedVersion": task_version(a.task), "title": "Launch plan"}),
            vec![json!({"taskId": b.task, "expectedVersion": task_version(b.task), "title": "Net"})],
        )
        .rejecting(
            [
                ("parentId", json!(b.task)),
                ("assignee", json!(b.rook)),
                ("milestone", json!(bx.milestone)),
                ("status", json!(bx.status)),
                ("type", json!(bx.task_type)),
                // In Ready, as the task is.
                ("beforeTaskId", json!(b.subtask)),
                ("fields", keyed(bx.field, json!("Net"))),
                ("fields", keyed(ax.person_field, json!(b.rook))),
            ]
            .into_iter()
            .map(|(key, value)| {
                with(
                    &json!({"taskId": a.task, "expectedVersion": task_version(a.task)}),
                    key,
                    value,
                )
            })
            .collect(),
        ),
        mcp_case(
            "task_create_many",
            json!({"projectId": a.project, "tasks": [{"title": "Net many"}], "clientRequestId": "net-many-own"}),
            vec![json!({"projectId": b.project, "tasks": [{"title": "Net many"}], "clientRequestId": "net-many-b"})],
        )
        .rejecting(vec![
            json!({
                "projectId": a.project,
                "tasks": [{"title": "Net many", "parentId": b.task}],
                "clientRequestId": "net-many-mixed",
            }),
            json!({
                "projectId": a.project,
                "tasks": [{"title": "Net many", "fields": keyed(bx.field, json!("Net"))}],
                "clientRequestId": "net-many-field",
            }),
            json!({
                "projectId": a.project,
                "tasks": [{"title": "Net many", "fields": keyed(ax.person_field, json!(b.rook))}],
                "clientRequestId": "net-many-person",
            }),
        ]),
        mcp_case(
            "task_update_many",
            json!({"projectId": a.project, "taskIds": [a.subtask], "priority": "high", "clientRequestId": "net-update-many-own"}),
            vec![
                json!({"taskIds": [b.task], "priority": "high", "clientRequestId": "net-update-many-b1"}),
                json!({"projectId": b.project, "taskIds": [b.task], "priority": "high", "clientRequestId": "net-update-many-b2"}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "taskIds": [a.subtask, b.task], "priority": "high", "clientRequestId": "net-update-many-m1"}),
            json!({"projectId": a.project, "taskIds": [a.subtask], "milestone": bx.milestone, "clientRequestId": "net-update-many-m2"}),
            json!({"projectId": a.project, "taskIds": [a.subtask], "assignee": b.rook, "clientRequestId": "net-update-many-m3"}),
            json!({"projectId": a.project, "taskIds": [a.subtask], "fields": keyed(bx.field, json!("Net")), "clientRequestId": "net-update-many-m4"}),
            json!({"projectId": a.project, "taskIds": [a.subtask], "fields": keyed(ax.person_field, json!(b.rook)), "clientRequestId": "net-update-many-m5"}),
        ]),
        mcp_case(
            "task_criteria",
            json!({
                "taskId": a.task,
                "expectedVersion": task_version(a.task),
                "operations": [{"op": "add", "text": "Net criterion two"}],
                "clientRequestId": "net-criteria-own",
            }),
            vec![json!({
                "taskId": b.task,
                "expectedVersion": task_version(b.task),
                "operations": [{"op": "add", "text": "Net criterion two"}],
                "clientRequestId": "net-criteria-b",
            })],
        )
        .rejecting(vec![json!({
            "taskId": a.task,
            "expectedVersion": task_version(a.task),
            "operations": [{"op": "check", "criterion": bx.criterion}],
            "clientRequestId": "net-criteria-mixed",
        })]),
        mcp_case(
            "task_comment",
            json!({
                "taskId": a.task,
                "text": "Net comment",
                "attachmentIds": [sp.tool_comment_upload],
                "clientRequestId": "net-comment-own",
            }),
            vec![json!({"taskId": b.task, "text": "Net comment", "clientRequestId": "net-comment-b"})],
        )
        .rejecting(vec![json!({
            "taskId": a.task,
            "text": "Net comment",
            "attachmentIds": [bx.comment_upload],
            "clientRequestId": "net-comment-mixed",
        })]),
        mcp_case(
            "task_comment_edit",
            json!({"taskId": a.task, "commentId": a.comment, "text": "Launch comment, edited by a tool"}),
            vec![json!({"taskId": b.task, "commentId": b.comment, "text": "Net edit"})],
        )
        .rejecting(vec![
            json!({"taskId": a.task, "commentId": b.comment, "text": "Net edit"}),
            json!({"taskId": b.task, "commentId": a.comment, "text": "Net edit"}),
        ]),
        mcp_case(
            "task_comment_delete",
            json!({"taskId": a.task, "commentId": sp.comments[1]}),
            vec![json!({"taskId": b.task, "commentId": b.comment})],
        )
        .rejecting(vec![
            json!({"taskId": a.task, "commentId": b.comment}),
            json!({"taskId": b.task, "commentId": sp.comments[1]}),
        ]),
        mcp_case(
            "task_mark_blocked",
            json!({
                "taskId": a.subtask,
                "expectedVersion": task_version(a.subtask),
                "reason": "Net waiting",
                "clientRequestId": "net-blocked-own",
            }),
            vec![json!({
                "taskId": b.task,
                "expectedVersion": task_version(b.task),
                "reason": "Net waiting",
                "clientRequestId": "net-blocked-b",
            })],
        ),
        mcp_case(
            "task_mark_ready_for_review",
            json!({
                "taskId": a.subtask,
                "expectedVersion": task_version(a.subtask),
                "clientRequestId": "net-review-own",
            }),
            vec![json!({
                "taskId": b.task,
                "expectedVersion": task_version(b.task),
                "clientRequestId": "net-review-b",
            })],
        ),
        mcp_case(
            "task_link_pull_request",
            json!({
                "taskId": a.task,
                "expectedVersion": task_version(a.task),
                "pullRequestId": a.pull_request,
                "clientRequestId": "net-pr-own",
            }),
            vec![json!({
                "taskId": b.task,
                "expectedVersion": task_version(b.task),
                "pullRequestId": b.pull_request,
                "clientRequestId": "net-pr-b",
            })],
        )
        .rejecting(vec![json!({
            "taskId": a.task,
            "expectedVersion": task_version(a.task),
            "pullRequestId": b.pull_request,
            "clientRequestId": "net-pr-mixed",
        })]),
        mcp_case(
            "task_reorder",
            json!({"taskIds": [sp.second_task], "afterTaskId": sp.task}),
            vec![json!({"taskIds": [b.task], "afterTaskId": b.subtask})],
        )
        .rejecting(vec![json!({"taskIds": [sp.second_task], "beforeTaskId": bx.backlog_task})]),
        mcp_case(
            "task_claim",
            json!({"taskId": a.subtask, "note": "Net claim"}),
            vec![json!({"taskId": b.subtask, "note": "Net claim"})],
        )
        .by_agent(),
        mcp_case(
            "task_release",
            json!({"taskId": a.subtask, "handoff": "Net handoff"}),
            vec![json!({"taskId": b.task, "handoff": "Net handoff"})],
        )
        .by_agent(),
        mcp_case(
            "task_link",
            json!({"taskId": a.task, "conversationId": a.channel}),
            vec![json!({"taskId": b.task, "conversationId": b.channel})],
        )
        .rejecting(
            [
                ("conversationId", json!(b.channel)),
                ("messageId", json!(b.message)),
                ("pageId", json!(b.page)),
                ("decisionId", json!(bx.decision)),
                ("repositoryId", json!(b.repository)),
                ("pullRequestId", json!(b.pull_request)),
                ("whiteboardId", json!(b.project_whiteboard)),
            ]
            .into_iter()
            .map(|(key, value)| with(&json!({"taskId": a.task}), key, value))
            .collect(),
        ),
        mcp_case(
            "field_write",
            json!({"projectId": a.project, "name": "Net mcp field", "type": "number"}),
            vec![json!({"projectId": b.project, "name": "Net mcp field", "type": "number"})],
        )
        .rejecting(vec![json!({
            "projectId": a.project,
            "field": bx.field,
            "description": "Net",
            "expectedVersion": version("field_definitions", bx.field),
        })]),
        mcp_case(
            "collection_write",
            json!({
                "projectId": a.project,
                "collection": a.collection,
                "description": "Net mcp",
                "fields": [{"name": "Net mcp field", "type": "number"}],
            }),
            vec![
                json!({"projectId": a.project, "collection": b.collection, "description": "Net"}),
                json!({"projectId": b.project, "collection": b.collection, "description": "Net"}),
                json!({"projectId": b.project, "name": "Net mcp collection"}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "collection": a.collection, "fields": [{"field": b.collection_field, "name": "Net"}]}),
            json!({"projectId": a.project, "collection": a.collection, "views": [{"view": b.collection_view, "name": "Net"}]}),
            json!({
                "projectId": a.project,
                "collection": a.collection,
                "views": [{"name": "Net mcp view", "filters": [{"field": "Tester", "op": "is", "values": [b.rook]}]}],
            }),
            json!({
                "projectId": a.project,
                "collection": a.collection,
                "views": [{"name": "Net mcp view", "filters": [{"field": "Build", "op": "is", "values": [b.task]}]}],
            }),
            json!({
                "projectId": a.project,
                "collection": a.collection,
                "views": [{"name": "Net mcp view", "groupBy": b.collection_field}],
            }),
        ]),
        mcp_case(
            "record_write",
            json!({
                "projectId": a.project,
                "collection": a.collection,
                "records": [
                    {"record": "#1", "fields": {"Notes": "Net mcp"}},
                    {"title": "Net mcp record", "fields": {"Tester": "me", "Build": a.task, "Spec": a.page}},
                ],
            }),
            vec![
                json!({"projectId": a.project, "collection": b.collection, "records": [{"title": "Net"}]}),
                json!({"projectId": b.project, "collection": b.collection, "records": [{"title": "Net"}]}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "collection": a.collection, "records": [{"record": b.record, "title": "Net"}]}),
            json!({"projectId": a.project, "collection": a.collection, "records": [{"title": "Net", "fields": {"Tester": b.rook}}]}),
            json!({"projectId": a.project, "collection": a.collection, "records": [{"title": "Net", "fields": {"Build": b.task}}]}),
            json!({"projectId": a.project, "collection": a.collection, "records": [{"title": "Net", "fields": {"Spec": b.page}}]}),
            json!({"projectId": a.project, "collection": a.collection, "records": [{"title": "Net", "fields": keyed(b.collection_field, json!("Net"))}]}),
        ]),
        mcp_case(
            "view_write",
            json!({"projectId": a.project, "name": "Net mcp view", "query": {"assigneeActorId": "me"}, "pinned": true}),
            vec![
                json!({"projectId": b.project, "name": "Net mcp view"}),
                json!({"projectId": b.project, "view": bx.view, "name": "Net"}),
                json!({"projectId": a.project, "view": bx.view, "name": "Net"}),
                json!({"projectId": a.project, "view": bx.view, "pinned": true}),
                json!({"projectId": a.project, "view": bx.view, "delete": true}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "name": "Net", "query": {"milestoneId": bx.milestone}}),
            json!({"projectId": a.project, "name": "Net", "query": {"labelIds": bx.label}}),
        ]),
        mcp_case(
            "label_write",
            json!({"projectId": a.project, "name": "Net mcp label"}),
            vec![json!({"projectId": b.project, "name": "Net mcp label"})],
        )
        .rejecting(vec![json!({
            "projectId": a.project,
            "label": bx.label,
            "description": "Net",
            "expectedVersion": version("project_labels", bx.label),
        })]),
        McpCase {
            // Moving B's task into A, or A's into B.
            foreign: vec![
                (json!({"taskId": b.task, "toProjectId": a.project}), Expect::Refused),
                (json!({"taskId": sp.task, "toProjectId": b.project}), Expect::Refused),
                // AWD-102: a preview, and a confirmation, name nothing of B's.
                (
                    json!({"taskId": b.task, "toProjectId": a.project, "preview": true}),
                    Expect::Refused,
                ),
                (
                    json!({"taskId": sp.task, "toProjectId": b.project, "preview": true}),
                    Expect::Refused,
                ),
                (
                    json!({"taskId": sp.task, "toProjectId": b.project, "snapshot": "0".repeat(64),
                           "leaveUnresolved": true, "clientRequestId": "net-move-b"}),
                    Expect::Refused,
                ),
            ],
            ..mcp_case(
                "task_move",
                json!({"taskId": sp.task, "toProjectId": sp.projects[0]}),
                Vec::new(),
            )
            // The move leaves the task archived here, as moved.
            .last()
        },
        mcp_case(
            "page_edit",
            json!({"pageId": a.page, "expectedVersion": version("project_pages", &a.page), "edits": [{"op": "append", "markdown": "Net block"}]}),
            vec![json!({"pageId": b.page, "expectedVersion": version("project_pages", &b.page), "edits": [{"op": "append", "markdown": "Net"}]})],
        ),
        mcp_case(
            "page_comment",
            json!({"pageId": a.page, "text": "Net mcp comment"}),
            vec![
                json!({"pageId": b.page, "text": "Net"}),
                json!({"pageId": a.page, "text": "Net", "replyTo": b.page_thread}),
            ],
        ),
        mcp_case(
            "page_comment_update",
            json!({"pageId": a.page, "commentId": a.page_thread, "resolved": false}),
            vec![
                json!({"pageId": b.page, "commentId": b.page_thread, "resolved": true}),
                json!({"pageId": a.page, "commentId": b.page_thread, "resolved": true}),
            ],
        ),
        mcp_case(
            "page_suggestion_decide",
            json!({"pageId": a.page, "suggestionId": a.page_suggestions[1], "decision": "reject"}),
            vec![
                json!({"pageId": b.page, "suggestionId": b.page_suggestions[1], "decision": "reject"}),
                json!({"pageId": a.page, "suggestionId": b.page_suggestions[1], "decision": "reject"}),
            ],
        ),
        mcp_case(
            "page_write",
            json!({"pageId": a.page, "body": "Launch notes, third draft", "expectedVersion": version("project_pages", &a.page)}),
            vec![
                json!({"pageId": b.page, "body": "Net", "expectedVersion": version("project_pages", &b.page)}),
                json!({"projectId": b.project, "title": "Net page", "body": "Net"}),
            ],
        )
        .rejecting(vec![json!({
            "projectId": a.project,
            "title": "Net mcp page",
            "body": "Net",
            "pinToConversationId": b.channel,
        })]),
        // Whiteboards
        mcp_case(
            "whiteboard_create",
            json!({"title": "Net board", "projectId": a.project}),
            vec![
                json!({"title": "Net board", "projectId": b.project}),
                json!({"title": "Net board", "workspaceId": b.organization}),
            ],
        )
        .rejecting(vec![json!({
            "title": "Net board",
            "projectId": a.project,
            "linkTo": {"targetKind": "task", "targetId": b.task},
        })]),
        mcp_case(
            "whiteboard_edit",
            json!({"whiteboardId": a.whiteboard, "operations": [{"op": "add", "type": "rectangle", "id": "netmcp", "text": "Net"}]}),
            vec![json!({"whiteboardId": b.whiteboard, "operations": [{"op": "add", "type": "rectangle", "id": "netmcp", "text": "Net"}]})],
        ),
        mcp_case(
            "whiteboard_update",
            json!({"whiteboardId": a.project_whiteboard, "title": "Plan"}),
            vec![json!({"whiteboardId": b.project_whiteboard, "title": "Net"})],
        )
        .rejecting(vec![json!({"whiteboardId": a.project_whiteboard, "projectId": b.project})]),
        mcp_case(
            "whiteboard_trash_empty",
            json!({"workspaceId": a.organization}),
            Vec::new(),
        )
        .no_effect(vec![
            json!({"workspaceId": b.organization}),
            json!({"projectId": b.project}),
        ]),
        mcp_case(
            "whiteboard_duplicate",
            json!({"whiteboardId": a.whiteboard, "title": "Net mcp copy"}),
            vec![json!({"whiteboardId": b.whiteboard, "title": "Net mcp copy"})],
        ),
        mcp_case(
            "whiteboard_link",
            json!({"whiteboardId": a.whiteboard, "targetKind": "task", "targetId": a.subtask}),
            vec![json!({"whiteboardId": b.whiteboard, "targetKind": "task", "targetId": b.subtask})],
        )
        .rejecting(vec![
            json!({"whiteboardId": a.whiteboard, "targetKind": "task", "targetId": b.task}),
            json!({"whiteboardId": a.whiteboard, "targetKind": "page", "targetId": b.page}),
        ]),
        // Decisions and planning
        mcp_case(
            "decision_record",
            json!({"projectId": a.project, "title": "Net mcp decision"}),
            vec![json!({"projectId": b.project, "title": "Net mcp decision"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "title": "Net mcp decision", "fromMessageId": b.message}),
            json!({"projectId": a.project, "title": "Net mcp decision", "conversationId": b.channel}),
            json!({"projectId": a.project, "title": "Net mcp decision", "supersedes": bx.decision}),
        ]),
        mcp_case(
            "task_depend",
            json!({"taskId": a.subtask, "blockedBy": [sp.task]}),
            vec![json!({"taskId": b.task, "blockedBy": [b.subtask]})],
        )
        .rejecting(vec![
            json!({"taskId": a.subtask, "blockedBy": [b.task]}),
            json!({"taskId": a.subtask, "blocks": [b.task]}),
        ]),
        mcp_case(
            "task_from_template",
            json!({"projectId": a.project, "template": ax.task_template, "title": "Net from template"}),
            vec![json!({"projectId": b.project, "template": bx.task_template, "title": "Net from template"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "template": bx.task_template, "title": "Net from template"}),
            json!({"projectId": a.project, "template": ax.task_template, "title": "Net", "assignee": b.rook}),
            json!({"projectId": a.project, "template": ax.task_template, "title": "Net", "milestone": bx.milestone}),
        ]),
        mcp_case(
            "template_write",
            json!({"projectId": a.project, "name": "Net mcp template", "title": "Net templated", "subtasks": [{"title": "Net step"}]}),
            vec![
                json!({"projectId": b.project, "name": "Net mcp template", "title": "Net"}),
                json!({"projectId": b.project, "template": bx.task_template, "title": "Net"}),
                json!({"projectId": a.project, "template": bx.task_template, "title": "Net"}),
                json!({"projectId": a.project, "template": bx.task_template, "archived": true}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "name": "Net typed", "title": "Net", "type": bx.task_type}),
            json!({"projectId": a.project, "template": ax.task_template, "type": bx.task_type}),
        ]),
        mcp_case(
            "recurring_write",
            json!({"projectId": a.project, "template": ax.task_template, "frequency": "weekly", "weekdays": [4], "assignee": "me"}),
            vec![
                json!({"projectId": b.project, "template": bx.task_template, "frequency": "daily"}),
                json!({"projectId": b.project, "recurring": bx.recurring, "enabled": false}),
                json!({"projectId": a.project, "recurring": bx.recurring, "enabled": false}),
                json!({"projectId": a.project, "recurring": bx.recurring, "delete": true}),
                json!({"projectId": a.project, "template": bx.task_template, "frequency": "daily"}),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "template": ax.task_template, "frequency": "daily", "assignee": b.rook}),
            json!({"projectId": a.project, "recurring": ax.recurring, "assignee": b.rook}),
        ]),
        mcp_case(
            "task_import",
            json!({"projectId": a.project, "source": "markdown", "text": "- [ ] Net imported"}),
            vec![json!({"projectId": b.project, "source": "markdown", "text": "- [ ] Net imported"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "importId": b.import}),
            json!({"projectId": a.project, "source": "github", "repositoryId": b.repository}),
        ]),
        mcp_case(
            "webhook_write",
            json!({
                "operation": "create",
                "name": "Net mcp hook",
                "url": "https://hooks.example.com/net-mcp",
                "eventTypes": ["run.finished"],
            }),
            vec![
                json!({"operation": "update", "webhookId": b.webhook, "name": "Net", "expectedVersion": version("webhook_subscriptions", b.webhook)}),
                json!({"operation": "delete", "webhookId": b.webhook}),
                json!({"operation": "rotate_secret", "webhookId": b.webhook}),
                json!({"operation": "test", "webhookId": b.webhook}),
                json!({"operation": "redeliver", "webhookId": b.webhook, "deliveryId": bx.delivery}),
                json!({
                    "operation": "create",
                    "name": "Net mcp hook",
                    "url": "https://hooks.example.com/net-mcp",
                    "eventTypes": ["run.finished"],
                    "projectId": b.project,
                }),
            ],
        )
        .rejecting(vec![json!({"operation": "redeliver", "webhookId": a.webhook, "deliveryId": bx.delivery})]),
        mcp_case(
            "automation_write",
            with(&with(&rule, "projectId", json!(a.project)), "operation", json!("create"))
                .as_object()
                .map(|fields| {
                    let mut fields = fields.clone();
                    fields.insert("name".to_owned(), json!("Net mcp rule"));
                    Value::Object(fields)
                })
                .expect("rule"),
            vec![
                json!({"projectId": b.project, "operation": "run", "ruleId": b.rule}),
                json!({
                    "projectId": b.project,
                    "operation": "create",
                    "name": "Net mcp rule",
                    "trigger": {"kind": "task_created"},
                    "actions": [{"kind": "notify", "recipients": "owner", "message": "Net"}],
                }),
            ],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "operation": "update", "ruleId": b.rule, "name": "Net", "expectedVersion": version("automation_rules", b.rule)}),
            json!({"projectId": a.project, "operation": "delete", "ruleId": b.rule}),
            json!({"projectId": a.project, "operation": "run", "ruleId": b.rule}),
            json!({"projectId": a.project, "operation": "run", "ruleId": a.rule, "taskId": b.task}),
            json!({
                "projectId": a.project,
                "operation": "create",
                "name": "Net mcp rule b",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "post_message", "conversationId": b.channel, "text": "Net"}],
            }),
        ]),
        mcp_case(
            "milestone_write",
            json!({"projectId": a.project, "name": "Net mcp milestone", "addTasks": [a.subtask]}),
            vec![json!({"projectId": b.project, "name": "Net mcp milestone"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "milestoneId": bx.milestone, "name": "Net", "expectedVersion": version("project_milestones", bx.milestone)}),
            json!({"projectId": a.project, "milestoneId": ax.milestone, "addTasks": [b.task], "expectedVersion": version("project_milestones", ax.milestone)}),
        ]),
        // AI
        mcp_case(
            "ai_suggestion_decide",
            json!({"projectId": a.project, "suggestionId": sp.suggestions[1], "decision": "dismiss"}),
            vec![json!({"projectId": b.project, "suggestionId": bx.suggestion, "decision": "dismiss"})],
        )
        .rejecting(vec![json!({"projectId": a.project, "suggestionId": bx.suggestion, "decision": "dismiss"})]),
        mcp_case(
            "ai_analyze",
            json!({"projectId": a.project, "taskId": a.task}),
            vec![json!({"projectId": b.project, "taskId": b.task})],
        )
        .rejecting(vec![json!({"projectId": a.project, "taskId": b.task})]),
        mcp_case(
            "ai_summarize",
            json!({"projectId": a.project, "taskId": a.task}),
            vec![json!({"projectId": b.project, "taskId": b.task})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "taskId": b.task}),
            json!({"projectId": a.project, "conversationId": b.channel}),
        ]),
        mcp_case(
            "plan_import",
            json!({"projectId": a.project, "title": "Net mcp plan", "plan": "- Net step"}),
            vec![json!({"projectId": b.project, "title": "Net mcp plan", "plan": "- Net step"})],
        )
        .rejecting(vec![
            json!({"projectId": a.project, "title": "Net page plan", "pageId": b.page}),
            json!({"projectId": a.project, "importId": b.plan, "commit": true, "expectedVersion": version("ai_plan_imports", b.plan)}),
        ]),
        mcp_case(
            "task_relate",
            json!({"taskId": a.subtask, "otherTaskId": sp.task, "kind": "relates_to"}),
            vec![json!({"taskId": b.task, "otherTaskId": b.subtask, "kind": "relates_to"})],
        )
        .rejecting(vec![json!({"taskId": a.subtask, "otherTaskId": b.task, "kind": "relates_to"})]),
        mcp_case(
            "task_watch",
            json!({"taskId": a.subtask, "watching": true}),
            vec![json!({"taskId": b.task, "watching": true})],
        ),
        mcp_case(
            "workflow_update",
            json!({
                "projectId": a.project,
                "expectedVersion": workflow_version(&a.project),
                "statuses": own_statuses,
                "types": own_types,
                "allTransitions": true,
            }),
            vec![json!({
                "projectId": b.project,
                "expectedVersion": workflow_version(&b.project),
                "statuses": own_statuses,
                "types": own_types,
                "allTransitions": true,
            })],
        )
        .rejecting(vec![
            json!({
                "projectId": a.project,
                "expectedVersion": workflow_version(&a.project),
                "statuses": plus(&a_statuses, with(&added_status, "id", json!(bx.status))),
                "types": own_types,
                "allTransitions": true,
            }),
            json!({
                "projectId": a.project,
                "expectedVersion": workflow_version(&a.project),
                "statuses": own_statuses,
                "types": plus(&sp.types, with(&added_type, "id", json!(bx.task_type))),
                "allTransitions": true,
            }),
        ]),
        // Runs
        mcp_case(
            "run_start",
            json!({
                "threadId": ax.run_threads[1],
                "repositoryId": a.repository,
                "prompt": "Net run",
                "clientRequestId": "net-run-own",
            }),
            vec![json!({"threadId": bx.run_threads[1], "prompt": "Net run", "clientRequestId": "net-run-b"})],
        )
        .rejecting(vec![json!({
            "threadId": ax.run_threads[1],
            "prompt": "Net run",
            "repositoryId": b.repository,
            "clientRequestId": "net-run-mixed",
        })]),
        mcp_case(
            "task_launch",
            json!({"taskIds": [a.subtask], "startWhen": "at", "startAt": far, "prompt": "Net", "clientRequestId": "net-launch-own"}),
            vec![json!({"taskIds": [b.task], "startWhen": "at", "startAt": far, "prompt": "Net", "clientRequestId": "net-launch-b"})],
        )
        .rejecting(vec![
            json!({"taskIds": [a.subtask, b.task], "startWhen": "at", "startAt": far, "clientRequestId": "net-launch-m1"}),
            json!({"taskIds": [a.subtask], "startWhen": "at", "startAt": far, "environmentId": bx.environment, "clientRequestId": "net-launch-m2"}),
        ]),
        mcp_case(
            "run_request",
            json!({
                "conversationId": ax.run_threads[1],
                "repositoryId": a.repository,
                "sponsorActorId": ax.owner,
                "prompt": "Net request",
                "clientRequestId": "net-request-own",
            }),
            vec![json!({
                "conversationId": bx.run_threads[1],
                "repositoryId": b.repository,
                "sponsorActorId": bx.owner,
                "prompt": "Net request",
                "clientRequestId": "net-request-b",
            })],
        )
        .rejecting(vec![
            json!({
                "conversationId": ax.run_threads[1],
                "repositoryId": b.repository,
                "sponsorActorId": ax.owner,
                "prompt": "Net request",
                "clientRequestId": "net-request-m1",
            }),
            json!({
                "conversationId": ax.run_threads[1],
                "repositoryId": a.repository,
                "sponsorActorId": bx.owner,
                "prompt": "Net request",
                "clientRequestId": "net-request-m2",
            }),
        ])
        .by_requester(),
        mcp_case(
            "run_stop",
            json!({"runId": sp.run_stop, "clientRequestId": "net-stop-own"}),
            vec![json!({"runId": b.run, "clientRequestId": "net-stop-b"})],
        ),
        mcp_case(
            "run_retry",
            json!({"runId": sp.run_retry, "clientRequestId": "net-retry-own"}),
            vec![json!({"runId": bx.failed_run, "clientRequestId": "net-retry-b"})],
        ),
        mcp_case(
            "launch_cancel",
            json!({"launchId": sp.launch}),
            vec![json!({"launchId": bx.launch})],
        ),
        mcp_case(
            "approval_respond",
            json!({
                "approvalId": sp.approvals[1],
                "expectedVersion": version("agent_approvals", sp.approvals[1]),
                "decision": "approve",
                "clientRequestId": "net-approval-own",
            }),
            vec![json!({
                "approvalId": b.approval,
                "expectedVersion": version("agent_approvals", b.approval),
                "decision": "approve",
                "clientRequestId": "net-approval-b",
            })],
        ),
    ]
}
