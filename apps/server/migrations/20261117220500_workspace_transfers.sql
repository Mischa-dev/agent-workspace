-- H1.9 and H1.10: a workspace leaves as one archive and comes back from
-- one. Each export or import is a transfer, driven by a job: an export
-- writes its archive to the workspace's store and keeps it 7 days; an
-- import walks an archive in batches, its cursor saved with each batch so
-- a stopped import resumes where it was. `workspace_id` is the workspace
-- exported, or the new workspace an import fills.
CREATE TABLE workspace_transfers (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('export', 'import')),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'expired')),
    requested_by_actor_id UUID NOT NULL REFERENCES actors(id),
    store_id TEXT CHECK (store_id IS NULL OR store_id ~ '^[a-z][a-z0-9_-]{0,47}$'),
    file_key TEXT CHECK (file_key IS NULL OR file_key ~ '^[0-9a-z/.-]{1,200}$'),
    byte_size BIGINT CHECK (byte_size IS NULL OR byte_size >= 0),
    id_mode TEXT CHECK (id_mode IS NULL OR id_mode IN ('preserve', 'copy')),
    cursor JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(cursor::text) <= 4096),
    counts JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(counts::text) <= 65536),
    error TEXT CHECK (error IS NULL OR char_length(error) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    -- Who is working on it until when: one process at a time drives a
    -- transfer, and another takes over once the lease runs out.
    lease_until TIMESTAMPTZ
);
CREATE INDEX workspace_transfers_workspace ON workspace_transfers (workspace_id, created_at DESC);
CREATE INDEX workspace_transfers_expiring ON workspace_transfers (expires_at)
    WHERE state = 'succeeded' AND expires_at IS NOT NULL;
SELECT tenancy_protect('workspace_transfers', 'workspace_id');

