-- AWD-74: work a person's app or script set going that runs later keeps to
-- that app's agent limit as it is when the work runs, like a launch or an
-- automation the app made:
--
-- - an import or a plan the app committed (or, for a GitHub import,
--   started) creates its tasks only within the limit as it is when each
--   chunk or step runs; tasks made before it narrowed are kept;
-- - a run the app started that waits in the admission queue starts only if
--   the app may still start runs then (the requester's role is checked
--   again for every queued run).

ALTER TABLE task_imports
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;

ALTER TABLE ai_plan_imports
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;

ALTER TABLE agent_runs
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;
