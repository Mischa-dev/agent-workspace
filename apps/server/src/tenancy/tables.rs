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
//!   yet, with the slice that adds it. This list only shrinks: H1.4 to H1.7
//!   empty it.
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
    ("agent_profiles", ORGANIZATION_ID),
    ("agent_run_usage", ORGANIZATION_ID),
    ("agent_sessions", ORGANIZATION_ID),
    ("ai_plan_imports", ORGANIZATION_ID),
    ("ai_project_scans", ORGANIZATION_ID),
    ("ai_runs", ORGANIZATION_ID),
    ("ai_suggestions", ORGANIZATION_ID),
    ("ai_usage", ORGANIZATION_ID),
    ("audit_events", ORGANIZATION_ID),
    ("automation_rules", ORGANIZATION_ID),
    ("background_jobs", ORGANIZATION_ID),
    ("bug_report_links", ORGANIZATION_ID),
    ("capacity_alert_rules", ORGANIZATION_ID),
    ("custom_harnesses", ORGANIZATION_ID),
    ("files", ORGANIZATION_ID),
    ("github_connect_attempts", ORGANIZATION_ID),
    ("github_installation_claims", ORGANIZATION_ID),
    ("harness_catalog_refreshes", ORGANIZATION_ID),
    ("harness_models", ORGANIZATION_ID),
    ("launches", ORGANIZATION_ID),
    ("layout_presets", ORGANIZATION_ID),
    ("link_previews", ORGANIZATION_ID),
    ("notification_action_tokens", ORGANIZATION_ID),
    ("notification_channel_preferences", ORGANIZATION_ID),
    ("notification_email_digests", ORGANIZATION_ID),
    ("onboarding_progress", ORGANIZATION_ID),
    ("organization_actor_profiles", ORGANIZATION_ID),
    ("organization_memberships", ORGANIZATION_ID),
    ("project_environments", ORGANIZATION_ID),
    ("project_invitations", ORGANIZATION_ID),
    ("project_layout_rules", ORGANIZATION_ID),
    ("project_templates", ORGANIZATION_ID),
    ("projects", ORGANIZATION_ID),
    ("provider_accounts", ORGANIZATION_ID),
    ("push_subscriptions", ORGANIZATION_ID),
    ("saved_items", ORGANIZATION_ID),
    ("scheduled_messages", ORGANIZATION_ID),
    ("secrets", ORGANIZATION_ID),
    ("settings", ORGANIZATION_ID),
    ("usage_accounts", ORGANIZATION_ID),
    ("usage_schedules", ORGANIZATION_ID),
    ("usage_window_cycles", ORGANIZATION_ID),
    ("usage_window_samples", ORGANIZATION_ID),
    ("usage_windows", ORGANIZATION_ID),
    ("webhook_deliveries", ORGANIZATION_ID),
    ("webhook_subscriptions", ORGANIZATION_ID),
    ("whiteboards", ORGANIZATION_ID),
    ("work_item_key_aliases", ORGANIZATION_ID),
    ("work_item_move_receipts", ORGANIZATION_ID),
    ("workos_agent_bindings", ORGANIZATION_ID),
    ("workos_organizations", ORGANIZATION_ID),
    ("workspace_invitations", ORGANIZATION_ID),
    ("workspace_mail_settings", ORGANIZATION_ID),
    ("workspace_roles", ORGANIZATION_ID),
    ("workspace_template_uses", ORGANIZATION_ID),
    ("workspace_templates", ORGANIZATION_ID),
];

