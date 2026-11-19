-- K5.13: a file copied to another store keeps the store it came from here
-- until `files-prune-source` deletes the old copy, so going back is possible
-- until then.
ALTER TABLE files ADD COLUMN moved_from_store_id TEXT
    CHECK (moved_from_store_id ~ '^[a-z][a-z0-9_-]{0,31}$');
CREATE INDEX files_moved_from ON files (id) WHERE moved_from_store_id IS NOT NULL;
