-- AWD-110: automation rules fire on a collection's records too.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'schedule', 'capacity_window'
));
