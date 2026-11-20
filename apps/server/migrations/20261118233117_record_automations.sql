-- T1.38: automations on records. A "record starts matching" rule fires when
-- a record enters its conditions, not on every change while it matches;
-- this keeps the records each such rule last saw match, as
-- automation_rule_matches does for tasks.
CREATE TABLE automation_record_matches (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    rule_id UUID NOT NULL REFERENCES automation_rules(id) ON DELETE CASCADE,
    record_id UUID NOT NULL REFERENCES collection_records(id) ON DELETE CASCADE,
    matched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (rule_id, record_id)
);

CREATE INDEX automation_record_matches_by_workspace ON automation_record_matches (workspace_id);
CREATE INDEX automation_record_matches_by_record ON automation_record_matches (record_id);
SELECT tenancy_protect('automation_record_matches', 'workspace_id');

-- The two record triggers: a record starts matching, and a field changes.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'record_matches', 'field_changed',
    'page_changed', 'sheet_changed', 'whiteboard_changed', 'incoming_hook', 'schedule',
    'capacity_window'
));