-- How an import maps the archive's ids: in copy mode every id it carries
-- gets a fresh one; in both modes the workspace, accounts matched to ones
-- already here, and rows the new workspace already had (its own space, its
-- built-in roles). References, in columns and in text, are rewritten
-- through it.
CREATE TABLE workspace_import_ids (
    transfer_id UUID NOT NULL REFERENCES workspace_transfers(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    source_id UUID NOT NULL,
    target_id UUID NOT NULL,
    PRIMARY KEY (transfer_id, source_id)
);
SELECT tenancy_protect('workspace_import_ids', 'workspace_id');

-- "Export the workspace" (`workspace.export`) in the SQL permission
-- catalogue, beside `policy::PERMISSIONS`. Owners hold it by default.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('workspace.export', 'settings', 'workspace', false, ARRAY['owner']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

-- Nothing an import writes is news (H1.10): while a transaction sets
-- `aw.importing`, the triggers that stage events for quiet writes stage
-- none, so no event, notification or webhook comes of an import.
CREATE OR REPLACE FUNCTION stage_event(
    p_type TEXT, p_class TEXT, p_space UUID, p_thing_kind TEXT, p_thing UUID,
    p_audience TEXT, p_actor UUID, p_conversation UUID DEFAULT NULL,
    p_account UUID DEFAULT NULL, p_access_thing UUID DEFAULT NULL,
    p_version BIGINT DEFAULT NULL, p_changes TEXT[] DEFAULT '{}',
    p_payload JSONB DEFAULT '{}'
) RETURNS VOID LANGUAGE sql AS $$
    INSERT INTO event_staging (
        id, workspace_id, space_id, thing_id, thing_kind, access_thing_id, thing_version,
        type, class, changes, audience, conversation_id, conversation_restricted,
        audience_account_id, actor_account_id, via_credential_id, requested_by_account_id,
        run_id, request_id, payload
    )
    SELECT gen_random_uuid(), project.organization_id, p_space, p_thing, p_thing_kind,
           p_access_thing, p_version, p_type, p_class, p_changes, p_audience,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(conversation.parent_channel_id, conversation.id) END,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(parent.visibility, conversation.visibility) <> 'project' END,
           CASE WHEN p_audience = 'account' THEN p_account END,
           COALESCE(NULLIF(current_setting('aw.actor', true), '')::uuid, p_actor),
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           p_payload
    FROM projects project
    -- visibility: only the root and its restriction are read, to route the event
    LEFT JOIN conversations conversation ON conversation.id = p_conversation
    LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
    WHERE project.id = p_space
      -- A conversation event needs its conversation (gone in a cascade).
      AND (p_audience <> 'conversation' OR conversation.id IS NOT NULL)
      AND COALESCE(current_setting('aw.importing', true), '') <> 'on'
$$;

CREATE OR REPLACE FUNCTION stage_access_changed(p_workspace UUID, p_space UUID, p_account UUID)
RETURNS VOID LANGUAGE sql AS $$
    INSERT INTO event_staging (
        id, workspace_id, space_id, type, class, audience, audience_account_id,
        actor_account_id, via_credential_id, requested_by_account_id, run_id,
        request_id, payload
    )
    SELECT gen_random_uuid(), p_workspace, p_space, 'account.access_changed', 'signal',
           'account', p_account,
           NULLIF(current_setting('aw.actor', true), '')::uuid,
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           jsonb_build_object('accountId', p_account)
    WHERE p_workspace IS NOT NULL AND p_account IS NOT NULL
      AND COALESCE(current_setting('aw.importing', true), '') <> 'on'
$$;

-- An import writes history as it was, not through the commands that
-- check each write as it happens (an edit's revision, a run's admission,
-- an archived thread's freeze). The checks below step aside inside an
-- import's transactions (`aw.importing`), and only there. Derivations
-- (a message's space, a task's record number, a membership's role) keep
-- running. A later migration that redefines one of these functions drops
-- its exemption: the import round-trip test (workspace_import.rs) fails
-- until it is added back.
CREATE FUNCTION importing() RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT COALESCE(current_setting('aw.importing', true), '') = 'on'
$$;

DO $$
DECLARE
    name TEXT;
    definition TEXT;
BEGIN
    FOREACH name IN ARRAY ARRAY[
        'enforce_agent_run_acknowledgement_scope', 'enforce_agent_run_config_frozen',
        'enforce_agent_run_identity', 'enforce_allocated_project_key_immutability',
        'enforce_conversation_archive', 'enforce_github_pull_request_identity',
        'enforce_project_archive_attribution', 'enforce_project_counter_monotonicity',
        'enforce_repository_archive', 'enforce_repository_identity',
        'enforce_repository_run_consistency_at_commit', 'enforce_thread_repository_immutability',
        'guard_message_mutation', 'guard_message_projection_mutation', 'guard_message_reply_to',
        'guard_message_revision_mutation', 'guard_work_item_comment_mutation',
        'guard_work_item_comment_projection_mutation', 'guard_work_item_comment_reply_to',
        'guard_work_item_comment_revision_mutation', 'launches_match_workspace',
        'prevent_disabling_referenced_work_item_definition', 'prevent_work_item_parent_cycle',
        'project_environments_match_workspace', 'runs_freeze_settings', 'task_record_guard',
        'task_record_has_task', 'validate_agent_run_scope_and_admission',
        'validate_conversation_parent', 'validate_conversation_read_state',
        'validate_external_execution_request', 'validate_github_pull_request_thread_link',
        'validate_message_mention', 'validate_message_reaction', 'validate_message_revision',
        'validate_project_membership_actor_role', 'validate_project_navigation_location',
        'validate_work_item_assignee', 'validate_work_item_comment_mention',
        'validate_work_item_comment_revision', 'validate_work_item_definition_state',
        'validate_work_item_label_active', 'validate_work_item_thread_link'
    ] LOOP
        SELECT pg_get_functiondef(p.oid) INTO definition
        FROM pg_proc p
        WHERE p.proname = name AND p.pronamespace = current_schema()::regnamespace;
        IF definition IS NULL OR definition LIKE '%importing()%' THEN
            CONTINUE;
        END IF;
        EXECUTE regexp_replace(
            definition, '\mBEGIN\M',
            'BEGIN IF importing() THEN RETURN COALESCE(NEW, OLD); END IF;', 'i');
    END LOOP;
END
$$;
