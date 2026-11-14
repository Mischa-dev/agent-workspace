//! Every server integration test, built as one binary so the server is
//! compiled and linked for tests once rather than once per file.

mod a1_host_protocol;
mod a1_launch_paths;
mod a1_requests;
mod a1_run_reporting;
mod a1_runs_record;
mod agent_interface;
mod awd102_task_move;
mod awd110_collection_forms;
mod awd110_collection_views;
mod awd110_records_everywhere;
mod awd110_task_like_collections;
mod awd110_task_records;
mod awd110_tasks_collection;
mod awd53_attribution;
mod awd53_discussion;
mod awd53_live_pages;
mod awd53_page_blocks;
mod awd53_transfer;
mod awd54_sheet_files;
mod awd54_sheets;
mod awd58_references;
mod awd62_ask_person;
mod awd63_setup_links;
mod awd76_workspace_templates;
mod awd89_custom_fields;
mod awd90_collections;
mod awd91_agent_parity;
mod awd95_comment_replies;
mod awd96_workspace_pages;
mod backbone;
mod bug_report_links;
mod bug_sweep;
mod direct_conversations;
mod github_task_keys;
mod harness_support;
mod journey;
mod layouts;
mod m0_database;
mod m1_api_policy_database;
mod m1_bootstrap_database;
mod m1_http_projects;
mod m1_identity_projects_database;
mod m1_realtime_websocket;
mod m1_recovery_provider_sessions;
mod m2_actor_realtime_convergence;
mod m2_concurrent_commands;
mod m2_conversations_database;
mod m2_conversations_http;
mod m2_outbox_publisher;
mod m2_provider_session_terminal_state;
mod m2_start_failure;
mod m3_account_http;
mod m3_attention_isolation;
mod m3_execution_foundation_database;
mod m3_execution_http;
mod m3_repositories_http;
mod m3_repository_mirror_upgrade;
mod m3_scheduler_http;
mod m4_work_items_http;
mod m5_github_foundation;
mod m6_mcp_foundation;
mod m7_execution_profiles;
mod m7_invitations;
mod m7_memberships;
mod m9_external_agents;
mod mcp_admin;
mod mcp_code;
mod mcp_groundwork;
mod mcp_groups;
mod mcp_labels;
mod mcp_planning;
mod mcp_revisions;
mod mcp_task_batches;
mod mcp_task_board;
mod mcp_task_criteria;
mod mcp_tasks;
mod message_edits;
mod message_reactions;
mod message_replies;
mod modules;
mod modules_ids;
mod modules_lists;
mod modules_targets;
mod notifications;
mod oauth_agent;
mod ops_decisions;
mod ops_edges;
mod ops_kernel;
mod packages;
mod phase3_takedown;
mod phase4_groundwork;
mod polish_leftovers;
mod reply_requests;
mod review_sweep;
mod roles_and_access;
mod server_operators;
mod settings_accounts;
mod w10_ai_planning;
mod w11_views_and_data;
mod w12_harnesses;
mod w13_payoff;
mod w14_automations_and_api;
mod w16_whiteboards;
mod w18_phone_first;
mod w19_self_hosting;
mod w20_files;
mod w21_later_and_scheduling;
mod w22_agent_kit;
mod w2_harness_platform;
mod w3_tasks_http;
mod w7_launch;
mod w9_capacity_and_push;
mod workspace_isolation;
mod write_isolation;

/// The PostgreSQL tests skip themselves when `AGENT_WORKSPACE_DATABASE_URL`
/// is unset, so without this a run with no database would pass with most of
/// the suite never run. Skipping them is a choice you make out loud.
#[test]
fn postgres_tests_are_never_skipped_silently() {
    let configured = std::env::var("AGENT_WORKSPACE_DATABASE_URL").is_ok_and(|url| !url.is_empty());
    let skipping =
        std::env::var_os("AGENT_WORKSPACE_SKIP_DATABASE_TESTS").is_some_and(|value| value == "1");
    assert!(
        configured || skipping,
        "AGENT_WORKSPACE_DATABASE_URL is unset, so every PostgreSQL test skipped itself. \
         Point it at a database (tools/cloud/setup.sh does in a cloud session), or set \
         AGENT_WORKSPACE_SKIP_DATABASE_TESTS=1 to skip them on purpose."
    );
}
