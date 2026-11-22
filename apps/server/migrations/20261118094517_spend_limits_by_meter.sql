-- H1.27, A2.14: `admit` reads a workspace's budgets on one meter for every
-- ask a budget can cover (each message, once a workspace limits messages).
CREATE INDEX spend_limits_meter ON spend_limits (workspace_id, meter);
