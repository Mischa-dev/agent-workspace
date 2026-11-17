-- K3.5: automation rules fire when a page, sheet or whiteboard changes.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'page_changed', 'sheet_changed',
    'whiteboard_changed', 'schedule', 'capacity_window'
));
