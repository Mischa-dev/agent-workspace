-- C1.17 (lane D2, documents): conditional formatting.
--
-- * `sheet_tabs.conditional_formats`: a tab's rules in order, each a
--   range, a test ("when") and a style; the first that applies to a cell
--   wins. Kept with the tab, so they go with it into history and files.
-- * `sheet_cells.conditional`: how a cell looks once the rules apply, when
--   that differs from its own look, worked out by the engine with the
--   values so every reader sees the same.

ALTER TABLE sheet_tabs
    ADD COLUMN conditional_formats JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(conditional_formats) = 'array'
        AND octet_length(conditional_formats::text) <= 65536
    );

ALTER TABLE sheet_cells
    ADD COLUMN conditional JSONB CHECK (
        conditional IS NULL
        OR (jsonb_typeof(conditional) = 'object' AND octet_length(conditional::text) <= 1024)
    );
