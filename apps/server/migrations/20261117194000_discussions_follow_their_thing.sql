-- A thing's discussion moves with it (Lane D2, follow-up from #287's
-- review). When a board (and later a page, C1.37) moves to another space or
-- to the workspace, its comments conversation, the threads under it and
-- everything hanging off their messages move with it: one UPDATE of the
-- comments conversation's project, carried down by the composite keys.
--
-- 1. Every composite key into conversations(project_id, ...) and
--    messages(project_id, ...) follows an update (ON UPDATE CASCADE). Ids
--    never change, so only a move ever cascades.
-- 2. The message tables' authors stop being tied to a membership of the
--    conversation's space (K4 retires the composite membership keys table
--    by table; these are the discussion ones). Who may write is decided by
--    access when they write; the key now says only that the account exists.
-- 3. The guards that refuse a change of project step aside while a move is
--    in progress (`discussion_move_in_progress()`, a transaction-local
--    setting only `discussion::follow` sets), as they do for workspace
--    deletion.

-- 1. Composite keys follow an update.
ALTER TABLE agent_runs
    DROP CONSTRAINT agent_runs_conversation_repository_fk,
    ADD CONSTRAINT agent_runs_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id) REFERENCES conversations(project_id, id, repository_id) ON UPDATE CASCADE;
ALTER TABLE agent_runs
    DROP CONSTRAINT agent_runs_prompt_message_fk,
    ADD CONSTRAINT agent_runs_prompt_message_fk
        FOREIGN KEY (project_id, prompt_message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE agent_sessions
    DROP CONSTRAINT agent_sessions_project_id_conversation_id_fkey,
    ADD CONSTRAINT agent_sessions_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE conversation_members
    DROP CONSTRAINT conversation_members_project_id_conversation_id_fkey,
    ADD CONSTRAINT conversation_members_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE conversation_read_states
    DROP CONSTRAINT conversation_read_states_project_id_conversation_id_fkey,
    ADD CONSTRAINT conversation_read_states_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE conversations
    DROP CONSTRAINT conversations_origin_message_project_fk,
    ADD CONSTRAINT conversations_origin_message_project_fk
        FOREIGN KEY (project_id, origin_message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE conversations
    DROP CONSTRAINT conversations_parent_project_fk,
    ADD CONSTRAINT conversations_parent_project_fk
        FOREIGN KEY (project_id, parent_channel_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE external_execution_requests
    DROP CONSTRAINT external_execution_requests_project_id_conversation_id_fkey,
    ADD CONSTRAINT external_execution_requests_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE files
    DROP CONSTRAINT files_project_id_conversation_id_fkey,
    ADD CONSTRAINT files_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE files
    DROP CONSTRAINT files_project_id_message_id_fkey,
    ADD CONSTRAINT files_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE github_pull_request_conversation_links
    DROP CONSTRAINT github_pull_request_conversatio_project_id_conversation_id_fkey,
    ADD CONSTRAINT github_pull_request_conversatio_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE launches
    DROP CONSTRAINT launches_project_id_conversation_id_fkey,
    ADD CONSTRAINT launches_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE launches
    DROP CONSTRAINT launches_project_id_source_message_id_fkey,
    ADD CONSTRAINT launches_project_id_source_message_id_fkey
        FOREIGN KEY (project_id, source_message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE message_link_previews
    DROP CONSTRAINT message_link_previews_project_id_message_id_fkey,
    ADD CONSTRAINT message_link_previews_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE message_mentions
    DROP CONSTRAINT message_mentions_project_id_message_id_fkey,
    ADD CONSTRAINT message_mentions_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE message_reactions
    DROP CONSTRAINT message_reactions_project_id_message_id_fkey,
    ADD CONSTRAINT message_reactions_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE message_revisions
    DROP CONSTRAINT message_revisions_project_id_message_id_fkey,
    ADD CONSTRAINT message_revisions_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE message_work_item_references
    DROP CONSTRAINT message_work_item_references_project_id_message_id_fkey,
    ADD CONSTRAINT message_work_item_references_project_id_message_id_fkey
        FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE messages
    DROP CONSTRAINT messages_project_conversation_fk,
    ADD CONSTRAINT messages_project_conversation_fk
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE project_navigation_states
    DROP CONSTRAINT project_navigation_states_project_id_conversation_id_fkey,
    ADD CONSTRAINT project_navigation_states_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE provider_sessions
    DROP CONSTRAINT provider_sessions_conversation_repository_fk,
    ADD CONSTRAINT provider_sessions_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id) REFERENCES conversations(project_id, id, repository_id) ON UPDATE CASCADE;
ALTER TABLE reply_requests
    DROP CONSTRAINT reply_requests_project_id_conversation_id_fkey,
    ADD CONSTRAINT reply_requests_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE reply_requests
    DROP CONSTRAINT reply_requests_reply_message_fkey,
    ADD CONSTRAINT reply_requests_reply_message_fkey
        FOREIGN KEY (project_id, reply_message_id) REFERENCES messages(project_id, id) ON UPDATE CASCADE;
ALTER TABLE reply_requests
    DROP CONSTRAINT reply_requests_thread_fkey,
    ADD CONSTRAINT reply_requests_thread_fkey
        FOREIGN KEY (project_id, thread_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE repository_workspaces
    DROP CONSTRAINT repository_workspaces_conversation_repository_fk,
    ADD CONSTRAINT repository_workspaces_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id) REFERENCES conversations(project_id, id, repository_id) ON UPDATE CASCADE;
ALTER TABLE requests
    DROP CONSTRAINT requests_space_id_conversation_id_fkey,
    ADD CONSTRAINT requests_space_id_conversation_id_fkey
        FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE runs
    DROP CONSTRAINT runs_space_id_conversation_id_fkey,
    ADD CONSTRAINT runs_space_id_conversation_id_fkey
        FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE work_item_conversation_links
    DROP CONSTRAINT work_item_conversation_links_project_id_conversation_id_fkey,
    ADD CONSTRAINT work_item_conversation_links_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;
ALTER TABLE work_item_worker_invocations
    DROP CONSTRAINT work_item_worker_invocations_project_id_conversation_id_fkey,
    ADD CONSTRAINT work_item_worker_invocations_project_id_conversation_id_fkey
        FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id) ON UPDATE CASCADE;

-- 2. The discussion tables' accounts are accounts, not memberships.
ALTER TABLE conversations DROP CONSTRAINT conversations_archive_actor_membership_fk;
ALTER TABLE messages DROP CONSTRAINT messages_author_membership_fk;
ALTER TABLE messages
    DROP CONSTRAINT messages_removed_by_membership_fk,
    ADD CONSTRAINT messages_removed_by_actor_fk
        FOREIGN KEY (removed_by_actor_id) REFERENCES actors(id);
ALTER TABLE message_mentions
    DROP CONSTRAINT message_mentions_project_id_actor_id_fkey,
    ADD CONSTRAINT message_mentions_actor_fk FOREIGN KEY (actor_id) REFERENCES actors(id);
ALTER TABLE conversation_read_states
    DROP CONSTRAINT conversation_read_states_project_id_actor_id_fkey,
    ADD CONSTRAINT conversation_read_states_actor_fk FOREIGN KEY (actor_id) REFERENCES actors(id);
ALTER TABLE reply_requests
    DROP CONSTRAINT reply_requests_project_id_agent_actor_id_fkey,
    ADD CONSTRAINT reply_requests_agent_actor_fk FOREIGN KEY (agent_actor_id) REFERENCES actors(id);
ALTER TABLE message_revisions
    DROP CONSTRAINT message_revisions_project_id_written_by_actor_id_fkey,
    ADD CONSTRAINT message_revisions_written_by_actor_fk
        FOREIGN KEY (written_by_actor_id) REFERENCES actors(id);
ALTER TABLE message_reactions
    DROP CONSTRAINT message_reactions_project_id_actor_id_fkey,
    ADD CONSTRAINT message_reactions_actor_fk FOREIGN KEY (actor_id) REFERENCES actors(id);

-- 3. Guards step aside during a move.
CREATE FUNCTION discussion_move_in_progress()
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('aw.discussion_move', true), '') = 'on'
$$;

DROP TRIGGER messages_guard_mutation ON messages;
CREATE TRIGGER messages_guard_mutation BEFORE DELETE OR UPDATE ON messages
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress() AND NOT discussion_move_in_progress())
    EXECUTE FUNCTION guard_message_mutation();
