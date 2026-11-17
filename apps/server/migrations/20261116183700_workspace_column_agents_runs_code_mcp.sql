-- H1.7 (build 3R): the workspace column on agent connections, execution, runs, approvals, automations, GitHub, repositories, MCP credentials and project memberships, expand step (H1 3.3).
-- Each table gets `workspace_id` (nullable for now, foreign key NOT VALID),
-- the fill trigger that sets it on insert from the row's project or
-- parent, and a `tenancy.backfill` job for the rows there are now. The
-- switch migration after this one finishes the copy inline and makes the
-- column required. Parents come before their children.

SELECT tenancy_expand('project_memberships');
SELECT tenancy_expand('provider_connections');
SELECT tenancy_expand('execution_profiles');
SELECT tenancy_expand('execution_workers');
SELECT tenancy_expand('repositories');
SELECT tenancy_expand('repository_workspaces');
SELECT tenancy_expand('git_worktrees', 'repository_workspaces', 'repository_workspace_id', 'workspace_id');
SELECT tenancy_expand('provider_sessions');
SELECT tenancy_expand('agent_runs');
SELECT tenancy_expand('agent_run_events', 'agent_runs', 'agent_run_id', 'workspace_id');
SELECT tenancy_expand('agent_run_acknowledgements');
SELECT tenancy_expand('external_agent_connections');
SELECT tenancy_expand('external_agent_event_counters', 'external_agent_connections', 'connection_id', 'workspace_id');
SELECT tenancy_expand('external_agent_events');
SELECT tenancy_expand('external_execution_requests');
SELECT tenancy_expand('agent_approvals', 'agent_runs', 'agent_run_id', 'workspace_id', 'external_execution_requests', 'external_execution_request_id', 'workspace_id');
SELECT tenancy_expand('agent_setup_links');
SELECT tenancy_expand('agent_webhook_deliveries', 'external_agent_connections', 'connection_id', 'workspace_id');
SELECT tenancy_expand('ai_plan_import_items', 'ai_plan_imports', 'import_id', 'organization_id');
SELECT tenancy_expand('automation_rule_matches', 'automation_rules', 'rule_id', 'organization_id');
SELECT tenancy_expand('automation_runs');
SELECT tenancy_expand('capacity_alert_fires', 'provider_accounts', 'account_id', 'organization_id');
SELECT tenancy_expand('github_installations');
SELECT tenancy_expand('github_installation_repositories');
SELECT tenancy_expand('github_pull_requests');
SELECT tenancy_expand('github_jobs');
SELECT tenancy_expand('github_pull_request_automation_results');
SELECT tenancy_expand('github_pull_request_check_runs');
SELECT tenancy_expand('github_pull_request_conversation_links');
SELECT tenancy_expand('github_pull_request_dismissed_links');
SELECT tenancy_expand('github_pull_request_requests');
SELECT tenancy_expand('github_pull_request_reviews');
SELECT tenancy_expand('github_pull_request_work_item_links');
SELECT tenancy_expand('github_pull_request_workspace_links');
SELECT tenancy_expand('github_webhook_deliveries');
SELECT tenancy_expand('project_github_automation_settings');
SELECT tenancy_expand('mcp_client_credentials');
SELECT tenancy_expand('mcp_oauth_grants');
SELECT tenancy_expand('mcp_oauth_authorization_codes');
SELECT tenancy_expand('mcp_oauth_access_tokens', 'mcp_oauth_grants', 'grant_id', 'workspace_id');
SELECT tenancy_expand('mcp_oauth_refresh_tokens', 'mcp_oauth_grants', 'grant_id', 'workspace_id');
