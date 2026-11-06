-- AWD-74: a launch queued by an automation that a person's app or script
-- made (automation_rules.made_through_app) starts only within that app's
-- agent limit as it is when the queue drains, like a launch the app queued
-- itself (source mcp).

ALTER TABLE launches
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;
