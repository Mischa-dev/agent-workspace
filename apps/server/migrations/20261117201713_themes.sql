-- Themes (Lane C, AWD-208): a theme is one document of design tokens for
-- light and dark, each starting from a built-in theme, plus optional CSS
-- (apps/web/src/theme/contract.json says what it may hold; src/themes.rs
-- checks it). A theme belongs to whoever made it; a holder of the
-- workspace's settings permission can offer it to everyone in the
-- workspace, who can then pick it.

CREATE TABLE themes (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    owner_account_id UUID NOT NULL REFERENCES actors(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60 AND name !~ '[[:cntrl:]]'),
    document JSONB NOT NULL CHECK (
        jsonb_typeof(document) = 'object' AND octet_length(document::text) <= 65536
    ),
    offered BOOLEAN NOT NULL DEFAULT false,
    offered_by_account_id UUID REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Someone's own themes, and the ones a workspace offers.
CREATE INDEX themes_owner ON themes (workspace_id, owner_account_id, name);
CREATE INDEX themes_offered ON themes (workspace_id, name) WHERE offered;

SELECT tenancy_protect('themes', 'workspace_id');