DROP TRIGGER message_mentions_guard_mutation ON message_mentions;
CREATE TRIGGER message_mentions_guard_mutation BEFORE DELETE OR UPDATE ON message_mentions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress() AND NOT discussion_move_in_progress())
    EXECUTE FUNCTION guard_message_projection_mutation();
DROP TRIGGER message_work_item_references_guard_mutation ON message_work_item_references;
CREATE TRIGGER message_work_item_references_guard_mutation
    BEFORE DELETE OR UPDATE ON message_work_item_references
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress() AND NOT discussion_move_in_progress())
    EXECUTE FUNCTION guard_message_projection_mutation();
DROP TRIGGER message_revisions_guard ON message_revisions;
CREATE TRIGGER message_revisions_guard BEFORE DELETE OR UPDATE ON message_revisions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress() AND NOT discussion_move_in_progress())
    EXECUTE FUNCTION guard_message_revision_mutation();
DROP TRIGGER message_mentions_validate ON message_mentions;
CREATE TRIGGER message_mentions_validate BEFORE INSERT OR UPDATE ON message_mentions
    FOR EACH ROW WHEN (NOT discussion_move_in_progress())
    EXECUTE FUNCTION validate_message_mention();
DROP TRIGGER conversation_read_states_validate ON conversation_read_states;
CREATE TRIGGER conversation_read_states_validate BEFORE INSERT OR UPDATE ON conversation_read_states
    FOR EACH ROW WHEN (NOT discussion_move_in_progress())
    EXECUTE FUNCTION validate_conversation_read_state();
DROP TRIGGER message_work_item_references_validate ON message_work_item_references;
CREATE TRIGGER message_work_item_references_validate
    BEFORE INSERT OR UPDATE ON message_work_item_references
    FOR EACH ROW WHEN (NOT discussion_move_in_progress())
    EXECUTE FUNCTION validate_work_item_reference_projection();
DROP TRIGGER message_reactions_validate ON message_reactions;
CREATE TRIGGER message_reactions_validate BEFORE INSERT OR UPDATE ON message_reactions
    FOR EACH ROW WHEN (NOT discussion_move_in_progress())
    EXECUTE FUNCTION validate_message_reaction();
DROP TRIGGER conversations_validate_parent ON conversations;
CREATE TRIGGER conversations_validate_parent
    BEFORE INSERT OR UPDATE OF kind, parent_channel_id, project_id, origin_message_id
    ON conversations
    FOR EACH ROW WHEN (NOT discussion_move_in_progress())
    EXECUTE FUNCTION validate_conversation_parent();
