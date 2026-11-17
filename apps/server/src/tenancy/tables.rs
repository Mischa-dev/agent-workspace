//! Which workspace every table belongs to (H1.3).
//!
//! Row rules, the workspace export, the isolation guards and the round-trip
//! test read a table's tenant column from here rather than guessing it: the
//! tables that had one keep `organization_id`, and the column H1.4 to H1.7
//! add (and that new tables are born with) is named `workspace_id` (ruling
//! R1), so both names are listed per table.
//!
//! Every table in the schema is in exactly one list:
//! - [`TENANT_TABLES`]: the table and its tenant column;
//! - [`GLOBAL_TABLES`]: server-wide, with the reason (accounts of every kind,
//!   credentials, sign-in state, hosts);
//! - [`MISSING_WORKSPACE_COLUMN`]: workspace-owned but without its column
//!   yet, with the slice that adds it. H1.4 to H1.7 (build 3R) emptied it;
//!   it stays empty.
//!
//! `tests/integration/tenancy_tables.rs` reads `information_schema` and
//! fails on a table in none of the lists, in two, or listed with a column it
//! doesn't have. A new table goes in [`TENANT_TABLES`] with a `workspace_id
//! NOT NULL` column from birth, or in [`GLOBAL_TABLES`] with its reason.

/// The tenant column a table names its workspace by.
pub const ORGANIZATION_ID: &str = "organization_id";
/// The column H1.4 to H1.7 add, and new tables are born with (R1).
pub const WORKSPACE_ID: &str = "workspace_id";

