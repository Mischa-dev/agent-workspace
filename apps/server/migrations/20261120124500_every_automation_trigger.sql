-- C1.28 and T1.38 each rewrote the trigger list on their own branches; this
-- holds every trigger both kept, so comment, proposal, record and incoming
-- hook rules all save.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'record_matches', 'field_changed',
    'page_changed', 'sheet_changed', 'whiteboard_changed', 'comment_posted', 'proposal_made',
    'incoming_hook', 'schedule', 'capacity_window'
));
