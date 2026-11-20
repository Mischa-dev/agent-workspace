-- T1.34 and T1.35: outside data. A space keeps connections to databases
-- kept elsewhere and saved, read-only queries over them; a query runs as a
-- job (read-only transaction, time limit, row cap) and its answer is
-- stored, so a dashboard shows it without anyone reaching the database.
-- A query is a draft until someone who can connect data reviews it; any
-- change to what it runs makes it a draft again. A reviewed query can be
-- shared (data.share): then everyone in the space sees its stored answer
-- and may run it again, and nobody without data.connect sees its text or
-- its connection.

CREATE TABLE data_connections (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    -- The protocol spoken; one today.
    kind TEXT NOT NULL CHECK (kind IN ('postgresql')),
    -- Where it points, shown to whoever manages it: user@host:port/database.
    -- The full address, with its password, is the secret.
    address TEXT NOT NULL CHECK (char_length(address) BETWEEN 1 AND 400),
    secret_id UUID REFERENCES secrets(id) ON DELETE SET NULL,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX data_connections_name_unique ON data_connections (space_id, lower(name));
CREATE INDEX data_connections_by_workspace ON data_connections (workspace_id);
SELECT tenancy_protect('data_connections', 'workspace_id');

CREATE TABLE data_queries (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES data_connections(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    -- One SELECT; {{name}} marks a parameter.
    statement TEXT NOT NULL CHECK (char_length(statement) BETWEEN 1 AND 20000),
    -- [{"name", "type": "text"|"number"|"date"|"boolean", "default"}]
    parameters JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(parameters) = 'array' AND octet_length(parameters::text) <= 8192
    ),
    row_cap INTEGER NOT NULL DEFAULT 1000 CHECK (row_cap BETWEEN 1 AND 5000),
    -- T1.35: draft until reviewed; a change to what it runs makes it a
    -- draft again.
    review_state TEXT NOT NULL DEFAULT 'draft' CHECK (review_state IN ('draft', 'reviewed')),
    reviewed_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    reviewed_at TIMESTAMPTZ,
    -- Its stored answer is everyone's in the space (data.share).
    shared BOOLEAN NOT NULL DEFAULT false,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT data_queries_reviewed_has_reviewer CHECK (
        review_state = 'draft' OR reviewed_at IS NOT NULL
    ),
    CONSTRAINT data_queries_shared_is_reviewed CHECK (
        NOT shared OR reviewed_at IS NOT NULL
    )
);

CREATE UNIQUE INDEX data_queries_name_unique ON data_queries (space_id, lower(name));
CREATE INDEX data_queries_by_connection ON data_queries (connection_id);
CREATE INDEX data_queries_by_workspace ON data_queries (workspace_id);
SELECT tenancy_protect('data_queries', 'workspace_id');

-- A query's stored answers: one for its last run as a draft and one for its
-- last run while reviewed, which is the one shared. A new run keeps the
-- answer there until its own is in (pending_since).
CREATE TABLE data_query_results (
    query_id UUID NOT NULL REFERENCES data_queries(id) ON DELETE CASCADE,
    reviewed BOOLEAN NOT NULL,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- A run asked for and not finished.
    pending_since TIMESTAMPTZ,
    pending_parameters JSONB NOT NULL DEFAULT '{}'::jsonb,
    requested_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    -- The last finished run.
    ran_at TIMESTAMPTZ,
    ran_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    parameters JSONB NOT NULL DEFAULT '{}'::jsonb,
    columns JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(columns) = 'array'),
    rows JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(rows) = 'array'),
    row_count INTEGER NOT NULL DEFAULT 0 CHECK (row_count >= 0),
    -- More rows than the cap (or the stored size) came back; the first are kept.
    truncated BOOLEAN NOT NULL DEFAULT false,
    error TEXT CHECK (error IS NULL OR char_length(error) <= 2000),
    duration_ms INTEGER,
    PRIMARY KEY (query_id, reviewed)
);

CREATE INDEX data_query_results_by_workspace ON data_query_results (workspace_id);
SELECT tenancy_protect('data_query_results', 'workspace_id');
