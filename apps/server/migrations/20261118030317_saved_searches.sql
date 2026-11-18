-- C2.26: searches a person saves, in the search grammar, each account's own
-- in each workspace. They hold the query as typed; what it finds is read
-- live with the reader's access every time.

CREATE TABLE saved_searches (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    query TEXT NOT NULL CHECK (char_length(query) BETWEEN 1 AND 200),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX saved_searches_name_unique
    ON saved_searches (account_id, workspace_id, lower(name));
CREATE INDEX saved_searches_by_workspace ON saved_searches (workspace_id);
SELECT tenancy_protect('saved_searches', 'workspace_id');
