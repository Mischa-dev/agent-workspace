-- M1.11: templates become packages; the directory; the switches' contract step.
--
-- * directory_sources: where installable packages come from. A row with no
--   workspace is server-wide (the folder and URL lists the operator sets,
--   `AGENT_WORKSPACE_DIRECTORY_DIR` and `AGENT_WORKSPACE_DIRECTORY_URLS`,
--   synced at start-up); a row with one is that workspace's own URL list.
--   The built-in packages (`modules/packages/`) are compiled in and need no
--   row. `fetched_at` says when the refresh job last read it.
-- * directory_entries: what each source offers, one row per package and
--   version, with the inline document read from it. A file or entry that
--   doesn't read is kept with its error (and an empty version), so a
--   broken package is listed, never dropped silently.
-- * Contract step for module switches (M1 section 4 step 2): every reader
--   moved to `module_installs` in M1.2, so the trigger that mirrored old
--   `modules.<id>` settings writes is dropped and the duplicate settings
--   rows are deleted.

CREATE TABLE directory_sources (
    id UUID PRIMARY KEY,
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('builtin', 'folder', 'url_list', 'catalogue')),
    location TEXT NOT NULL CHECK (char_length(location) BETWEEN 1 AND 2000),
    trusted_key TEXT CHECK (trusted_key IS NULL OR char_length(trusted_key) <= 200),
    name TEXT NOT NULL DEFAULT '' CHECK (char_length(name) <= 120),
    enabled BOOLEAN NOT NULL DEFAULT true,
    error TEXT CHECK (error IS NULL OR char_length(error) <= 2000),
    fetched_at TIMESTAMPTZ,
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A workspace's own sources are URL lists: a folder is a path on the
    -- server, which only its operator sets.
    CHECK (workspace_id IS NULL OR kind = 'url_list')
);

CREATE UNIQUE INDEX directory_sources_server
    ON directory_sources (kind, location) WHERE workspace_id IS NULL;
CREATE UNIQUE INDEX directory_sources_workspace
    ON directory_sources (workspace_id, kind, location) WHERE workspace_id IS NOT NULL;
CREATE INDEX directory_sources_due ON directory_sources (fetched_at NULLS FIRST)
    WHERE enabled;

CREATE TABLE directory_entries (
    source_id UUID NOT NULL REFERENCES directory_sources(id) ON DELETE CASCADE,
    -- The source's workspace, copied so a workspace's entries are found
    -- (and removed with it) without a join.
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('package', 'connected_module')),
    entry_key TEXT NOT NULL CHECK (char_length(entry_key) BETWEEN 1 AND 300),
    version TEXT NOT NULL CHECK (char_length(version) <= 40),
    name TEXT NOT NULL CHECK (char_length(name) <= 200),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    location TEXT NOT NULL CHECK (char_length(location) <= 2000),
    digest TEXT CHECK (digest IS NULL OR char_length(digest) <= 100),
    document JSONB CHECK (
        document IS NULL
        OR (jsonb_typeof(document) = 'object' AND octet_length(document::text) <= 1048576)
    ),
    error TEXT CHECK (error IS NULL OR char_length(error) <= 2000),
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (source_id, entry_kind, entry_key, version),
    CHECK ((document IS NULL) = (error IS NOT NULL))
);

CREATE INDEX directory_entries_workspace ON directory_entries (workspace_id)
    WHERE workspace_id IS NOT NULL;

-- The contract step.
DROP TRIGGER IF EXISTS settings_module_switch_mirror ON settings;
DROP FUNCTION IF EXISTS module_installs_mirror_setting();
DELETE FROM settings WHERE scope = 'workspace' AND key LIKE 'modules.%';
