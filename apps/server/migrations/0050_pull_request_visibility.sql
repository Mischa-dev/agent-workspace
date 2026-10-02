-- Phase 4 groundwork (docs/plans/mcp-tools.md). Phase 3 ended at 0049.
--
-- Who may see a pull request: it follows the thread the run that opened it
-- happened in, so a PR from a private DM or group thread stays private. A PR
-- with no run_artifact link (one found by webhook, or opened outside Agent
-- Workspace) is visible to everyone who can reach its project. Only
-- run_artifact links decide: an explicit link to a public thread must never
-- make a private PR public.
--
-- This checks the thread only. Every query that uses it must also keep
-- pull_request.project_id among the projects the viewer can reach.
CREATE FUNCTION pull_request_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT NOT EXISTS (
               SELECT 1 FROM github_pull_request_conversation_links link
               WHERE link.github_pull_request_id = target
                 AND link.source = 'run_artifact')
        OR EXISTS (
               SELECT 1 FROM github_pull_request_conversation_links link
               WHERE link.github_pull_request_id = target
                 AND link.source = 'run_artifact'
                 AND conversation_visible_to(link.conversation_id, viewer))
$$;

-- A person's GitHub username, so "pull requests that need me" can include
-- the ones asking for their review. It is self-declared and not unique: a
-- personal filter only, never used for authorization or attribution.
ALTER TABLE users ADD COLUMN github_login TEXT CHECK (
    github_login IS NULL
    OR github_login ~ '^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$'
);