/// Workspace-owned tables and their tenant column.
pub const TENANT_TABLES: &[(&str, &str)] = &[
    ("agent_approvals", WORKSPACE_ID),
    ("agent_profiles", ORGANIZATION_ID),
    ("agent_run_acknowledgements", WORKSPACE_ID),
    ("agent_run_events", WORKSPACE_ID),
    ("agent_run_usage", ORGANIZATION_ID),
    ("agent_runs", WORKSPACE_ID),
    ("agent_sessions", ORGANIZATION_ID),
    ("agent_setup_links", WORKSPACE_ID),
    ("agent_webhook_deliveries", WORKSPACE_ID),
    ("ai_plan_import_items", WORKSPACE_ID),
    ("ai_plan_imports", ORGANIZATION_ID),
    ("ai_project_scans", ORGANIZATION_ID),
    ("ai_runs", ORGANIZATION_ID),
    ("ai_suggestions", ORGANIZATION_ID),
    ("ai_usage", ORGANIZATION_ID),
    ("audit_events", ORGANIZATION_ID),
    ("automation_rule_matches", WORKSPACE_ID),
    ("automation_rules", ORGANIZATION_ID),
    ("automation_runs", WORKSPACE_ID),
    ("background_jobs", ORGANIZATION_ID),
    ("bug_report_links", ORGANIZATION_ID),
    ("capacity_alert_fires", WORKSPACE_ID),
    ("capacity_alert_rules", ORGANIZATION_ID),
    ("collection_record_history", WORKSPACE_ID),
    ("collection_records", WORKSPACE_ID),
    ("collection_views", WORKSPACE_ID),
    ("collections", WORKSPACE_ID),
    ("conversation_entries", WORKSPACE_ID),
    ("conversation_members", WORKSPACE_ID),
    ("conversation_notification_levels", WORKSPACE_ID),
    ("conversation_pins", WORKSPACE_ID),
    ("conversation_read_states", WORKSPACE_ID),
    ("conversations", WORKSPACE_ID),
    ("custom_harnesses", ORGANIZATION_ID),
    ("event_staging", WORKSPACE_ID),
    ("events", WORKSPACE_ID),
    ("events_default", WORKSPACE_ID),
    ("events_p0", WORKSPACE_ID),
    ("execution_profiles", WORKSPACE_ID),
    ("execution_workers", WORKSPACE_ID),
    ("external_agent_connections", WORKSPACE_ID),
    ("external_agent_event_counters", WORKSPACE_ID),
    ("external_agent_events", WORKSPACE_ID),
    ("external_execution_requests", WORKSPACE_ID),
    ("field_definitions", WORKSPACE_ID),
    ("files", ORGANIZATION_ID),
    ("git_worktrees", WORKSPACE_ID),
    ("github_connect_attempts", ORGANIZATION_ID),
    ("github_installation_claims", ORGANIZATION_ID),
    ("github_installation_repositories", WORKSPACE_ID),
    ("github_installations", WORKSPACE_ID),
    ("github_jobs", WORKSPACE_ID),
    ("github_pull_request_automation_results", WORKSPACE_ID),
    ("github_pull_request_check_runs", WORKSPACE_ID),
    ("github_pull_request_conversation_links", WORKSPACE_ID),
    ("github_pull_request_dismissed_links", WORKSPACE_ID),
    ("github_pull_request_requests", WORKSPACE_ID),
    ("github_pull_request_reviews", WORKSPACE_ID),
    ("github_pull_request_work_item_links", WORKSPACE_ID),
    ("github_pull_request_workspace_links", WORKSPACE_ID),
    ("github_pull_requests", WORKSPACE_ID),
    ("github_webhook_deliveries", WORKSPACE_ID),
    ("harness_catalog_refreshes", ORGANIZATION_ID),
    ("harness_models", ORGANIZATION_ID),
    ("host_commands", WORKSPACE_ID),
    ("launches", ORGANIZATION_ID),
    ("layout_presets", ORGANIZATION_ID),
    ("link_previews", ORGANIZATION_ID),
    ("links", WORKSPACE_ID),
    ("mcp_client_credentials", WORKSPACE_ID),
    ("mcp_oauth_access_tokens", WORKSPACE_ID),
    ("mcp_oauth_authorization_codes", WORKSPACE_ID),
    ("mcp_oauth_grants", WORKSPACE_ID),
    ("mcp_oauth_refresh_tokens", WORKSPACE_ID),
    ("message_link_previews", WORKSPACE_ID),
    ("message_mentions", WORKSPACE_ID),
    ("message_reactions", WORKSPACE_ID),
    ("message_revisions", WORKSPACE_ID),
    ("message_work_item_references", WORKSPACE_ID),
    ("messages", WORKSPACE_ID),
    ("module_installs", WORKSPACE_ID),
    ("notification_action_tokens", ORGANIZATION_ID),
    ("notification_channel_preferences", ORGANIZATION_ID),
    ("notification_email_digests", ORGANIZATION_ID),
    ("notification_occurrences", WORKSPACE_ID),
    ("notification_project_mutes", WORKSPACE_ID),
    ("notifications", WORKSPACE_ID),
    ("onboarding_progress", ORGANIZATION_ID),
    ("operation_alias_usage", WORKSPACE_ID),
    ("organization_actor_profiles", ORGANIZATION_ID),
    ("organization_memberships", ORGANIZATION_ID),
    ("project_counters", WORKSPACE_ID),
    ("project_decisions", WORKSPACE_ID),
    ("project_environments", ORGANIZATION_ID),
    ("project_github_automation_settings", WORKSPACE_ID),
    ("project_instructions", WORKSPACE_ID),
    ("project_invitations", ORGANIZATION_ID),
    ("project_labels", WORKSPACE_ID),
    ("project_layout_rules", ORGANIZATION_ID),
    ("project_memberships", WORKSPACE_ID),
    ("project_milestones", WORKSPACE_ID),
    ("project_navigation_states", WORKSPACE_ID),
    ("project_page_comments", WORKSPACE_ID),
    ("project_page_documents", WORKSPACE_ID),
    ("project_page_revisions", WORKSPACE_ID),
    ("project_page_suggestions", WORKSPACE_ID),
    ("project_pages", WORKSPACE_ID),
    ("project_task_settings", WORKSPACE_ID),
    ("project_templates", ORGANIZATION_ID),
    ("projects", ORGANIZATION_ID),
    ("provider_accounts", ORGANIZATION_ID),
    ("provider_connections", WORKSPACE_ID),
    ("provider_sessions", WORKSPACE_ID),
    ("push_deliveries", WORKSPACE_ID),
    ("push_subscriptions", ORGANIZATION_ID),
    ("recurring_task_occurrences", WORKSPACE_ID),
    ("recurring_tasks", WORKSPACE_ID),
    ("reply_requests", WORKSPACE_ID),
    ("repositories", WORKSPACE_ID),
    ("repository_workspaces", WORKSPACE_ID),
    ("requests", WORKSPACE_ID),
    ("run_activities", WORKSPACE_ID),
    ("run_credentials", WORKSPACE_ID),
    ("run_usage", WORKSPACE_ID),
    ("runs", WORKSPACE_ID),
    ("saved_items", ORGANIZATION_ID),
    ("saved_view_pins", WORKSPACE_ID),
    ("saved_views", WORKSPACE_ID),
    ("scheduled_messages", ORGANIZATION_ID),
    ("secrets", ORGANIZATION_ID),
    ("settings", ORGANIZATION_ID),
    ("sheet_cells", WORKSPACE_ID),
    ("sheet_cleared_cells", WORKSPACE_ID),
    ("sheet_revisions", WORKSPACE_ID),
    ("sheet_tabs", WORKSPACE_ID),
    ("sheets", WORKSPACE_ID),
    ("task_import_links", WORKSPACE_ID),
    ("task_imports", WORKSPACE_ID),
    ("task_templates", WORKSPACE_ID),
    ("thing_references", WORKSPACE_ID),
    ("things", WORKSPACE_ID),
    ("usage_accounts", ORGANIZATION_ID),
    ("usage_reports", WORKSPACE_ID),
    ("usage_schedules", ORGANIZATION_ID),
    ("usage_window_cycles", ORGANIZATION_ID),
    ("usage_window_samples", ORGANIZATION_ID),
    ("usage_windows", ORGANIZATION_ID),
    ("webhook_deliveries", ORGANIZATION_ID),
    ("webhook_subscriptions", ORGANIZATION_ID),
    ("whiteboard_elements", WORKSPACE_ID),
    ("whiteboard_files", WORKSPACE_ID),
    ("whiteboard_links", WORKSPACE_ID),
    ("whiteboard_revisions", WORKSPACE_ID),
    ("whiteboards", ORGANIZATION_ID),
    ("work_item_acceptance_criteria", WORKSPACE_ID),
    ("work_item_activity", WORKSPACE_ID),
    ("work_item_claims", WORKSPACE_ID),
    ("work_item_comment_mentions", WORKSPACE_ID),
    ("work_item_comment_references", WORKSPACE_ID),
    ("work_item_comment_revisions", WORKSPACE_ID),
    ("work_item_comments", WORKSPACE_ID),
    ("work_item_conversation_links", WORKSPACE_ID),
    ("work_item_dependencies", WORKSPACE_ID),
    ("work_item_handoffs", WORKSPACE_ID),
    ("work_item_key_aliases", ORGANIZATION_ID),
    ("work_item_labels", WORKSPACE_ID),
    ("work_item_links", WORKSPACE_ID),
    ("work_item_move_receipts", ORGANIZATION_ID),
    ("work_item_relations", WORKSPACE_ID),
    ("work_item_repository_links", WORKSPACE_ID),
    ("work_item_status_transitions", WORKSPACE_ID),
    ("work_item_statuses", WORKSPACE_ID),
    ("work_item_timeline_entries", WORKSPACE_ID),
    ("work_item_types", WORKSPACE_ID),
    ("work_item_watchers", WORKSPACE_ID),
    ("work_item_worker_invocations", WORKSPACE_ID),
    ("work_items", WORKSPACE_ID),
    ("workos_agent_bindings", ORGANIZATION_ID),
    ("workos_organizations", ORGANIZATION_ID),
    ("workspace_invitations", ORGANIZATION_ID),
    ("workspace_mail_settings", ORGANIZATION_ID),
    ("workspace_roles", ORGANIZATION_ID),
    ("workspace_slugs", WORKSPACE_ID),
    ("workspace_template_uses", ORGANIZATION_ID),
    ("workspace_templates", ORGANIZATION_ID),
];

