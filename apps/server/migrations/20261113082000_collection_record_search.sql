-- AWD-110: records in search.
--
-- A record's words (its title, and every text value: text and URL fields,
-- and the IDs choice, person and link values store, which never match
-- words people type) are kept as a generated tsvector, the same way
-- tasks and pages search, so `search` finds records with one index.
ALTER TABLE collection_records
    ADD COLUMN search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english'::regconfig, title), 'A')
        || setweight(jsonb_to_tsvector('english'::regconfig, field_values, '["string"]'), 'B')
    ) STORED;

CREATE INDEX collection_records_search ON collection_records USING GIN (search_vector);
