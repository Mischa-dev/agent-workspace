-- T1.18: records whose rollups need working out again because a record
-- they read changed. A write that changes a record adds the records whose
-- rollups read it (one row each, so a burst of changes to the records one
-- record reads is worked out once) and queues the `fields.recalculate`
-- job, which takes rows in batches, works them out and deletes them.
-- `depth` counts how many records away the first change was, so a chain of
-- rollups stops at the limit the field settings already enforce.

CREATE TABLE record_recalculations (
    record_id UUID PRIMARY KEY REFERENCES collection_records(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    depth SMALLINT NOT NULL DEFAULT 1 CHECK (depth BETWEEN 1 AND 10),
    queued_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX record_recalculations_by_space ON record_recalculations (space_id, queued_at);
CREATE INDEX record_recalculations_by_workspace ON record_recalculations (workspace_id);
SELECT tenancy_protect('record_recalculations', 'workspace_id');
