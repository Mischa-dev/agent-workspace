-- 20261120124500 rewrote the trigger list from the branches it merged, and
-- left out the connected module trigger (M1.13); this holds every trigger
-- `automations::TRIGGER_KINDS` names, so module event rules save again.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'record_matches', 'field_changed',
    'page_changed', 'sheet_changed', 'whiteboard_changed', 'module_event', 'comment_posted',
    'proposal_made', 'incoming_hook', 'schedule', 'capacity_window'
));
