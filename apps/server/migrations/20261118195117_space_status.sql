-- T1.43 Project status and update posts: a space's health, target date and
-- lead, and the update thread that last set its health. An update is a
-- thread (made by the one thread-creation function) whose outcome is the
-- health it set; its words stay in its messages.

CREATE TABLE space_statuses (
    space_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    health TEXT CHECK (health IN ('on_track', 'at_risk', 'off_track')),
    target_date DATE,
    lead_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    latest_update_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    latest_update_at TIMESTAMPTZ,
    latest_update_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    updated_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    version BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX space_statuses_by_workspace ON space_statuses (workspace_id);
CREATE INDEX space_statuses_latest_update ON space_statuses (latest_update_id)
    WHERE latest_update_id IS NOT NULL;
SELECT tenancy_protect('space_statuses', 'workspace_id');