/// Server-wide tables, each with the reason it belongs to no workspace.
pub const GLOBAL_TABLES: &[(&str, &str)] = &[
    ("_sqlx_migrations", "the schema's migration ledger"),
    (
        "actors",
        "every account, of any kind: accounts are server-wide (R19), each with its owner",
    ),
    (
        "login_challenges",
        "two-factor sign-in challenges, before any workspace is known",
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
        "secret_master_keys_retired",
        "the server's own key history, not any workspace's",
    ),
    (
        "sessions",
        "browser sign-in sessions, before any workspace is chosen",
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
    "agent_outside_refs",
    "agent_connections",
    "connection_signing_secrets",
    "hosts",
    "host_agents",
    "host_enrolments",
];

/// Workspace-owned tables still without a workspace column, and the slice
/// that adds it. Only ever shrinks.
pub const MISSING_WORKSPACE_COLUMN: &[(&str, &str)] = &[
    ("agent_approvals", "H1.7"),
    ("agent_run_acknowledgements", "H1.7"),
    ("agent_run_events", "H1.7"),
    ("agent_runs", "H1.7"),
    ("agent_setup_links", "H1.7"),
    ("agent_webhook_deliveries", "H1.7"),
    ("ai_plan_import_items", "H1.7"),
    ("automation_rule_matches", "H1.7"),
    ("automation_runs", "H1.7"),
    ("capacity_alert_fires", "H1.7"),
    ("collection_record_history", "H1.6"),
    ("collection_records", "H1.6"),
    ("collection_views", "H1.6"),
    ("collections", "H1.6"),
    ("conversation_entries", "H1.4"),
    ("conversation_members", "H1.4"),
    ("conversation_notification_levels", "H1.4"),
    ("conversation_pins", "H1.4"),
    ("conversation_read_states", "H1.4"),
    ("conversations", "H1.4"),
    ("execution_profiles", "H1.7"),
    ("execution_workers", "H1.7"),
    ("external_agent_connections", "H1.7"),
    ("external_agent_event_counters", "H1.7"),
    ("external_agent_events", "H1.7"),
    ("external_execution_requests", "H1.7"),
    ("field_definitions", "H1.5"),
    ("git_worktrees", "H1.7"),
    ("github_installation_repositories", "H1.7"),
    ("github_installations", "H1.7"),
    ("github_jobs", "H1.7"),
    ("github_pull_request_automation_results", "H1.7"),
    ("github_pull_request_check_runs", "H1.7"),
    ("github_pull_request_conversation_links", "H1.7"),
    ("github_pull_request_dismissed_links", "H1.7"),
    ("github_pull_request_requests", "H1.7"),
    ("github_pull_request_reviews", "H1.7"),
    ("github_pull_request_work_item_links", "H1.7"),
    ("github_pull_request_workspace_links", "H1.7"),
    ("github_pull_requests", "H1.7"),
    ("github_webhook_deliveries", "H1.7"),
    ("idempotency_keys", "H1.7"),
    ("mcp_client_credentials", "H1.7"),
    ("mcp_oauth_access_tokens", "H1.7"),
    ("mcp_oauth_authorization_codes", "H1.7"),
    ("mcp_oauth_grants", "H1.7"),
    ("mcp_oauth_refresh_tokens", "H1.7"),
    ("message_link_previews", "H1.4"),
    ("message_mentions", "H1.4"),
    ("message_reactions", "H1.4"),
    ("message_revisions", "H1.4"),
    ("message_work_item_references", "H1.4"),
    ("messages", "H1.4"),
    ("notification_occurrences", "H1.4"),
    ("notification_project_mutes", "H1.4"),
    ("notifications", "H1.4"),
    ("outbox_events", "H1.4"),
    ("person_questions", "H1.4"),
    ("project_counters", "H1.5"),
    ("project_decisions", "H1.4"),
    ("project_github_automation_settings", "H1.7"),
    ("project_instructions", "H1.6"),
    ("project_labels", "H1.5"),
    ("project_memberships", "H1.7"),
    ("project_milestones", "H1.5"),
    ("project_navigation_states", "H1.5"),
    ("project_page_comments", "H1.6"),
    ("project_page_documents", "H1.6"),
    ("project_page_revisions", "H1.6"),
    ("project_page_suggestions", "H1.6"),
    ("project_pages", "H1.6"),
    ("project_task_settings", "H1.5"),
    ("provider_connections", "H1.7"),
    ("provider_sessions", "H1.7"),
    ("push_deliveries", "H1.4"),
    ("recurring_task_occurrences", "H1.5"),
    ("recurring_tasks", "H1.5"),
    ("reply_requests", "H1.4"),
    ("repositories", "H1.7"),
    ("repository_workspaces", "H1.7"),
    ("saved_view_pins", "H1.5"),
    ("saved_views", "H1.5"),
    ("sheet_cells", "H1.6"),
    ("sheet_cleared_cells", "H1.6"),
    ("sheet_revisions", "H1.6"),
    ("sheet_tabs", "H1.6"),
    ("sheets", "H1.6"),
    ("task_import_links", "H1.5"),
    ("task_imports", "H1.5"),
    ("task_templates", "H1.5"),
    ("thing_references", "H1.4"),
    ("whiteboard_elements", "H1.6"),
    ("whiteboard_files", "H1.6"),
    ("whiteboard_links", "H1.6"),
    ("whiteboard_revisions", "H1.6"),
    ("work_item_acceptance_criteria", "H1.5"),
    ("work_item_activity", "H1.5"),
    ("work_item_claims", "H1.5"),
    ("work_item_comment_mentions", "H1.5"),
    ("work_item_comment_references", "H1.5"),
    ("work_item_comment_revisions", "H1.5"),
    ("work_item_comments", "H1.5"),
    ("work_item_conversation_links", "H1.5"),
    ("work_item_dependencies", "H1.5"),
    ("work_item_handoffs", "H1.5"),
    ("work_item_labels", "H1.5"),
    ("work_item_links", "H1.5"),
    ("work_item_relations", "H1.5"),
    ("work_item_repository_links", "H1.5"),
    ("work_item_status_transitions", "H1.5"),
    ("work_item_statuses", "H1.5"),
    ("work_item_timeline_entries", "H1.5"),
    ("work_item_types", "H1.5"),
    ("work_item_watchers", "H1.5"),
    ("work_item_worker_invocations", "H1.5"),
    ("work_items", "H1.5"),
];

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
