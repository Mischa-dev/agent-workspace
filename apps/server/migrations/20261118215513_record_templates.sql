-- T1.30: record templates and recurring records, for any collection. A
-- template is a record to start from: its title (where "{date}" becomes the
-- day it's made) and field values by name or id, as record_create takes
-- them. A recurring record makes one from a template on a schedule (the
-- same daily, weekly or monthly schedules as recurring tasks), as the
-- person who set it up. Task templates and recurring tasks stay as they
-- are: they are the Tasks collection's.

CREATE TABLE record_templates (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 300),
    field_values JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(field_values) = 'object' AND octet_length(field_values::text) <= 65536
    ),
    created_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    archived_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (space_id, id),
    FOREIGN KEY (space_id, collection_id) REFERENCES collections (project_id, id)
);

CREATE INDEX record_templates_by_workspace ON record_templates (workspace_id);
CREATE INDEX record_templates_by_collection ON record_templates (space_id, collection_id, created_at);
SELECT tenancy_protect('record_templates', 'workspace_id');

CREATE TABLE recurring_records (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    template_id UUID NOT NULL,
    -- Who it acts as: whoever set it up or last started it again, through
    -- the agent connection or connected app they used, so each record is
    -- checked as that call would be.
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    external_agent_connection_id UUID REFERENCES external_agent_connections(id) ON DELETE SET NULL,
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) ON DELETE SET NULL,
    frequency TEXT NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    repeat_every INTEGER NOT NULL DEFAULT 1 CHECK (repeat_every BETWEEN 1 AND 52),
    weekdays SMALLINT[] NOT NULL DEFAULT ARRAY[]::smallint[] CHECK (
        weekdays <@ ARRAY[1, 2, 3, 4, 5, 6, 7]::smallint[]
    ),
    month_day SMALLINT CHECK (month_day IS NULL OR month_day BETWEEN 1 AND 31),
    time_of_day TIME NOT NULL DEFAULT '09:00',
    timezone TEXT NOT NULL DEFAULT 'UTC' CHECK (char_length(timezone) BETWEEN 1 AND 64),
    starts_on DATE NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    paused_reason TEXT CHECK (paused_reason IS NULL OR char_length(paused_reason) <= 300),
    next_run_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    last_record_id UUID,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, template_id) REFERENCES record_templates (space_id, id) ON DELETE CASCADE,
    CHECK (frequency <> 'weekly' OR cardinality(weekdays) >= 1),
    CHECK (frequency <> 'monthly' OR month_day IS NOT NULL)
);

CREATE INDEX recurring_records_by_workspace ON recurring_records (workspace_id);
CREATE INDEX recurring_records_by_template ON recurring_records (space_id, template_id);
-- The sweep reads due rows only.
CREATE INDEX recurring_records_due ON recurring_records (next_run_at) WHERE enabled;
SELECT tenancy_protect('recurring_records', 'workspace_id');
