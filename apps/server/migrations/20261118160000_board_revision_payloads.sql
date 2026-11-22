-- C1.34 (lane D2, documents): history is never cut. A board's saved
-- versions are no longer dropped past the newest 100; instead the
-- `history.compact` job moves the elements of versions older than 30 days
-- to the file store (`payload_key`, the elements left as an empty array),
-- and showing or restoring such a version reads them back from there.

ALTER TABLE whiteboard_revisions
    ADD COLUMN payload_key TEXT
        CHECK (payload_key IS NULL OR payload_key ~ '^[a-z0-9][a-z0-9./-]{0,199}$');

CREATE INDEX whiteboard_revisions_to_compact ON whiteboard_revisions (created_at)
    WHERE payload_key IS NULL;
