-- A1.14: the built-in host keeps its runs' records itself
-- (`runs::builtin`): it opens a run's `runs` row with its detail row and
-- moves it with every state change. The trigger that copied `agent_runs`
-- into `runs`, and the SQL mappings only it used, go. Every built-in run
-- already has its record (the trigger wrote it); the detail row's id
-- references it (`agent_runs_run_fk`, checked when the transaction commits).

DROP TRIGGER agent_runs_mirror ON agent_runs;
DROP FUNCTION runs_mirror_agent_run();
DROP FUNCTION runs_state_from_legacy(TEXT, TEXT);
DROP FUNCTION runs_title_from_prompt(TEXT);
