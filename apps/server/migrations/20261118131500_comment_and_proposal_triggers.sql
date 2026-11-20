-- C1.28: automation rules fire when someone comments on a thing or proposes
-- a change to a page, sheet or whiteboard. Every trigger other branches
-- added stays, so rules already saved keep passing the check.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'record_matches', 'field_changed',
    'page_changed', 'sheet_changed', 'whiteboard_changed', 'comment_posted', 'proposal_made',
    'incoming_hook', 'schedule', 'capacity_window'
));
