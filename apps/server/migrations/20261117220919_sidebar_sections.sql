-- C2.9: sidebar sections a person arranges. Each account keeps its own
-- sections in each workspace: Starred (made the first time something is
-- starred) and any it names. A conversation sits in at most one of them;
-- one in none stays where it always was, under its space's channels or
-- with the direct messages. Reads go through `conversation_visible_to`, so
-- losing access hides an item and keeps its row.

CREATE TABLE sidebar_sections (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    kind TEXT NOT NULL DEFAULT 'custom' CHECK (kind IN ('custom', 'starred')),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    -- How its items are ordered: as placed, by name, or newest activity first.
    sort TEXT NOT NULL DEFAULT 'manual' CHECK (sort IN ('manual', 'alpha', 'recent')),
    -- Everything in it, or only what's unread.
    show TEXT NOT NULL DEFAULT 'all' CHECK (show IN ('all', 'unread')),
    collapsed BOOLEAN NOT NULL DEFAULT false,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX sidebar_sections_name_unique
    ON sidebar_sections (account_id, workspace_id, lower(name));
-- One Starred section each.
CREATE UNIQUE INDEX sidebar_sections_one_starred
    ON sidebar_sections (account_id, workspace_id) WHERE kind = 'starred';
CREATE INDEX sidebar_sections_by_workspace ON sidebar_sections (workspace_id);
SELECT tenancy_protect('sidebar_sections', 'workspace_id');

CREATE TABLE sidebar_items (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    section_id UUID NOT NULL REFERENCES sidebar_sections(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A conversation sits in one section.
    PRIMARY KEY (account_id, conversation_id)
);

CREATE INDEX sidebar_items_by_section ON sidebar_items (section_id, position);
CREATE INDEX sidebar_items_by_workspace ON sidebar_items (workspace_id);
SELECT tenancy_protect('sidebar_items', 'workspace_id', 'conversation_id');
