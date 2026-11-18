-- C1.14 (lane D2, documents): number formats and cell styles.
--
-- * `format`: the cell's number format, an Excel format code such as
--   `0.0%`, `$#,##0` or `yyyy-mm-dd`; NULL is automatic. Typing `10%` or
--   `$5` keeps the format it implies.
-- * `style`: bold, italic, underline, strike, text color, fill, alignment
--   and wrap, as a small object; NULL is plain.
-- * `display`: the value as its format shows it (`10%` for 0.1), kept by
--   the engine beside the value for cells with a format.
-- * A cell can now hold only a look: formatting a block reaches its empty
--   cells too, and clearing a cell keeps its look. Such a cell's input is
--   empty and it isn't counted as filled.

ALTER TABLE sheet_cells
    ADD COLUMN format TEXT CHECK (format IS NULL OR char_length(format) BETWEEN 1 AND 100),
    ADD COLUMN style JSONB CHECK (
        style IS NULL OR (jsonb_typeof(style) = 'object' AND octet_length(style::text) <= 1024)
    ),
    ADD COLUMN display TEXT CHECK (display IS NULL OR char_length(display) <= 10000);

ALTER TABLE sheet_cells DROP CONSTRAINT sheet_cells_input_check;
ALTER TABLE sheet_cells ADD CONSTRAINT sheet_cells_input_check CHECK (
    char_length(input) <= 10000
    AND (char_length(input) >= 1 OR format IS NOT NULL OR style IS NOT NULL)
);
