-- T1.7 (build 1T): indexes for reading collection records through the one
-- views engine, the built-in Tasks collection's records included (each
-- joined to its task, so views over tasks read the same table):
--
-- * the newest-first and number-ordered pages of a collection's live
--   records;
-- * title search (ILIKE '%word%') by trigram, as tasks already have;
-- * containment filters on stored values (`field_values @> '{"<field>": ...}'`,
--   which the compiler writes for "is" on one choice or person), by a
--   jsonb_path_ops GIN.

CREATE INDEX IF NOT EXISTS collection_records_live_number
    ON collection_records (collection_id, number DESC)
    WHERE archived_at IS NULL;

CREATE INDEX IF NOT EXISTS collection_records_title_trgm
    ON collection_records USING gin (title public.gin_trgm_ops);

CREATE INDEX IF NOT EXISTS collection_records_field_values_path
    ON collection_records USING gin (field_values jsonb_path_ops);
