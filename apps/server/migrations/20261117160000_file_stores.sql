-- K5.11: which store holds a file's bytes, so files can live in more than
-- one (local disk, an S3-compatible bucket) and move between them (K5.13).
ALTER TABLE files ADD COLUMN store_id TEXT NOT NULL DEFAULT 'local'
    CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,31}$');
ALTER TABLE link_previews ADD COLUMN image_store_id TEXT
    CHECK (image_store_id ~ '^[a-z][a-z0-9_-]{0,31}$');
UPDATE link_previews SET image_store_id = 'local' WHERE image_key IS NOT NULL;

-- Bytes written to a store before their file row exists. An upload streams
-- its bytes first and records its row after; the row's transaction deletes
-- this entry. One left behind (a crash, a refused upload whose cleanup
-- failed) names bytes nothing points at, and the sweep deletes them.
CREATE TABLE file_objects_pending (
    storage_key TEXT PRIMARY KEY CHECK (storage_key ~ '^[0-9a-f/.a-z-]{1,200}$'),
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    store_id TEXT NOT NULL CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,31}$'),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX file_objects_pending_started ON file_objects_pending (started_at);
SELECT tenancy_protect('file_objects_pending', 'workspace_id');
