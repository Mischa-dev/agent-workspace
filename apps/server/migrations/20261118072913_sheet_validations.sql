-- C1.15 (lane D2, documents): validation and dropdowns. A tab keeps its
-- rules for what its cells may hold (a list of choices, a checkbox, a number,
-- a date, a text length), each over a range; a strict rule refuses a write
-- that breaks it, a lenient one marks the cell. The rules are the
-- `sheets::validation::ValidationRule` JSON and follow inserted and deleted
-- rows and columns like the tab's conditional formats.

ALTER TABLE sheet_tabs
    ADD COLUMN validations JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(validations) = 'array' AND octet_length(validations::text) <= 262144);
