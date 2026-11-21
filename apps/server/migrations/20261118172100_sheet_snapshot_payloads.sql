-- C1.34 (lane D2, documents): a sheet's old snapshots are compacted like a
-- board's versions. The `history.compact` job moves the state of a snapshot
-- older than 30 days that a newer one has replaced as the sheet's latest to
-- the file store (`payload_key`, the state left as an empty object);
-- showing or restoring a revision built on it reads it back from there.

ALTER TABLE sheet_snapshots
    ADD COLUMN payload_key TEXT
        CHECK (payload_key IS NULL OR payload_key ~ '^[a-z0-9][a-z0-9./-]{0,199}$');

CREATE INDEX sheet_snapshots_to_compact ON sheet_snapshots (created_at)
    WHERE payload_key IS NULL;
