-- K3.25 (ruling R24 rule 7): history is never cut; old history moves to
-- cheap storage and is read back from there, slower but whole.
--
-- The log: before retention lets a workspace's old facts go, the
-- `events.archive` job writes them to its store as one compressed,
-- position-ordered file per run, recorded here with the range it holds and
-- the things it names. Retention deletes a fact only once a file holds it,
-- and the activity and audit readers load the files a page reaches back
-- into. `workspace_id` is a plain column like the log's own (the direct
-- scope has no workspace row until K4.23).
CREATE TABLE event_archives (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL,
    first_position BIGINT NOT NULL,
    last_position BIGINT NOT NULL CHECK (last_position >= first_position),
    first_at TIMESTAMPTZ NOT NULL,
    last_at TIMESTAMPTZ NOT NULL,
    row_count INTEGER NOT NULL CHECK (row_count > 0),
    thing_ids UUID[] NOT NULL,
    store_id TEXT NOT NULL CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,47}$'),
    object_key TEXT NOT NULL UNIQUE CHECK (object_key ~ '^[0-9a-z/.-]{1,200}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX event_archives_range ON event_archives (workspace_id, last_position);
CREATE INDEX event_archives_things ON event_archives USING gin (thing_ids);
SELECT tenancy_protect('event_archives', 'workspace_id');

-- Page versions: a version older than a year (never a page's newest) has
-- its body moved into a compressed file of that page's old versions; the
-- row stays, with the body's digest so the history list still says what
-- changed. Opening or restoring it brings the body back first.
CREATE TABLE history_archives (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind ~ '^[a-z_]{1,40}$'),
    thing_id UUID NOT NULL,
    first_version BIGINT NOT NULL,
    last_version BIGINT NOT NULL CHECK (last_version >= first_version),
    version_count INTEGER NOT NULL CHECK (version_count > 0),
    store_id TEXT NOT NULL CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,47}$'),
    object_key TEXT NOT NULL UNIQUE CHECK (object_key ~ '^[0-9a-z/.-]{1,200}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX history_archives_thing ON history_archives (kind, thing_id);
SELECT tenancy_protect('history_archives', 'workspace_id');

ALTER TABLE project_page_revisions
    ADD COLUMN archive_id UUID REFERENCES history_archives(id),
    ADD COLUMN body_md5 TEXT,
    ALTER COLUMN body DROP NOT NULL,
    ADD CONSTRAINT project_page_revisions_body_kept
        CHECK (body IS NOT NULL OR (archive_id IS NOT NULL AND body_md5 IS NOT NULL));
CREATE INDEX project_page_revisions_cold ON project_page_revisions (created_at)
    WHERE archive_id IS NULL;
