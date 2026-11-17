-- T1.17 (early field work): formula fields keep each record's results in
-- `collection_records.computed_values`, keyed by the formula field's ID, so
-- views filter, sort and group by them the way they read `field_values`.
-- A result is a plain JSON value, or `{"error": "..."}` when the formula
-- gave an error for that record (shown in the cell, filtered as empty).
--
-- The server writes the column in the same transaction as the record's own
-- values, and a job fills it for every record when a formula is made or
-- changed. Existing records start empty: no formula fields exist yet.
-- `field_definitions.field_type` already allows 'formula' (20261114161000).

ALTER TABLE collection_records
    ADD COLUMN computed_values JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(computed_values) = 'object'
        AND octet_length(computed_values::text) <= 65536
    );