/// Server-wide tables, each with the reason it belongs to no workspace.
pub const GLOBAL_TABLES: &[(&str, &str)] = &[
    ("_sqlx_migrations", "the schema's migration ledger"),
    (
        "access_disagreements",
        "the server's log of where the old and new access checks differ during K1's cutover; a row names a workspace when there is one",
    ),
    (
        "account_managers",
        "who manages an agent or app account: accounts and their naming are server-wide (R19)",
    ),
    (
        "actors",
        "every account, of any kind: accounts are server-wide (R19), each with its owner",
    ),
    (
        "event_consumer_failures",
        "events a consumer of the server's log could not handle yet, by log position",
    ),
    (
        "event_consumers",
        "how far each of the server's log consumers has read (K3.3)",
    ),
    (
        "event_position_counter",
        "the one row that numbers the server's event log (K3.3)",
    ),
    (
        "host_agents",
        "which accounts a host runs: hosts are server-wide (A1 3.9)",
    ),
    (
        "host_enrolments",
        "one-time codes that enrol a host, kept as digests: hosts are server-wide",
    ),
    (
        "host_notices",
        "what a host is told about its agents: hosts are server-wide",
    ),
    (
        "hosts",
        "machines that run agents, owned by an account or a workspace (A1 3.9)",
    ),
    (
        "idempotency_keys",
        "an account's retry keys: keyed by the account (accounts are server-wide, R19), short-lived, and looked up before the request's workspace is known",
    ),
    (
        "leases",
        "which server instance holds each single-holder piece of server work (K5.2)",
    ),
    (
        "login_challenges",
        "two-factor sign-in challenges, before any workspace is known",
    ),
    (
        "maintenance_leases",
        "leases on the server's own maintenance (backups, key rotation)",
    ),
    (
        "mcp_oauth_clients",
        "dynamic client registration happens before anyone signs in",
    ),
    (
        "organizations",
        "the workspaces themselves: a row is its own tenant (its id)",
    ),
    ("password_reset_tokens", "account mail tokens, server-wide"),
    (
        "permission_catalogue",
        "the permissions the server knows, the same for every workspace (K1.3)",
    ),
    (
        "rate_limit_counters",
        "short-lived request budgets keyed by digests; sign-in budgets count before any workspace is known, and a per-workspace bucket carries workspace_id (K5.6)",
    ),
    (
        "secret_master_keys_retired",
        "the server's own key history, not any workspace's",
    ),
    (
        "mcp_ended_sessions",
        "digests of MCP session ids their clients ended; a session belongs to a connection, not a workspace (K5.7)",
    ),
    (
        "server_instances",
        "the server processes sharing this database and their heartbeats (K5.2)",
    ),
    (
        "sessions",
        "browser sign-in sessions, before any workspace is chosen",
    ),
    (
        "thing_kinds",
        "the kinds of thing the server knows: a kind is the server's, not a workspace's (K3.10)",
    ),
    (
        "user_email_addresses",
        "a person's own addresses, server-wide",
    ),
    (
        "user_password_credentials",
        "a credential: server-wide and never exported",
    ),
    (
        "user_recovery_codes",
        "a credential: server-wide and never exported",
    ),
    (
        "user_totp_factors",
        "a credential: server-wide and never exported",
    ),
    ("users", "a person's sign-in account, server-wide"),
    (
        "workos_identities",
        "sign-in provider identities, server-wide",
    ),
    (
        "workos_login_attempts",
        "sign-in ceremonies, before any account is known",
    ),
];

