-- A board version or sheet snapshot moved to the file store names the store
-- it went to, as a page's history archive does: switching the primary
-- store later leaves it readable where it is. Payloads moved before this
-- went to the server's local store, the only one compaction had used.
ALTER TABLE whiteboard_revisions
    ADD COLUMN payload_store_id TEXT CHECK (payload_store_id ~ '^[a-z][a-z0-9_-]{0,47}$');
UPDATE whiteboard_revisions SET payload_store_id = 'local' WHERE payload_key IS NOT NULL;
ALTER TABLE whiteboard_revisions ADD CONSTRAINT whiteboard_revisions_payload_store
    CHECK ((payload_key IS NULL) = (payload_store_id IS NULL));

ALTER TABLE sheet_snapshots
    ADD COLUMN payload_store_id TEXT CHECK (payload_store_id ~ '^[a-z][a-z0-9_-]{0,47}$');
UPDATE sheet_snapshots SET payload_store_id = 'local' WHERE payload_key IS NOT NULL;
ALTER TABLE sheet_snapshots ADD CONSTRAINT sheet_snapshots_payload_store
    CHECK ((payload_key IS NULL) = (payload_store_id IS NULL));
