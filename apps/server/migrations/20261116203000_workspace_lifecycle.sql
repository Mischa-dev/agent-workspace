-- H1.24 (docs/plans/master/plans/H1-hosting-and-release.md section 3.6):
-- workspace lifecycle.
--
-- * organizations.lifecycle: `active`, `importing` (H1.10 fills it),
--   `suspended` (MCP, the API and operations refuse with
--   `workspace_suspended`) or `deleting` (the owner asked to delete it;
--   the `workspace.delete` job removes it at `delete_after` unless they
--   cancel first). Every existing workspace is active.
-- * deleted_workspaces: the one small record a deleted workspace leaves,
--   for billing and staff. Server-wide: its workspace is gone.
-- * workspace_deletion_in_progress(): true only inside the deletion job's
--   transaction, which sets `aw.workspace_delete` transaction-locally. The
--   guards that keep history (sent messages, comments and their revisions,
--   allocated task numbers, task records) skip while it is set: deleting a
--   whole workspace is the one path that removes them (AGENTS.md rule 16).
--   The guards' functions are untouched; their triggers gain a WHEN, so a
--   later change to a guard's body keeps the exception.

ALTER TABLE organizations
    ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'active'
        CHECK (lifecycle IN ('active', 'importing', 'suspended', 'deleting')),
    ADD COLUMN lifecycle_reason TEXT
        CHECK (lifecycle_reason IS NULL OR (
            char_length(lifecycle_reason) BETWEEN 1 AND 40
            AND lifecycle_reason ~ '^[a-z][a-z_]*$'
        )),
    ADD COLUMN lifecycle_changed_at TIMESTAMPTZ,
    ADD COLUMN delete_after TIMESTAMPTZ,
    ADD COLUMN deletion_requested_by_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT organizations_deleting_has_a_date
        CHECK ((lifecycle = 'deleting') = (delete_after IS NOT NULL)),
    ADD CONSTRAINT organizations_suspended_has_a_reason
        CHECK (lifecycle <> 'suspended' OR lifecycle_reason IS NOT NULL);

-- The deletion job looks for nothing by date (it is queued with its run
-- time), but staff and the console list what is closing.
CREATE INDEX organizations_not_active
    ON organizations (lifecycle, delete_after)
    WHERE lifecycle <> 'active';

CREATE TABLE deleted_workspaces (
    id UUID PRIMARY KEY,
    deleted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Who asked, kept as a bare id: the account may be gone by then.
    requested_by_actor_id UUID
);

CREATE OR REPLACE FUNCTION workspace_deletion_in_progress() RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('aw.workspace_delete', true), '') <> ''
$$;

DROP TRIGGER IF EXISTS messages_guard_mutation ON messages;
CREATE TRIGGER messages_guard_mutation
    BEFORE DELETE OR UPDATE ON messages
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_message_mutation();

DROP TRIGGER IF EXISTS message_mentions_guard_mutation ON message_mentions;
CREATE TRIGGER message_mentions_guard_mutation
    BEFORE DELETE OR UPDATE ON message_mentions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_message_projection_mutation();

DROP TRIGGER IF EXISTS message_work_item_references_guard_mutation ON message_work_item_references;
CREATE TRIGGER message_work_item_references_guard_mutation
    BEFORE DELETE OR UPDATE ON message_work_item_references
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_message_projection_mutation();

DROP TRIGGER IF EXISTS message_revisions_guard ON message_revisions;
CREATE TRIGGER message_revisions_guard
    BEFORE DELETE OR UPDATE ON message_revisions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_message_revision_mutation();

DROP TRIGGER IF EXISTS work_item_comments_guard_mutation ON work_item_comments;
CREATE TRIGGER work_item_comments_guard_mutation
    BEFORE DELETE OR UPDATE ON work_item_comments
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_work_item_comment_mutation();

DROP TRIGGER IF EXISTS work_item_comment_mentions_guard_mutation ON work_item_comment_mentions;
CREATE TRIGGER work_item_comment_mentions_guard_mutation
    BEFORE DELETE OR UPDATE ON work_item_comment_mentions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_work_item_comment_projection_mutation();

DROP TRIGGER IF EXISTS work_item_comment_references_guard_mutation ON work_item_comment_references;
CREATE TRIGGER work_item_comment_references_guard_mutation
    BEFORE DELETE OR UPDATE ON work_item_comment_references
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_work_item_comment_projection_mutation();

DROP TRIGGER IF EXISTS work_item_comment_revisions_guard ON work_item_comment_revisions;
CREATE TRIGGER work_item_comment_revisions_guard
    BEFORE DELETE OR UPDATE ON work_item_comment_revisions
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION guard_work_item_comment_revision_mutation();

DROP TRIGGER IF EXISTS project_counters_enforce_monotonicity ON project_counters;
CREATE TRIGGER project_counters_enforce_monotonicity
    BEFORE DELETE OR UPDATE ON project_counters
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION enforce_project_counter_monotonicity();

DROP TRIGGER IF EXISTS collection_records_task_guard ON collection_records;
CREATE TRIGGER collection_records_task_guard
    BEFORE DELETE OR UPDATE ON collection_records
    FOR EACH ROW WHEN (NOT workspace_deletion_in_progress())
    EXECUTE FUNCTION task_record_guard();
