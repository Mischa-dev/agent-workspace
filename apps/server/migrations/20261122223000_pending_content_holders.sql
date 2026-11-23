-- How many in-flight writes staged the same bytes and haven't yet claimed
-- them with a row or given them back. Taking a stage back only deletes the
-- bytes once nobody else still holds them.
ALTER TABLE file_objects_pending
    ADD COLUMN holders INTEGER NOT NULL DEFAULT 1 CHECK (holders >= 0);
