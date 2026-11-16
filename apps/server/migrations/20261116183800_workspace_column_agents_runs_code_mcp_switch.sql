-- H1.7 (build 3R): the workspace column on agent connections, execution, runs, approvals, automations, GitHub, repositories, MCP credentials and project memberships, switch step (H1 3.3).
-- Finishes the backfill inline (whether or not the job ran), makes
-- `workspace_id` NOT NULL through a validated CHECK, validates its foreign
-- key and indexes it where no index leads with it. Parents first.

SELECT tenancy_switch('project_memberships');
SELECT tenancy_switch('provider_connections');
SELECT tenancy_switch('execution_profiles');
SELECT tenancy_switch('execution_workers');
SELECT tenancy_switch('repositories');
SELECT tenancy_switch('repository_workspaces');
SELECT tenancy_switch('git_worktrees');
SELECT tenancy_switch('provider_sessions');
SELECT tenancy_switch('agent_runs');
SELECT tenancy_switch('agent_run_events');
SELECT tenancy_switch('agent_run_acknowledgements');
SELECT tenancy_switch('external_agent_connections');
SELECT tenancy_switch('external_agent_event_counters');
SELECT tenancy_switch('external_agent_events');
SELECT tenancy_switch('external_execution_requests');
SELECT tenancy_switch('agent_approvals');
SELECT tenancy_switch('agent_setup_links');
SELECT tenancy_switch('agent_webhook_deliveries');
SELECT tenancy_switch('ai_plan_import_items');
SELECT tenancy_switch('automation_rule_matches');
SELECT tenancy_switch('automation_runs');
SELECT tenancy_switch('capacity_alert_fires');
SELECT tenancy_switch('github_installations');
SELECT tenancy_switch('github_installation_repositories');
SELECT tenancy_switch('github_pull_requests');
SELECT tenancy_switch('github_jobs');
SELECT tenancy_switch('github_pull_request_automation_results');
SELECT tenancy_switch('github_pull_request_check_runs');
SELECT tenancy_switch('github_pull_request_conversation_links');
SELECT tenancy_switch('github_pull_request_dismissed_links');
SELECT tenancy_switch('github_pull_request_requests');
SELECT tenancy_switch('github_pull_request_reviews');
SELECT tenancy_switch('github_pull_request_work_item_links');
SELECT tenancy_switch('github_pull_request_workspace_links');
SELECT tenancy_switch('github_webhook_deliveries');
SELECT tenancy_switch('project_github_automation_settings');
SELECT tenancy_switch('mcp_client_credentials');
SELECT tenancy_switch('mcp_oauth_grants');
SELECT tenancy_switch('mcp_oauth_authorization_codes');
SELECT tenancy_switch('mcp_oauth_access_tokens');
SELECT tenancy_switch('mcp_oauth_refresh_tokens');
