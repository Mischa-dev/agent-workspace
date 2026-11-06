-- AWD-74: a rule run by hand through a person's app or script (MCP
-- automation_write "run", or the public API) keeps to that app's agent
-- limit as it is when the run fires, inline or from the job, even when the
-- rule itself was made in the browser. Event and scheduled firings keep
-- the rule's own attribution (automation_rules.made_through_app).

ALTER TABLE automation_runs
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;
