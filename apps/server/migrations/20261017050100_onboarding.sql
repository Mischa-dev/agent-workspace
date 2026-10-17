-- P5: the first-run path. Most steps are read from what exists (a project,
-- a repository, a signed-in account, an environment, tasks, a run); this
-- keeps what can't be: the project the path is setting up, the steps a
-- person confirmed or skipped (naming the workspace, adding an agent), and
-- whether they put the path away. One row per person per workspace.
CREATE TABLE onboarding_progress (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    confirmed_steps TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(confirmed_steps) <= 16),
    skipped_steps TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(skipped_steps) <= 16),
    dismissed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, actor_id)
);