/// Tables that must stay global whenever they exist: accounts, their
/// credentials, sign-in state and hosts (R19, A1 3.9). Listing one of them
/// as a tenant table fails the guard. Names not created yet are checked once
/// they are.
pub const MUST_BE_GLOBAL: &[&str] = &[
    "actors",
    "users",
    "sessions",
    "login_challenges",
    "password_reset_tokens",
    "user_email_addresses",
    "user_password_credentials",
    "user_recovery_codes",
    "user_totp_factors",
    "workos_identities",
    "workos_login_attempts",
    "mcp_oauth_clients",
    "account_credentials",
    "account_managers",
    "agent_outside_refs",
    "agent_connections",
    "connection_signing_secrets",
    "hosts",
    "host_agents",
    "host_enrolments",
    "host_notices",
];

/// Workspace-owned tables still without a workspace column, and the slice
/// that adds it. Empty since H1.4 to H1.7 gave every one its `workspace_id`
/// (filled on insert by `<table>_fill_workspace`); a new table is born with
/// the column instead.
pub const MISSING_WORKSPACE_COLUMN: &[(&str, &str)] = &[];

/// Tables that may hold rows of the direct scope (DMs and groups, R20):
/// conversations and what hangs off them. Row rules give these a
/// participants-only branch; no membership or role row may name the direct
/// scope.
pub const DIRECT_SCOPE_TABLES: &[&str] = &[
    "conversations",
    "conversation_members",
    "conversation_read_states",
    "conversation_notification_levels",
    "conversation_entries",
    "conversation_pins",
    "messages",
    "message_mentions",
    "message_reactions",
    "message_revisions",
    "message_link_previews",
    "message_work_item_references",
    "reply_requests",
    "files",
];

/// A table's tenant column, if it has one yet.
pub fn tenant_column(table: &str) -> Option<&'static str> {
    TENANT_TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, column)| *column)
}

/// Whether a table is server-wide.
pub fn is_global(table: &str) -> bool {
    GLOBAL_TABLES.iter().any(|(name, _)| *name == table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_table_is_listed_once_and_every_global_table_says_why() {
        let mut seen = std::collections::BTreeSet::new();
        for table in TENANT_TABLES
            .iter()
            .map(|(table, _)| *table)
            .chain(GLOBAL_TABLES.iter().map(|(table, _)| *table))
            .chain(MISSING_WORKSPACE_COLUMN.iter().map(|(table, _)| *table))
        {
            assert!(seen.insert(table), "{table} is listed twice");
        }
        for (table, reason) in GLOBAL_TABLES {
            assert!(reason.len() > 10, "{table}: say why it is global");
        }
        for (table, column) in TENANT_TABLES {
            assert!(
                [ORGANIZATION_ID, WORKSPACE_ID].contains(column),
                "{table}: {column}"
            );
            assert!(!MUST_BE_GLOBAL.contains(table), "{table} must be global");
        }
        for (table, slice) in MISSING_WORKSPACE_COLUMN {
            assert!(
                ["H1.4", "H1.5", "H1.6", "H1.7"].contains(slice),
                "{table}: {slice}"
            );
        }
        assert_eq!(tenant_column("projects"), Some(ORGANIZATION_ID));
        assert!(is_global("actors"));
    }
}
